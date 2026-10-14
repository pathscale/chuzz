use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

use crate::internal_pages::{error_html, source_html};
use blitz_dom::{Document as _, DocumentConfig, FontContext, NodeId};
use blitz_html::HtmlProvider;
use blitz_traits::navigation::{NavigationOptions, NavigationProvider};
use blitz_traits::net::{Request, Url};
use blitz_traits::shell::{ClipboardError, FileDialogFilter, ShellProvider};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::decode::decode_body;
use crate::document_loader::{NetProvider, install_web_api_shim};
use crate::nav::{NEW_TAB_URL, display_title, request_from_input};
use crate::tab_records::{DocumentGeneration, Residency, TabId, TabRegistry};
#[path = "tab_lifecycle.rs"]
mod lifecycle;
use lifecycle::{LifecycleMessage, TabLife, TabPhase};

/// The document an empty tab shows.
///
/// Transparent on purpose. It used to hardcode `background:#0f1622`, a navy
/// that no theme could reach — chuzz derives every surface from
/// `--az-surface`, which the user changes with the surface wheel, so a blank
/// tab stayed blue whatever they picked. The shell paints `.page` with the
/// themed surface and this lets it through.
const BLANK_HTML: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title></title></head><body style="margin:0;background:transparent"></body></html>"#;

const EMPTY_HTML: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>Empty response</title></head><body style="margin:0;background:#f6f6f7;color:#16181d"><h1>Empty response</h1><p>The server returned no content.</p></body></html>"#;

pub type ChuzzAppHandle = izumo::AppHandle;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FrontendTab {
    id: u64,
    title: String,
    url: String,
    status: &'static str,
    can_go_back: bool,
    can_go_forward: bool,
    residency: Residency,
    lifecycle: TabPhase,
    lifecycle_reason: &'static str,
    visible: bool,
    preview_available: bool,
    runtime_released: bool,
}

#[derive(Clone, Serialize)]
pub struct PanelSections {
    page: bool,
    history: bool,
    network: bool,
    console: bool,
    /// The verbose stream: every fetch, script, module and mount, as it
    /// happens. Open by default, because the reason it exists is that nobody
    /// knew to go looking for the information it carries.
    debugging: bool,
}

#[derive(Clone, Serialize)]
pub struct PanelState {
    collapsed: bool,
    sections: PanelSections,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusReadout {
    status: &'static str,
    url: String,
    tab_count: usize,
    node_count: usize,
    transferred: &'static str,
    sleeping_bytes: usize,
    sleeping_warning: bool,
    live_page_count: usize,
}

struct MountedPage {
    owner_document_id: u64,
    mount_node_id: u64,
    viewport_node_id: Option<u64>,
    document_id: u64,
    script: bool,
}

struct TabState {
    id: u64,
    mounted_generation: u64,
    mounted: Option<MountedPage>,
    history: Vec<Request>,
    current: usize,
    title: String,
    loading: bool,
    /// How the last completed load went. `loading` wins over it while a load
    /// is in flight, so the previous page's outcome never shows against the
    /// new one's address.
    outcome: PageOutcome,
    generation: u64,
    life: TabLife,
}

impl TabState {
    fn request(&self) -> &Request {
        &self.history[self.current]
    }

    fn snapshot(&self) -> FrontendTab {
        FrontendTab {
            id: self.id,
            title: display_title(&self.title, &self.request().url),
            url: self.request().url.to_string(),
            status: if self.loading {
                "loading"
            } else {
                self.outcome.name()
            },

            can_go_back: self.current > 0,
            can_go_forward: self.current + 1 < self.history.len(),
            residency: match self.life.phase {
                TabPhase::Sleeping | TabPhase::RestoreFailed => Residency::Sleeping,
                TabPhase::Discarding | TabPhase::Freezing => Residency::Discarding,
                TabPhase::Waking => Residency::Waking,
                _ if self.loading => Residency::Loading,
                _ if self.mounted.is_some() => Residency::Live,
                _ => Residency::Dormant,
            },
            lifecycle: self.life.phase,
            lifecycle_reason: self.life.reason,
            visible: self.life.visible,
            preview_available: self.life.preview.is_some(),
            runtime_released: matches!(self.life.phase, TabPhase::Sleeping | TabPhase::RestoreFailed) && self.mounted.is_none() && self.life.release.is_none(),
        }
    }
}

/// One line in the debugging panel.
///
/// The browser already said most of this on stderr, where nobody watching the
/// window could see it. A page that renders and then does nothing is almost
/// always a script or a module that never arrived, and that fact existed only
/// in a terminal the person looking at the blank page did not have open.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DebugEntry {
    /// Monotonic, so the window can ask for everything after what it has
    /// rather than re-reading the buffer and guessing what is new.
    seq: u64,
    /// `info`, `warn` or `error`. Drives the colour and nothing else.
    level: &'static str,
    /// Which part of the browser said it: `net`, `page`, `script`.
    source: &'static str,
    message: String,
}

/// The last [`DEBUG_LOG_CAPACITY`] things the browser did.
///
/// A ring rather than a growing list: this records every subresource of every
/// page for the life of the process, and a browser that leaks a line per
/// request is a browser that eventually stops.
struct DebugLog {
    next_seq: u64,
    entries: VecDeque<DebugEntry>,
}

const DEBUG_LOG_CAPACITY: usize = 500;

impl DebugLog {
    fn push(&mut self, level: &'static str, source: &'static str, message: String) -> DebugEntry {
        let entry = DebugEntry {
            seq: self.next_seq,
            level,
            source,
            message,
        };
        self.next_seq += 1;
        if self.entries.len() == DEBUG_LOG_CAPACITY {
            self.entries.pop_front();
        }
        self.entries.push_back(entry.clone());
        entry
    }

    /// Everything the caller has not seen. `since` is the last `seq` it holds.
    fn since(&self, since: Option<u64>) -> Vec<DebugEntry> {
        match since {
            Some(seq) => self
                .entries
                .iter()
                .filter(|entry| entry.seq > seq)
                .cloned()
                .collect(),
            None => self.entries.iter().cloned().collect(),
        }
    }
}

struct BrowserState {
    tabs: Vec<TabState>,
    records: TabRegistry,
    active: u64,
    next_tab_id: u64,
    panel: PanelState,
}

struct PageBundle {
    net: Arc<NetProvider>,
    tab_id: u64,
    generation: u64,
    resolved_url: String,
    source: PageSource,
    outcome: PageOutcome,
}

/// How a load went, in the five states the tab indicator can show.
///
/// Ordered by severity so a page with more than one thing wrong reports the
/// worst of them. Deliberately not a boolean pair: "loaded" and "loaded with
/// something missing" are the states a person actually wants to tell apart at
/// a glance, and a browser that only says loading/not-loading makes a page
/// whose scripts all 404'd look exactly like one that worked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PageOutcome {
    /// Nothing has been asked for. `about:blank`, a new tab.
    Blank,
    /// The document arrived and everything it named arrived with it.
    Ready,
    /// The document arrived; something it named did not. A script that could
    /// not be fetched, a module that would not validate, an empty body.
    Warning,
    /// The document itself did not arrive.
    Error,
}

impl PageOutcome {
    /// The name the window uses. Kept next to the variants so the two cannot
    /// drift, and matched by `LoadStatus` in the frontend's `types`.
    fn name(self) -> &'static str {
        match self {
            Self::Blank => "blank",
            Self::Ready => "ready",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }
}

/// Where a page's document comes from.
enum PageSource {
    Html {
        html: String,
        scripts: HashMap<Url, String>,
    },
    /// The bytes a server sent, shown rather than rendered.
    Source { text: String },
}

/// How long the window waits for a script the page asked for while running.
///
/// Shorter than the capture's, and the reason is worth stating plainly:
/// `ScriptFetcher::fetch` is synchronous and the page's scripts run on the UI
/// thread, so this blocks the whole window, other tabs included, for as long
/// as it waits. The alternative is what happened before, which was to drop the
/// script and render a page missing whatever it was going to build. A short
/// stall is the better of the two, but only a short one; a page cannot be
/// allowed to freeze the browser because one of its servers went quiet.
///
/// The real answer is an asynchronous script-loading path in the engine, which
/// would not need to choose.
const WINDOW_SCRIPT_DEADLINE: std::time::Duration = std::time::Duration::from_secs(5);

/// How long one `fetch` or `XMLHttpRequest` from the page may take.
///
/// Generous compared to the script deadline, and it can afford to be: this one
/// blocks nothing. The request is asynchronous, the window keeps painting, and
/// the answer arrives on a later poll.
const WINDOW_NETWORK_DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);

struct BrowserInner {
    state: Mutex<BrowserState>,
    log: Mutex<DebugLog>,
    net: Mutex<Arc<NetProvider>>,
    user_agent_spoofing: Mutex<bool>,
    completed: Mutex<VecDeque<PageBundle>>,
    lifecycle: Mutex<VecDeque<LifecycleMessage>>,
    sleeping_budget: Arc<crate::tab_stash::SleepingBudget>,
    sleep_delay_ns: u64,
    app: Mutex<Option<ChuzzAppHandle>>,

}

/// Queue embedded-page redraws without blocking network workers on AppKit.
///
/// Boa resolves a runtime-discovered module synchronously on the UI thread.
/// A direct macOS redraw from every resource-completion worker also waits for
/// that UI thread, so a page with many images can occupy the runtime pool with
/// workers waiting for the same thread that is waiting for the module fetch.
/// Coalescing redraws onto izumo's document-thread queue breaks that cycle.
struct QueuedRedrawShell {
    inner: Arc<dyn ShellProvider>,
    app: ChuzzAppHandle,
    redraw_pending: Arc<AtomicBool>,
    /// The thread that built this shell, which is the document thread.
    document_thread: std::thread::ThreadId,
}

impl QueuedRedrawShell {
    fn new(inner: Arc<dyn ShellProvider>, app: ChuzzAppHandle) -> Self {
        Self {
            inner,
            app,
            redraw_pending: Arc::new(AtomicBool::new(false)),
            document_thread: std::thread::current().id(),
        }
    }
}

impl ShellProvider for QueuedRedrawShell {
    fn request_redraw(&self) {
        // On the document thread there is no worker to unblock, and queueing
        // would hand the script queue a task that counts as runnable work, so
        // an eval in any poll hook would keep a settling document busy forever.
        if std::thread::current().id() == self.document_thread {
            self.inner.request_redraw();
            return;
        }
        if self.redraw_pending.swap(true, Ordering::AcqRel) {
            return;
        }
        let inner = Arc::clone(&self.inner);
        let pending = Arc::clone(&self.redraw_pending);
        if self
            .app
            .run_on_main_thread(move || {
                pending.store(false, Ordering::Release);
                inner.request_redraw();
            })
            .is_err()
        {
            self.redraw_pending.store(false, Ordering::Release);
        }
    }

    fn set_cursor(&self, icon: Option<cursor_icon::CursorIcon>) {
        self.inner.set_cursor(icon);
    }

    fn set_window_title(&self, title: String) {
        self.inner.set_window_title(title);
    }

    fn set_ime_enabled(&self, is_enabled: bool) {
        self.inner.set_ime_enabled(is_enabled);
    }

    fn set_ime_cursor_area(&self, x: f32, y: f32, width: f32, height: f32) {
        self.inner.set_ime_cursor_area(x, y, width, height);
    }

    fn get_clipboard_text(&self) -> Result<String, ClipboardError> {
        self.inner.get_clipboard_text()
    }

    fn set_clipboard_text(&self, text: String) -> Result<(), ClipboardError> {
        self.inner.set_clipboard_text(text)
    }

    fn open_file_dialog(
        &self,
        multiple: bool,
        filter: Option<FileDialogFilter>,
    ) -> Vec<std::path::PathBuf> {
        self.inner.open_file_dialog(multiple, filter)
    }

    fn request_window_close(&self) {
        self.inner.request_window_close();
    }

    fn set_window_minimized(&self, minimized: bool) {
        self.inner.set_window_minimized(minimized);
    }

    fn set_window_maximized(&self, maximized: bool) {
        self.inner.set_window_maximized(maximized);
    }

    fn is_window_maximized(&self) -> bool {
        self.inner.is_window_maximized()
    }

    fn set_window_decorations(&self, decorations: bool) {
        self.inner.set_window_decorations(decorations);
    }

    fn drag_window(&self) {
        self.inner.drag_window();
    }
}

#[derive(Clone)]
pub struct Browser(Arc<BrowserInner>);

impl Browser {
    pub fn new(startup: Option<Url>) -> Self {
        Self::build(startup)
    }

    /// Compatibility entry point for callers of the retired guest API.
    pub fn with_wasm_page(_module: std::path::PathBuf) -> Result<Self, String> {
        Err(crate::wasm_retired::WasmUnsupported.to_string())
    }

    fn build(startup: Option<Url>) -> Self {
        let first = startup.unwrap_or_else(|| Url::parse(NEW_TAB_URL).unwrap());
        let user_agent_spoofing = stored_user_agent().spoofing;
        Self(Arc::new(BrowserInner {
            state: Mutex::new(BrowserState {
                tabs: vec![TabState {
                    id: 0,
                    mounted_generation: 0,
                    mounted: None,
                    history: vec![crate::nav::browser_request(first)],
                    current: 0,
                    title: String::new(),
                    loading: false,
                    outcome: PageOutcome::Blank,
                    generation: 0,
                    life: TabLife::default(),
                }],
                records: {
                    let mut records = TabRegistry::default();
                    records.admit(TabId(0)).expect("initial tab admission");
                    records
                },
                active: 0,
                next_tab_id: 1,
                panel: PanelState {
                    collapsed: true,
                    sections: PanelSections {
                        page: true,
                        history: true,
                        network: false,
                        console: false,
                        debugging: true,
                    },
                },
            }),
            log: Mutex::new(DebugLog {
                next_seq: 1,
                entries: VecDeque::new(),
            }),
            net: Mutex::new(Arc::new(NetProvider::with_user_agent(
                None,
                &crate::identity::user_agent_for_spoofing(user_agent_spoofing),
            ))),
            user_agent_spoofing: Mutex::new(user_agent_spoofing),
            completed: Mutex::new(VecDeque::new()),
            lifecycle: Mutex::new(VecDeque::new()),
            sleeping_budget: Arc::new(crate::tab_stash::SleepingBudget::default()),
            sleep_delay_ns: lifecycle::sleep_delay_ns(),
            app: Mutex::new(None),
        }))
    }

    pub fn attach_app(&self, app: ChuzzAppHandle) {
        *self.0.app.lock().unwrap() = Some(app);
        self.schedule_current(0, false);
    }

    pub fn install_document_lifecycle(&self, document: &mut blitz_script::ScriptDocument) {
        self.install_surface_lifecycle();
        #[cfg(unix)]
        {
            let weak = Arc::downgrade(&self.0);
            izumo::set_page_target_provider(move || {
                let Some(inner) = weak.upgrade() else {
                    return Vec::new();
                };
                let state = inner.state.lock().unwrap();
                state
                    .tabs
                    .iter()
                    .map(|tab| {
                        let mounted = tab.mounted.as_ref();
                        let script = mounted.is_some_and(|page| page.script);
                        izumo::PageTarget {
                            owner_document_id: mounted.map(|page| page.owner_document_id),
                            mount_node_id: mounted.map(|page| page.mount_node_id),
                            viewport_node_id: mounted.and_then(|page| page.viewport_node_id),
                            info: blitz_control_protocol::TargetInfo {
                                id: blitz_control_protocol::TargetId::Tab(tab.id),
                                url: tab.request().url.to_string(),
                                title: display_title(&tab.title, &tab.request().url),
                                active: state.active == tab.id,
                                generation: tab.mounted_generation,
                                document_id: mounted.map(|page| page.document_id),
                                capabilities: blitz_control_protocol::TargetCapabilities {
                                    agent: script,
                                    diagnostics: script,
                                    capture: script,
                                    console: script,
                                    runtime_errors: script,
                                    web_gpu: script,
                                },
                            },
                        }
                    })
                    .collect()
            });
            let weak = Arc::downgrade(&self.0);
            izumo::set_page_navigation_handler(move |id, url| {
                let inner = weak.upgrade().ok_or("the browser stopped")?;
                let browser = Browser(inner);
                let destination = {
                    let state = browser.0.state.lock().unwrap();
                    let tab = state
                        .tabs
                        .iter()
                        .find(|tab| tab.id == id)
                        .ok_or("the tab closed")?;
                    tab.request()
                        .url
                        .join(url)
                        .map_err(|error| error.to_string())?
                };
                if browser.navigate_request(id, crate::nav::browser_request(destination)) {
                    Ok(())
                } else {
                    Err("the tab closed".into())
                }
            });
            izumo::set_page_diagnostics_handler(|document, request| {
                use blitz_control_protocol::{DebugResponse, DiagnosticsRequest};
                match request {
                    DiagnosticsRequest::WebGpuInspect => crate::webgpu::inspect_gpu(document)
                        .map(DebugResponse::WebGpu)
                        .unwrap_or_else(DebugResponse::Error),
                    DiagnosticsRequest::WebGpuCapture { target } => {
                        crate::webgpu::capture_gpu(document, &target)
                            .map(DebugResponse::Captured)
                            .unwrap_or_else(DebugResponse::Error)
                    }
                    DiagnosticsRequest::WebGpuBuffer {
                        target,
                        offset,
                        length,
                    } => crate::webgpu::capture_gpu_buffer(document, &target, offset, length)
                        .map(DebugResponse::WebGpuBuffer)
                        .unwrap_or_else(DebugResponse::Error),
                    _ => DebugResponse::Error(blitz_control_protocol::target::error(
                        "unsupportedRequest",
                        "unsupported page diagnostic",
                    )),
                }
            });
        }
        let browser = self.clone();
        let mut pending = VecDeque::new();
        document.add_poll_hook(move |document, _| browser.poll_document(document, &mut pending));
    }

    pub(crate) fn net(&self) -> Arc<NetProvider> {
        Arc::clone(&self.0.net.lock().unwrap())
    }

    /// Open the process-wide profile jar before the first tab is scheduled.
    pub async fn open_cookie_profile(
        &self,
        directory: impl AsRef<std::path::Path>,
    ) -> Result<(), crate::cookie_store::CookieStoreError> {
        let cookies = Arc::new(crate::cookie_store::BrowserCookieStore::open(directory).await?);
        let user_agent_spoofing = *self.0.user_agent_spoofing.lock().unwrap();
        *self.0.net.lock().unwrap() = Arc::new(NetProvider::with_user_agent_and_cookies(
            None,
            &crate::identity::user_agent_for_spoofing(user_agent_spoofing),
            cookies,
        ));
        Ok(())
    }

    /// Drain cookie writes during orderly application shutdown.
    pub async fn flush_cookie_store(&self) -> Result<(), crate::cookie_store::CookieStoreError> {
        self.net().cookie_store().flush().await
    }

    fn app(&self) -> Option<ChuzzAppHandle> {
        self.0.app.lock().unwrap().clone()
    }

    fn snapshots(&self) -> (Vec<FrontendTab>, u64, PanelState, StatusReadout) {
        let state = self.0.state.lock().unwrap();
        let tabs: Vec<_> = state.tabs.iter().map(TabState::snapshot).collect();
        let active = state.active;
        let active_tab = state.tabs.iter().find(|tab| tab.id == active).unwrap();
        let status = StatusReadout {
            // The same vocabulary the tab dot uses. Two names for the same
            // state is how a strip and a dot end up disagreeing about the tab
            // they are both describing.
            status: active_tab.snapshot().status,
            url: active_tab.request().url.to_string(),
            tab_count: tabs.len(),
            node_count: 0,
            transferred: "0 B",
            sleeping_bytes: self.0.sleeping_budget.used(),
            sleeping_warning: self.0.sleeping_budget.warning(),
            live_page_count: state.tabs.iter().filter(|tab| tab.mounted.is_some() || tab.life.release.as_ref().is_some_and(|receipt| !receipt.complete())).count(),
        };
        (tabs, active, state.panel.clone(), status)
    }

    /// Record something the browser did, for the debugging panel.
    ///
    /// Also goes to stderr, because a browser that only says what happened
    /// inside its own window is unusable from a script and from CI. The panel
    /// is the copy for whoever is looking at the window.
    fn note(&self, level: &'static str, source: &'static str, message: impl Into<String>) {
        let message = message.into();
        eprintln!("chuzz: {source}: {message}");
        let entry = self.0.log.lock().unwrap().push(level, source, message);
        if let Some(app) = self.app() {
            let _ = app.emit("debug-entry", entry);
        }
    }

    fn emit_state(&self) {
        let Some(app) = self.app() else { return };
        let (tabs, active, panel, status) = self.snapshots();
        let _ = app.emit("tabs-changed", tabs);
        let _ = app.emit("active-tab-changed", active);
        let _ = app.emit("panel-changed", panel);
        let _ = app.emit("status-changed", status);
    }

    fn wake_document(&self) {
        if let Some(app) = self.app() {
            app.script_queue().enqueue_task(|| {});
        }
    }

    fn schedule_current(&self, tab_id: u64, revalidate: bool) {
        let (request, generation) = {
            let mut state = self.0.state.lock().unwrap();
            let Some(tab) = state.tabs.iter_mut().find(|tab| tab.id == tab_id) else {
                return;
            };
            if tab.life.phase == TabPhase::Discarding { tab.life.wake_requested = true; return; }
            if tab.life.discarded && tab.mounted.is_none() && !matches!(tab.request().method.as_str(), "GET" | "HEAD") {
                tab.life.phase = TabPhase::RestoreFailed; tab.life.reason = "reload-requires-resubmission";
                drop(state); self.emit_state(); return;
            }
            tab.life.cancel_deadline();
            tab.life.await_preview = false;
            if tab.life.discarded && tab.mounted.is_none() { tab.life.phase = TabPhase::Waking; }
            tab.generation = tab.generation.checked_add(1).expect("document generation exhausted");
            tab.loading = true;

            (tab.request().clone(), tab.generation)
        };
        self.0.state.lock().unwrap().records.loading(TabId(tab_id), DocumentGeneration(generation));
        self.emit_state();

        // A wasm page has nothing to fetch, so its bundle is complete the
        // moment it is made and never goes near the network or the runtime.
        // Matched on the URL rather than the tab id, so navigating this tab
        // elsewhere is an ordinary navigation and coming back re-runs the guest.
        if let Some(page) = self.0.wasm.as_ref().filter(|page| page.url == request.url) {
            self.0.completed.lock().unwrap().push_back(PageBundle {
                net: self.net(),
                tab_id,
                generation,
                resolved_url: page.url.to_string(),
                source: PageSource::Wasm {
                    module: page.module.clone(),
                },
                // A guest that will not build becomes an error page where it is
                // mounted, which is also where that outcome is decided.
                outcome: PageOutcome::Ready,
            });
            self.wake_document();
            return;
        }

        let browser = self.clone();
        nagoya::spawn(async move {
            let bundle = fetch_page(&browser, tab_id, generation, request, revalidate).await;
            browser.0.completed.lock().unwrap().push_back(bundle);
            browser.wake_document();
        });
    }

    fn schedule_browser_navigation(&self, tab_id: u64, revalidate: bool) {
        let net = self.net();
        {
            let mut state = self.0.state.lock().unwrap();
            let Some(tab) = state.tabs.iter_mut().find(|tab| tab.id == tab_id) else { return; };
            let url = tab.request().url.clone();
            let current = tab.current;
            tab.history[current].initiator = net.browser_initiator(url);
        }
        self.schedule_current(tab_id, revalidate);
    }

    fn navigate_request(&self, tab_id: u64, request: Request) -> bool {
        {
            let mut state = self.0.state.lock().unwrap();
            let Some(tab) = state.tabs.iter_mut().find(|tab| tab.id == tab_id) else {
                return false;
            };
            tab.life.navigate();
            tab.history.truncate(tab.current + 1);
            tab.history.push(request);
            tab.current += 1;
            if tab.history.len() > 128 { tab.history.remove(0); tab.current -= 1; }
            tab.title.clear();
        }
        self.schedule_current(tab_id, false);
        true
    }

    fn poll_document(
        &self,
        ui: &mut blitz_script::ScriptDocument,
        pending: &mut VecDeque<PageBundle>,
    ) -> bool {
        pending.extend(self.0.completed.lock().unwrap().drain(..));
        let mut retained = VecDeque::new();
        let mut changed = self.poll_lifecycle(ui);

        while let Some(bundle) = pending.pop_front() {
            let current = {
                let state = self.0.state.lock().unwrap();
                state
                    .tabs
                    .iter()
                    .find(|tab| tab.id == bundle.tab_id)
                    .is_some_and(|tab| tab.generation == bundle.generation)
            };
            if !current {
                continue;
            }

            let Some(target) = page_node(&ui.inner(), bundle.tab_id) else {
                retained.push_back(bundle);
                continue;
            };

            let direct_shell = ui.inner().shell_provider.clone();
            let shell_provider: Arc<dyn ShellProvider> = match self.app() {
                Some(app) => Arc::new(QueuedRedrawShell::new(direct_shell, app)),
                None => direct_shell,
            };

            let net = Arc::clone(&bundle.net);
            let lifetime = blitz_dom::release::Teardown::new();
            let page_signal = blitz_traits::net::AbortSignal::default();
            let page_net = Arc::new(crate::tab_resources::PageNet { net: Arc::clone(&net), lifetime: Arc::clone(&lifetime) });
            let make_config = || DocumentConfig {
                base_url: Some(bundle.resolved_url.clone()),
                net_provider: Some(Arc::clone(&page_net) as _),
                teardown: Some(Arc::clone(&lifetime)),
                abort_signal: Some(page_signal.clone()),
                navigation_provider: Some(Arc::new(PageNavigation {
                    browser: Arc::downgrade(&self.0),
                    tab_id: bundle.tab_id,
                    initiator: net.initiator(),
                })),
                shell_provider: Some(shell_provider.clone()),
                html_parser_provider: Some(Arc::new(HtmlProvider)),
                font_ctx: Some(FontContext::default()),
                ..Default::default()
            };
            // Preserve the outcome established during loading.
            let outcome = bundle.outcome;
            let (page, title): (Box<dyn blitz_dom::Document>, String) = match bundle.source {

                PageSource::Html { html, scripts } => {
                    let mut page = crate::webgpu::script_document(&html, make_config())
                        .with_fetcher(crate::script_fetch::PageScripts::new(
                            scripts,
                            Arc::clone(&net),
                            WINDOW_SCRIPT_DEADLINE,
                        ));
                    self.prepare_tab_page(&mut page, bundle.tab_id, bundle.generation, &bundle.resolved_url);
                    install_web_api_shim(&mut page, &net);
                    crate::net_bridge::install(
                        &mut page,
                        Arc::clone(&net),
                        WINDOW_NETWORK_DEADLINE,
                        blitz_traits::net::Url::parse(&bundle.resolved_url).ok(),
                    );
                    page.execute_scripts();
                    let title = page
                        .inner()
                        .find_title_node()
                        .map(|node| node.text_content())
                        .unwrap_or_default();
                    (Box::new(page), title)
                }
                PageSource::Source { text } => {
                    let page =
                        blitz_html::HtmlDocument::from_html(&source_html(&text), make_config())
                            .into_inner();
                    (Box::new(page), format!("source of {}", bundle.resolved_url))
                }

            };
            self.note(
                "info",
                "page",
                format!(
                    "attached {} to tab {} as {}",
                    bundle.resolved_url,
                    bundle.tab_id,
                    outcome.name()
                ),
            );
            let failed_wake = outcome == PageOutcome::Error && self.0.state.lock().unwrap().tabs.iter().any(|tab| tab.id == bundle.tab_id && tab.life.phase == TabPhase::Waking);
            if failed_wake {
                let receipt = page.dispose();
                let mut state = self.0.state.lock().unwrap();
                if let Some(tab) = state.tabs.iter_mut().find(|tab| tab.id == bundle.tab_id && tab.generation == bundle.generation) {
                    tab.loading = false;
                    tab.life.phase = TabPhase::RestoreFailed;
                    tab.life.reason = "reload-failed";
                    tab.life.preview_surface.take();
                    tab.outcome = outcome;
                }
                drop(state);
                if !receipt.complete() { self.track_release(bundle.tab_id, receipt); }
                changed = true;
                continue;
            }
            let mounted = MountedPage {
                owner_document_id: ui.inner().id() as u64,
                mount_node_id: target.as_u64(),
                viewport_node_id: ui
                    .inner()
                    .query_selector(".page-stack")
                    .ok()
                    .flatten()
                    .map(NodeId::as_u64),
                document_id: page.id() as u64,
                script: (&*page as &dyn std::any::Any).is::<blitz_script::ScriptDocument>(),
            };
            {
                let mut state = self.0.state.lock().unwrap();
                let Some(tab) = state
                    .tabs
                    .iter_mut()
                    .find(|tab| tab.id == bundle.tab_id && tab.generation == bundle.generation)
                else {
                    continue;
                };
                ui.inner_mut().set_sub_document(target, page);
                tab.mounted_generation += 1;
                tab.mounted = Some(mounted);
                if let Ok(url) = Url::parse(&bundle.resolved_url) {
                    tab.history[tab.current].url = url;
                }
                tab.title = title;
                tab.loading = false;
                tab.life.visible = false;
                tab.life.hidden_at = nagoya::now_ns();
                tab.life.preview_surface.take();
                if tab.life.phase != TabPhase::Waking { tab.life.phase = TabPhase::Live; }
                tab.outcome = outcome;
                state.records.attached(TabId(bundle.tab_id), DocumentGeneration(bundle.generation));
            }
            self.attached_lifecycle(bundle.tab_id);
            changed = true;
        }

        *pending = retained;
        if changed {
            self.emit_state();
        }
        changed
    }
}

struct PageNavigation {
    browser: Weak<BrowserInner>,
    tab_id: u64,
    initiator: blitz_traits::net::Initiator,
}

impl NavigationProvider for PageNavigation {
    fn navigate_to(&self, options: NavigationOptions) {
        if let Some(inner) = self.browser.upgrade() {
            Browser(inner).navigate_request(self.tab_id, options.into_request().initiator(self.initiator.clone()));
        }
    }
}


pub(crate) fn page_node(document: &blitz_dom::BaseDocument, tab_id: u64) -> Option<NodeId> {
    if let Some(node) = document.get_element_by_id(&format!("chuzz-page-{tab_id}")) {
        return Some(node);
    }
    let tab_id = tab_id.to_string();
    document
        .query_selector_all("web-view")
        .ok()?
        .into_iter()
        .find(|node_id| {
            document
                .get_node(*node_id)
                .and_then(|node| node.element_data())
                .is_some_and(|element| {
                    element.attrs.iter().any(|attr| {
                        attr.name.local.as_ref() == "data-tab-id" && attr.value == tab_id
                    })
                })
        })
}

fn revalidate(request: &mut Request) {
    request.headers.insert(
        blitz_traits::net::http::header::CACHE_CONTROL,
        blitz_traits::net::http::HeaderValue::from_static("no-cache"),
    );
}

async fn fetch_page(
    browser: &Browser,
    tab_id: u64,
    generation: u64,
    mut request: Request,
    force_revalidate: bool,
) -> PageBundle {
    let mut net = browser.net();
    if matches!(request.initiator, blitz_traits::net::Initiator::Browser(_)) {
        request.initiator = net.browser_initiator(request.url.clone());
    }
    let navigation = request.initiator.clone();
    browser.note("info", "nav", format!("navigating to {}", request.url));
    if let Some(html) = crate::internal_pages::html_for(&request.url) {
        return PageBundle {
            net: Arc::clone(&net),
            tab_id,
            generation,
            resolved_url: request.url.to_string(),
            source: PageSource::Html {
                html,
                scripts: HashMap::new(),
            },
            outcome: PageOutcome::Ready,
        };
    }
    if request.url.scheme() == "about" {
        return PageBundle {
            net: Arc::clone(&net),
            tab_id,
            generation,
            resolved_url: request.url.to_string(),
            source: PageSource::Html {
                html: BLANK_HTML.to_owned(),
                scripts: HashMap::new(),
            },
            outcome: PageOutcome::Blank,
        };
    }
    // `view-source:` is answered by fetching what it names and showing the
    // bytes instead of rendering them. The prefix stays on the tab's URL, so
    // history and the address bar hold it like any other address.
    if request.url.scheme() == "view-source" {
        let inner = request.url.path().to_owned();
        // The outcome tracks whether the *bytes arrived*, not whether the
        // source document rendered. A view-source tab that shows a fetch
        // failure is a red tab showing what went wrong, which is the same
        // reading as an ordinary failed page.
        let (source, outcome) = match Url::parse(&inner) {
            Ok(url) => match net.fetch_async(Request::get(url.clone()).initiator(
                if matches!(navigation, blitz_traits::net::Initiator::Browser(_)) {
                    net.browser_initiator(url.clone())
                } else { navigation.clone() }
            )).await {
                Ok((_, bytes)) => {
                    browser.note(
                        "info",
                        "net",
                        format!("fetched {inner} for view-source, {} bytes", bytes.len()),
                    );
                    (decode_body(&bytes), PageOutcome::Ready)
                }
                Err(error) => {
                    browser.note(
                        "error",
                        "net",
                        format!("could not fetch {inner}: {error:?}"),
                    );
                    if let blitz_net::ProviderError::TlsPolicy(reason) = &error {
                        return PageBundle {
                            net: Arc::clone(&net),
                            tab_id,
                            generation,
                            resolved_url: request.url.to_string(),
                            source: PageSource::Html {
                                html: crate::internal_pages::tls_refused_html(&url, *reason),
                                scripts: HashMap::new(),
                            },
                            outcome: PageOutcome::Error,
                        };
                    }
                    (
                        format!("could not fetch {inner}: {error:?}"),
                        PageOutcome::Error,
                    )
                }
            },
            Err(error) => (format!("{inner} is not a URL: {error}"), PageOutcome::Error),
        };
        return PageBundle {
            net: Arc::clone(&net),
            tab_id,
            generation,
            resolved_url: request.url.to_string(),
            source: PageSource::Source { text: source },
            outcome,
        };
    }
    if force_revalidate {
        revalidate(&mut request);
    }
    let requested_url = request.url.to_string();

    let (resolved_url, bytes) = match net.fetch_document_async(request).await {
        Ok(response) => response,
        Err(error) => {
            browser.note(
                "error",
                "net",
                format!("{requested_url} did not load: {error:?}"),
            );
            return PageBundle {
                net: Arc::clone(&net),
                tab_id,
                generation,
                resolved_url: requested_url.clone(),
                source: PageSource::Html {
                    html: match &error {
                        blitz_traits::platform::FetchError::TlsPolicy(reason) => {
                            let url = Url::parse(&requested_url).expect("request URL is valid");
                            crate::internal_pages::tls_refused_html(&url, *reason)
                        }
                        _ => error_html(&format!("{error:?}")),
                    },
                    scripts: HashMap::new(),
                },
                outcome: PageOutcome::Error,
            };
        }
    };
    // Worst-wins from here: the document arrived, so the floor is Ready, and
    // each thing the page named that did not arrive can only raise it.
    net = Arc::new(net.for_document(Url::parse(&resolved_url).expect("provider returned a URL"), &navigation));
    let mut outcome = PageOutcome::Ready;
    browser.note(
        "info",
        "net",
        format!("loaded {resolved_url}, {} bytes", bytes.len()),
    );
    let html = if bytes.is_empty() {
        // A 200 with nothing in it is not a failed load and not a good one.
        // The reader gets a page explaining that, and a tab that says so.
        outcome = outcome.max(PageOutcome::Warning);
        browser.note("warn", "net", "the server returned an empty body");
        EMPTY_HTML.to_owned()
    } else {
        decode_body(&bytes)
    };
    // Discover JavaScript without constructing a DOM that would be discarded.
    // XHTML retains its XML parser and complete tree.
    let urls = if blitz_html::DocumentHtmlParser::is_xhtml_document(&html) {
        let document = blitz_script::ScriptDocument::from_html(
            &html,
            DocumentConfig {
                base_url: Some(resolved_url.clone()),
                ..Default::default()
            },
        );
        document.external_script_urls()
    } else {
        let tags = blitz_html::scan_script_tags(&html);
        let base_url = Url::parse(&resolved_url).ok();
        tags.iter()
            .filter(|tag| tag.is_javascript())
            .filter_map(|tag| tag.src.as_ref())
            .filter_map(|src| match &base_url {
                Some(base) => base.join(src.as_ref()).ok(),
                None => Url::parse(src.as_ref()).ok(),
            })
            .collect()
    };
    let mut scripts = HashMap::new();
    for url in urls {
        if scripts.contains_key(&url) {
            continue;
        }
        let mut script_request = Request::get(url.clone());
        if force_revalidate {
            revalidate(&mut script_request);
        }
        match net.fetch_async(script_request).await {
            Ok((_, bytes)) => {
                browser.note(
                    "info",
                    "script",
                    format!("fetched {url}, {} bytes", bytes.len()),
                );
                scripts.insert(url, decode_body(&bytes));
            }
            // A script that never arrives is the single most common reason a
            // page renders but does nothing, and it used to be swallowed here
            // without a trace anywhere in the window.
            Err(error) => {
                browser.note(
                    "error",
                    "script",
                    format!("could not fetch {url}: {error:?}"),
                );
                outcome = outcome.max(PageOutcome::Warning);
            }
        }
    }
    PageBundle {
        net: Arc::clone(&net),
        tab_id,
        generation,
        resolved_url,
        source: PageSource::Html { html, scripts },
        outcome,
    }
}

#[derive(Deserialize)]
struct NoArgs {}

#[derive(Deserialize)]
struct TabArgs {
    id: u64,
}

#[derive(Deserialize)]
struct OpenTabArgs {
    url: Option<String>,
}

#[derive(Deserialize)]
struct NavigateArgs {
    id: u64,
    input: String,
}

#[derive(Deserialize)]
struct PanelCollapsedArgs {
    collapsed: bool,
}

#[derive(Deserialize)]
struct SectionArgs {
    section: String,
}

#[derive(Deserialize)]
struct DebugLogArgs {
    since: Option<u64>,
}

#[derive(Deserialize)]
struct UserAgentSpoofingArgs {
    spoofing: bool,
}

#[derive(Deserialize)]
struct DiagnosticsArgs {
    inspection: bool,
    profiling: bool,
}

fn command_handler<A, R>(
    browser: &Arc<Browser>,
    handler: impl Fn(Arc<Browser>, A) -> Result<R, String> + Send + Sync + 'static,
) -> izumo::CommandHandler
where
    A: DeserializeOwned,
    R: Serialize,
{
    let browser = Arc::clone(browser);
    Arc::new(move |args| {
        let args = serde_json::from_value(args)
            .map_err(|error| format!("invalid command arguments: {error}"))?;
        let reply = handler(Arc::clone(&browser), args)?;
        serde_json::to_value(reply)
            .map_err(|error| format!("could not serialize command reply: {error}"))
    })
}

/// JSON handlers for the chrome's existing command names.
///
/// Register these on the izumo builder using the same Arc passed to manage.
pub fn commands(browser: Arc<Browser>) -> [(&'static str, izumo::CommandHandler); 18] {
    [
        (
            "list_tabs",
            command_handler(&browser, |browser, _: NoArgs| Ok(list_tabs(browser))),
        ),
        (
            "active_tab_id",
            command_handler(&browser, |browser, _: NoArgs| Ok(active_tab_id(browser))),
        ),
        (
            "select_tab",
            command_handler(&browser, |browser, args: TabArgs| {
                select_tab(browser, args.id)
            }),
        ),
        (
            "open_tab",
            command_handler(&browser, |browser, args: OpenTabArgs| {
                open_tab(browser, args.url)
            }),
        ),
        (
            "open_dormant_tab",
            command_handler(&browser, |browser, args: OpenTabArgs| {
                open_dormant_tab(browser, args.url)
            }),
        ),
        (
            "close_tab",
            command_handler(&browser, |browser, args: TabArgs| {
                close_tab(browser, args.id);
                Ok(())
            }),
        ),
        (
            "navigate",
            command_handler(&browser, |browser, args: NavigateArgs| {
                Ok(navigate(browser, args.id, args.input))
            }),
        ),
        (
            "go_back",
            command_handler(&browser, |browser, args: TabArgs| {
                go_back(browser, args.id);
                Ok(())
            }),
        ),
        (
            "go_forward",
            command_handler(&browser, |browser, args: TabArgs| {
                go_forward(browser, args.id);
                Ok(())
            }),
        ),
        (
            "sleep_tab",
            command_handler(&browser, |browser, args: TabArgs| sleep_tab(browser, args.id)),
        ),
        (
            "wake_tab",
            command_handler(&browser, |browser, args: TabArgs| wake_tab(browser, args.id)),
        ),
        (
            "reload",
            command_handler(&browser, |browser, args: TabArgs| {
                reload(browser, args.id);
                Ok(())
            }),
        ),
        (
            "panel_state",
            command_handler(&browser, |browser, _: NoArgs| Ok(panel_state(browser))),
        ),
        (
            "set_panel_collapsed",
            command_handler(&browser, |browser, args: PanelCollapsedArgs| {
                set_panel_collapsed(browser, args.collapsed);
                Ok(())
            }),
        ),
        (
            "toggle_section",
            command_handler(&browser, |browser, args: SectionArgs| {
                toggle_section(browser, args.section)
            }),
        ),
        (
            "debug_log",
            command_handler(&browser, |browser, args: DebugLogArgs| {
                Ok(debug_log(browser, args.since))
            }),
        ),
        (
            "status",
            command_handler(&browser, |browser, _: NoArgs| Ok(status(browser))),
        ),
        (
            "user_agent",
            command_handler(&browser, |browser, _: NoArgs| Ok(user_agent(browser))),
        ),
        (
            "set_user_agent_spoofing",
            command_handler(&browser, |browser, args: UserAgentSpoofingArgs| {
                Ok(set_user_agent_spoofing(browser, args.spoofing))
            }),
        ),
        (
            "diagnostics",
            command_handler(&browser, |_, _: NoArgs| Ok(diagnostics())),
        ),
        (
            "set_diagnostics",
            command_handler(&browser, |_, args: DiagnosticsArgs| {
                set_diagnostics(args.inspection, args.profiling)
            }),
        ),
    ]
}

pub fn list_tabs(browser: Arc<Browser>) -> Vec<FrontendTab> {
    browser.snapshots().0
}

pub fn active_tab_id(browser: Arc<Browser>) -> u64 {
    browser.snapshots().1
}

pub fn select_tab(browser: Arc<Browser>, id: u64) -> Result<(), String> {
    let mut state = browser.0.state.lock().unwrap();
    let dormant = state.records.record(TabId(id)).is_some_and(|record| record.residency == Residency::Dormant);
    let sleeping = state.tabs.iter().find(|tab| tab.id == id).is_some_and(|tab| matches!(tab.life.phase, TabPhase::Sleeping | TabPhase::Discarding | TabPhase::RestoreFailed));
    if !state.tabs.iter().any(|tab| tab.id == id) {
        return Err(format!("unknown tab {id}"));
    }
    state.active = id;
    drop(state);
    if sleeping { let _ = browser.request_wake(id); } else if dormant { browser.schedule_current(id, false); }
    browser.emit_state();
    Ok(())
}

pub fn open_tab(browser: Arc<Browser>, url: Option<String>) -> Result<FrontendTab, String> {
    open_tab_record(browser, url, true)
}

pub fn open_dormant_tab(browser: Arc<Browser>, url: Option<String>) -> Result<FrontendTab, String> {
    open_tab_record(browser, url, false)
}

fn open_tab_record(browser: Arc<Browser>, url: Option<String>, select: bool) -> Result<FrontendTab, String> {
    let request = match url {
        Some(url) => request_from_input(&url).ok_or_else(|| format!("invalid URL {url}"))?,
        None => crate::nav::browser_request(Url::parse(NEW_TAB_URL).unwrap()),
    };
    let id = {
        let mut state = browser.0.state.lock().unwrap();
        let id = state.next_tab_id;
        state.records.admit(TabId(id)).map_err(str::to_owned)?;
        state.next_tab_id = id.checked_add(1).ok_or("tab-id-exhausted")?;
        state.tabs.push(TabState {
            id,
            mounted_generation: 0,
            mounted: None,
            history: vec![request],
            current: 0,
            title: String::new(),
            loading: false,
            outcome: PageOutcome::Blank,
            generation: 0,
            life: TabLife::default(),
        });
        if select { state.active = id; }
        id
    };
    if select { browser.schedule_current(id, false); } else { browser.emit_state(); }
    Ok(browser
        .0
        .state
        .lock()
        .unwrap()
        .tabs
        .iter()
        .find(|tab| tab.id == id)
        .unwrap()
        .snapshot())
}

fn close_tab_locked(state: &mut BrowserState, id: u64) -> bool {
    if state.tabs.len() == 1 {
        return false;
    }
    let Some(index) = state.tabs.iter().position(|tab| tab.id == id) else {
        return false;
    };
    state.tabs.remove(index);
    state.records.remove(TabId(id));
    if state.active == id {
        state.active = state.tabs[index.min(state.tabs.len() - 1)].id;
    }
    true
}

pub fn close_tab(browser: Arc<Browser>, id: u64) {
    let closed = {
        let mut state = browser.0.state.lock().unwrap();
        close_tab_locked(&mut state, id)
    };
    if closed {
        let active = browser.0.state.lock().unwrap().active;
        let _ = select_tab(Arc::clone(&browser), active);
        browser.wake_document();
    }
}

pub fn close_active_tab(browser: Arc<Browser>) {
    let closed = {
        let mut state = browser.0.state.lock().unwrap();
        let id = state.active;
        close_tab_locked(&mut state, id)
    };
    if closed {
        let active = browser.0.state.lock().unwrap().active;
        let _ = select_tab(Arc::clone(&browser), active);
        browser.wake_document();
    }
}

pub fn navigate(browser: Arc<Browser>, id: u64, input: String) -> bool {
    request_from_input(&input).is_some_and(|request| browser.navigate_request(id, request))
}

pub fn go_back(browser: Arc<Browser>, id: u64) {
    let moved = {
        let mut state = browser.0.state.lock().unwrap();
        state
            .tabs
            .iter_mut()
            .find(|tab| tab.id == id)
            .is_some_and(|tab| {
                if tab.current == 0 {
                    false
                } else {
                    tab.current -= 1;
                    tab.title.clear();
                    true
                }
            })
    };
    if moved {
        browser.schedule_browser_navigation(id, false);
    }
}

pub fn go_forward(browser: Arc<Browser>, id: u64) {
    let moved = {
        let mut state = browser.0.state.lock().unwrap();
        state
            .tabs
            .iter_mut()
            .find(|tab| tab.id == id)
            .is_some_and(|tab| {
                if tab.current + 1 >= tab.history.len() {
                    false
                } else {
                    tab.current += 1;
                    tab.title.clear();
                    true
                }
            })
    };
    if moved {
        browser.schedule_browser_navigation(id, false);
    }
}

pub fn sleep_tab(browser: Arc<Browser>, id: u64) -> Result<(), String> { browser.request_sleep(id) }

pub fn wake_tab(browser: Arc<Browser>, id: u64) -> Result<(), String> { browser.request_wake(id) }

pub fn reload(browser: Arc<Browser>, id: u64) {
    let sleeping = browser.0.state.lock().unwrap().tabs.iter().find(|tab| tab.id == id)
        .is_some_and(|tab| matches!(tab.life.phase, TabPhase::Sleeping | TabPhase::Discarding | TabPhase::RestoreFailed));
    if sleeping { let _ = browser.request_wake(id); } else { browser.schedule_browser_navigation(id, true); }
}

pub fn panel_state(browser: Arc<Browser>) -> PanelState {
    browser.snapshots().2
}

pub fn set_panel_collapsed(browser: Arc<Browser>, collapsed: bool) {
    browser.0.state.lock().unwrap().panel.collapsed = collapsed;
    browser.emit_state();
}

pub fn toggle_section(browser: Arc<Browser>, section: String) -> Result<(), String> {
    let mut state = browser.0.state.lock().unwrap();
    let value = match section.as_str() {
        "page" => &mut state.panel.sections.page,
        "history" => &mut state.panel.sections.history,
        "network" => &mut state.panel.sections.network,
        "console" => &mut state.panel.sections.console,
        "debugging" => &mut state.panel.sections.debugging,
        _ => return Err(format!("unknown panel section {section}")),
    };
    *value = !*value;
    drop(state);
    browser.emit_state();
    Ok(())
}

/// Everything the panel has not seen yet.
///
/// A pull as well as the `debug-entry` push, so a window that opens the panel
/// after a page has already loaded still sees what happened, and so a dropped
/// event cannot leave the panel permanently behind.
pub fn debug_log(browser: Arc<Browser>, since: Option<u64>) -> Vec<DebugEntry> {
    browser.0.log.lock().unwrap().since(since)
}

pub fn status(browser: Arc<Browser>) -> StatusReadout {
    browser.snapshots().3
}

/// The browser identity applied to new document and subresource requests.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserAgentState {
    spoofing: bool,
    locked: bool,
    user_agent: String,
}

#[derive(Clone, Copy, Serialize, serde::Deserialize)]
pub struct StoredUserAgent {
    #[serde(default = "default_spoofing")]
    pub spoofing: bool,
}

const fn default_spoofing() -> bool {
    true
}

impl Default for StoredUserAgent {
    fn default() -> Self {
        Self { spoofing: true }
    }
}

fn user_agent_path() -> Option<std::path::PathBuf> {
    Some(crate::cookie_store::profile_directory().join("user-agent.json"))
}

pub fn stored_user_agent() -> StoredUserAgent {
    user_agent_path()
        .and_then(|path| std::fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn store_user_agent(value: StoredUserAgent) {
    let Some(path) = user_agent_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(encoded) = serde_json::to_vec_pretty(&value) {
        let _ = std::fs::write(path, encoded);
    }
}

fn user_agent_state(browser: &Browser) -> UserAgentState {
    let spoofing = *browser.0.user_agent_spoofing.lock().unwrap();
    UserAgentState {
        spoofing,
        locked: crate::identity::environment_override_is_set(),
        user_agent: crate::identity::user_agent_for_spoofing(spoofing),
    }
}

pub fn user_agent(browser: Arc<Browser>) -> UserAgentState {
    user_agent_state(&browser)
}

/// Apply the compatibility identity immediately and reload the active tab.
///
/// The network provider owns its cache and User-Agent, so changing identity
/// replaces that provider while retaining the profile cookie jar. The current
/// document keeps the provider it was created with; reloading makes the
/// visible page and every subresource agree on the new identity.
pub fn set_user_agent_spoofing(browser: Arc<Browser>, spoofing: bool) -> UserAgentState {
    if crate::identity::environment_override_is_set() {
        return user_agent_state(&browser);
    }
    *browser.0.user_agent_spoofing.lock().unwrap() = spoofing;
    let cookies = browser.net().cookie_store();
    *browser.0.net.lock().unwrap() = Arc::new(NetProvider::with_user_agent_and_cookies(
        None,
        &crate::identity::user_agent_for_spoofing(spoofing),
        cookies,
    ));
    store_user_agent(StoredUserAgent { spoofing });
    let active = browser.snapshots().1;
    browser.schedule_current(active, true);
    user_agent_state(&browser)
}

/// What the diagnostics switches are actually doing right now.
///
/// Read back from the runtime rather than from anything this process
/// remembers. The runtime refuses deep profiling while inspection is off, so a
/// window that reported the requested pair could show profiling on while
/// nothing was being collected.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsState {
    /// The local inspection and agent-control socket is listening.
    inspection: bool,
    /// Intrusive performance collection is running.
    profiling: bool,
    /// `CHUZZ_CONTROL` was set, so the environment decides and the window
    /// must not pretend otherwise.
    locked: bool,
}

/// Whether the environment forced the diagnostics plane on for this run.
///
/// Kept as an override rather than replaced by the setting: scripts and the
/// acceptance run start the browser with `CHUZZ_CONTROL=1` and must get a
/// socket regardless of what a previous session happened to store.
pub fn diagnostics_locked() -> bool {
    matches!(
        std::env::var("CHUZZ_CONTROL").ok().as_deref(),
        Some("1") | Some("true") | Some("on")
    )
}

fn diagnostics_now() -> DiagnosticsState {
    DiagnosticsState {
        inspection: izumo::agent_control_enabled(),
        profiling: izumo::deep_profiling_enabled(),
        locked: diagnostics_locked(),
    }
}

/// Where the diagnostics choice is kept between runs.
///
/// Rust rather than the window's `localStorage`: that is an in-memory shim in
/// the Blitz document, so a setting stored there is forgotten on quit. A switch
/// that silently forgets itself is worse than no switch, because the answer to
/// "is the socket open?" would then depend on something the window cannot see.
fn diagnostics_path() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(
        std::path::PathBuf::from(home)
            .join("Library/Application Support/ai.chuzz.browser")
            .join("diagnostics.json"),
    )
}

#[derive(Clone, Copy, Default, Serialize, serde::Deserialize)]
pub struct StoredDiagnostics {
    #[serde(default)]
    pub inspection: bool,
    #[serde(default)]
    pub profiling: bool,
}

/// What was chosen last time. Absent, unreadable, or malformed all mean "off",
/// which is the safe answer for a switch that opens a socket.
pub fn stored_diagnostics() -> StoredDiagnostics {
    diagnostics_path()
        .and_then(|path| std::fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn store_diagnostics(value: StoredDiagnostics) {
    let Some(path) = diagnostics_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(encoded) = serde_json::to_vec_pretty(&value) {
        let _ = std::fs::write(path, encoded);
    }
}

pub fn diagnostics() -> DiagnosticsState {
    diagnostics_now()
}

/// Turn the inspection plane and deep profiling on or off, now.
///
/// Both are live switches: enabling inspection binds the control socket and
/// disabling it unbinds and removes the descriptor, so turning this off in
/// Settings actually closes the door rather than hiding the button for it.
pub fn set_diagnostics(inspection: bool, profiling: bool) -> Result<DiagnosticsState, String> {
    if diagnostics_locked() {
        // Report the truth instead of failing: the switch is simply not the
        // authority this run, and the window shows it as held by the
        // environment.
        return Ok(diagnostics_now());
    }
    izumo::apply_runtime_debug_options(izumo::RuntimeDebugOptions {
        inspection_and_agent_control: inspection,
        deep_intrusive_profiling: profiling,
    })
    .map_err(|error| format!("could not apply the diagnostics settings: {error}"))?;
    // Written after the runtime accepted it, so a refused change is not
    // remembered as though it had taken.
    store_diagnostics(StoredDiagnostics {
        inspection,
        profiling,
    });
    Ok(diagnostics_now())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The browser's own pages have to be legible on their own.
    ///
    /// They declare no colours, so they took the engine's default black text
    /// and transparent background, over a viewport the shell paints with the
    /// dark theme surface. `view-source:` produced a document that was fetched,
    /// parsed, laid out and painted, and looked like a black rectangle. It was
    /// reported as the source not showing, which is exactly what it looked
    /// like.
    #[test]
    fn the_pages_the_browser_writes_carry_their_own_colours() {
        for html in [
            error_html("something went wrong"),
            source_html("<b>hi</b>"),
            EMPTY_HTML.to_owned(),
        ] {
            assert!(
                html.contains("background:#f6f6f7") && html.contains("color:#16181d"),
                "an internal page with no colours of its own is invisible on a dark \
                 viewport: {html}"
            );
        }
    }

    /// Source is shown, never re-serialised.
    #[test]
    fn source_is_escaped_rather_than_rendered() {
        let html = source_html("<script>alert(1)</script>& <b>");
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;&amp; &lt;b&gt;"));
        assert!(
            !html.contains("<script>alert"),
            "view source must not be able to run what it is showing"
        );
    }

    /// The five names the tab indicator draws, and the order that decides
    /// which one a page with several problems gets.
    ///
    /// The names are a contract with `types/index.ts`: the dot's colour is
    /// looked up by this string, and a rename on either side turns every tab
    /// grey with nothing to say it happened.
    #[test]
    fn an_outcome_reports_the_worst_thing_that_happened() {
        assert_eq!(PageOutcome::Blank.name(), "blank");
        assert_eq!(PageOutcome::Ready.name(), "ready");
        assert_eq!(PageOutcome::Warning.name(), "warning");
        assert_eq!(PageOutcome::Error.name(), "error");

        // `max` is how a load accumulates: the document arriving sets the
        // floor at Ready, and each thing that failed after that can only raise
        // it. A page whose scripts 404'd must not report Ready because the
        // HTML was fine.
        assert_eq!(
            PageOutcome::Ready.max(PageOutcome::Warning),
            PageOutcome::Warning
        );
        assert_eq!(
            PageOutcome::Warning.max(PageOutcome::Ready),
            PageOutcome::Warning
        );
        assert_eq!(
            PageOutcome::Warning.max(PageOutcome::Error),
            PageOutcome::Error
        );
        assert!(
            PageOutcome::Blank < PageOutcome::Ready,
            "an untouched tab must not outrank a loaded one, or a new tab \
             would go green"
        );
    }

    /// A tab that is loading says so, whatever it used to be.
    ///
    /// Without this, navigating from a working page to a broken one shows the
    /// old page's green against the new page's address for the whole fetch,
    /// which is the one moment the indicator is being watched.
    #[test]
    fn loading_outranks_the_previous_outcome() {
        let mut tab = TabState {
            id: 0,
            mounted_generation: 0,
            mounted: None,
            history: vec![Request::get(Url::parse("https://example.com/").unwrap())],
            current: 0,
            title: String::new(),
            loading: true,
            outcome: PageOutcome::Ready,
            generation: 0,
        };
        assert_eq!(tab.snapshot().status, "loading");
        tab.loading = false;
        assert_eq!(tab.snapshot().status, "ready");
        tab.outcome = PageOutcome::Error;
        assert_eq!(tab.snapshot().status, "error");
    }

    /// Anything but an explicit `true` has to mean "closed".
    ///
    /// This switch opens a socket that lets any local process drive the window,
    /// so every way of failing to read the setting — truncated file, an older
    /// shape, a hand-edit — must land on off. A `Default` that fell the other
    /// way would open the socket on exactly the runs where something already
    /// went wrong.
    #[test]
    fn an_unreadable_diagnostics_record_means_off() {
        for raw in [
            "{}",
            r#"{"profiling":true}"#,
            r#"{"inspection":null}"#,
            r#"{"inspection":"true"}"#,
            "",
            "not json at all",
        ] {
            let stored: StoredDiagnostics = serde_json::from_str(raw).unwrap_or_default();
            assert!(
                !stored.inspection,
                "{raw:?} must not open the inspection socket"
            );
        }
        let stored: StoredDiagnostics = serde_json::from_str(r#"{"inspection":true}"#).unwrap();
        assert!(stored.inspection, "an explicit true is still honoured");
    }

    /// A reload has to ask the network, not the cache.
    ///
    /// This replaces a test that lived on the old loader, where a wrapping
    /// provider stamped `no-cache` on *every* request a reloaded document made.
    /// That guarantee is gone: `fetch_page` stamps the document request and each
    /// prefetched script, and anything the document goes on to fetch for itself
    /// once attached — stylesheets, images, fonts — is served under the ordinary
    /// policy. Reloading a page whose HTML is unchanged but whose CSS moved will
    /// still show the old CSS. Worth knowing before someone debugs it twice.
    #[test]
    fn a_reload_asks_the_network_for_the_document() {
        let mut request = Request::get(Url::parse("https://example.com/").unwrap());
        assert!(
            request
                .headers
                .get(blitz_traits::net::http::header::CACHE_CONTROL)
                .is_none(),
            "an ordinary navigation carries no cache directive"
        );

        revalidate(&mut request);

        assert_eq!(
            request
                .headers
                .get(blitz_traits::net::http::header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok()),
            Some("no-cache")
        );
    }

    /// The fallback is the whole degradation story, so each way of failing has
    /// to leave it standing. A partial mount, or an emptied mount after a
    /// failed instantiate, turns a working static page into a blank one.
    #[test]
    fn a_failed_module_leaves_the_fallback_standing() {
        const HTML: &str = r#"<!doctype html><html><body>
            <div id="root"><p id="fallback">fallback</p></div>
            </body></html>"#;

        let config = || DocumentConfig {
            html_parser_provider: Some(Arc::new(HtmlProvider)),
            ..Default::default()
        };
        let good = std::fs::read(crate::wasm_page::fixture_module()).expect("fixture module");

        let cases: Vec<(&str, PageModule)> = vec![
            (
                "a selector matching nothing",
                PageModule {
                    bytes: good.clone(),
                    selector: "#nowhere".to_owned(),
                },
            ),
            (
                "bytes that are not a module",
                PageModule {
                    bytes: b"<!doctype html>".to_vec(),
                    selector: "#root".to_owned(),
                },
            ),
            (
                "a module whose entry export is missing",
                PageModule {
                    // Valid wasm, no `run` export: instantiates, then fails.
                    bytes: wat::parse_str("(module (memory (export \"memory\") 1))").unwrap(),
                    selector: "#root".to_owned(),
                },
            ),
        ];

        for (what, module) in cases {
            assert!(
                mount_page_module(HTML, &module, config()).is_none(),
                "{what} should not mount"
            );
        }

        // And the success case does replace it, so the assertions above are not
        // passing because nothing ever mounts.
        let mounted = mount_page_module(
            HTML,
            &PageModule {
                bytes: good,
                selector: "#root".to_owned(),
            },
            config(),
        )
        .expect("the fixture module should mount");
        let mount = mounted.query_selector("#root").unwrap().unwrap();
        let children = mounted.get_node(mount).unwrap().children.to_vec();
        assert!(!children.is_empty(), "the guest built nothing");
        assert!(
            mounted.query_selector("#fallback").unwrap().is_none(),
            "the fallback survived a successful mount"
        );
        assert!(
            mounted.query_selector(".panel").unwrap().is_some(),
            "the guest's tree is not in the document"
        );
    }

    /// What you read has to be what arrived. For a page whose whole claim is
    /// "there is no script here", a source view that re-serialised a parsed
    /// tree would be answering a different question.
    #[test]
    fn view_source_shows_the_bytes_verbatim() {
        let page = r##"<div id="root"><p>fallback</p></div>
<script type="application/wasm" src="/demo.wasm" mount="#root"></script>"##;
        let shown = source_html(page);

        // The tag survives, escaped, so a reader can see there is exactly one
        // script and what type it is.
        assert!(shown.contains("&lt;script type=\"application/wasm\""));
        assert!(shown.contains("mount=\"#root\"&gt;"));

        // Escaped, not executed or parsed away.
        assert!(
            !shown.contains("<div id=\"root\">"),
            "the source was interpolated as markup instead of escaped"
        );
        assert!(shown.contains("&lt;div id=\"root\"&gt;"));

        // Ampersands first, or `&lt;` would come back out as `&amp;lt;`.
        assert_eq!(
            source_html("a & b < c").matches("&amp;").count(),
            1,
            "escaping order mangles ampersands"
        );
    }

    #[test]
    fn browser_starts_with_one_blank_tab() {
        let browser = Browser::new(None);
        let (tabs, active, _, status) = browser.snapshots();
        assert_eq!(tabs.len(), 1);
        assert_eq!(active, 0);
        assert_eq!(status.url, NEW_TAB_URL);
    }

    /// `--wasm` puts a guest-built document on the real tab mount.
    ///
    /// The window cannot be screenshotted on every machine, and "the process
    /// stayed alive" is not evidence that anything rendered. This drives the
    /// actual path instead — `schedule_current`, the bundle queue,
    /// `poll_document`, `page_node`, `set_sub_document` — against the real
    /// chrome document, and then reads the page back out of the mount.
    ///
    /// It asserts through `page_node` rather than a CSS query for the same
    /// reason the frontend test does: that lookup is what decides whether a
    /// page is ever attached, and it is the thing that regressed once.
    #[test]
    fn a_wasm_page_is_attached_to_the_tab_mount() {
        // Boa needs the stack, the same as the frontend test.
        // owner ruling: typed Arc<T> hand-off, concurrency only via nagoya/ps-st3; owner exception: test-only big stack.
        #[allow(clippy::disallowed_methods)]
        std::thread::Builder::new()
            .stack_size(16 * 1024 * 1024)
            .spawn(|| {
                let module = crate::wasm_page::fixture_module();
                let browser = Browser::with_wasm_page(module).expect("the fixture module exists");

                let mut chrome = crate::frontend::document(browser.clone(), "chuzz://ui/").unwrap();
                chrome.set_ipc_handler(|_| {});
                chrome.eval(crate::frontend::TAURI_TEST_BRIDGE);
                chrome.execute_scripts();
                for _ in 0..64 {
                    chrome.poll(None);
                }

                // No app handle in a test, so `emit_state` and `wake_document`
                // return early; the wasm branch still queues the bundle, which
                // is the part under test.
                browser.schedule_current(0, false);
                let mut pending = VecDeque::new();
                assert!(
                    browser.poll_document(&mut chrome, &mut pending),
                    "the bundle should have been attached, not retained"
                );

                let mut inner = chrome.inner_mut();
                inner.set_viewport(blitz_traits::shell::Viewport::new(
                    1440,
                    960,
                    1.0,
                    blitz_traits::shell::ColorScheme::Dark,
                ));
                inner.resolve(0.0);

                let mount = page_node(&inner, 0).expect("page mount");
                let page = inner
                    .get_node(mount)
                    .and_then(|node| node.element_data())
                    .and_then(|element| element.sub_doc_data())
                    .expect("a document should be attached to the tab mount");

                // The guest's tree, read out of the document the window holds.
                let page = page.inner();
                let panel = page
                    .query_selector(".panel")
                    .ok()
                    .flatten()
                    .expect("the guest's panel should be in the attached document");
                let rows = page.query_selector_all(".row").unwrap();
                assert_eq!(rows.len(), 3, "the guest's three rows should be there");

                // Attached is not the same as rendered. Without a box the
                // document would be correct and the window would look empty,
                // which is the failure the frontend test exists to catch on the
                // mount and this one catches on the page.
                let layout = page.get_node(panel).unwrap().final_layout();
                assert!(
                    layout.size.width > 0.0 && layout.size.height > 0.0,
                    "the guest's panel has no box: {:?}",
                    layout.size
                );
            })
            .unwrap()
            .join()
            .unwrap();
    }
}
