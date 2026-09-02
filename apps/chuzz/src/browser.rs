use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, Weak};

use blitz_dom::{Document as _, DocumentConfig, FontContext, NodeId};
use blitz_html::HtmlProvider;
use blitz_traits::navigation::{NavigationOptions, NavigationProvider};
use blitz_traits::net::{Request, Url};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_runtime_blitz::BlitzRuntime;

use crate::decode::decode_body;
use crate::document_loader::{NetProvider, WEB_API_SHIM};
use crate::nav::{NEW_TAB_URL, display_title, request_from_input};

/// The document an empty tab shows.
///
/// Transparent on purpose. It used to hardcode `background:#0f1622`, a navy
/// that no theme could reach — chuzz derives every surface from
/// `--az-surface`, which the user changes with the surface wheel, so a blank
/// tab stayed blue whatever they picked. The shell paints `.page` with the
/// themed surface and this lets it through.
const BLANK_HTML: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title></title></head><body style="margin:0;background:transparent"></body></html>"#;
/// Colours for the pages the browser writes itself.
///
/// Explicit, and light, like every other browser's error and source pages.
/// These documents declare no colours of their own, so they inherited the
/// engine's defaults: black text on a transparent background, over a viewport
/// the shell paints with the dark theme surface. The source of a page was
/// therefore rendered, laid out, and unreadable, which is indistinguishable
/// from not being rendered at all and was reported as exactly that.
///
/// Not a theme token. These are documents in a page viewport, not part of the
/// chrome, and nothing in a page can reach the shell's custom properties.
const INTERNAL_PAGE_STYLE: &str = "margin:0;background:#f6f6f7;color:#16181d";

const EMPTY_HTML: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>Empty response</title></head><body style="margin:0;background:#f6f6f7;color:#16181d"><h1>Empty response</h1><p>The server returned no content.</p></body></html>"#;

fn error_html(error: &str) -> String {
    let escaped = error
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><title>Page not available</title></head><body style="{INTERNAL_PAGE_STYLE};padding:2rem;font:14px system-ui,sans-serif"><h1>Page not available</h1><p>{escaped}</p></body></html>"#
    )
}

pub type ChuzzAppHandle = AppHandle<BlitzRuntime>;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FrontendTab {
    id: u64,
    title: String,
    url: String,
    status: &'static str,
    can_go_back: bool,
    can_go_forward: bool,
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
}

struct TabState {
    id: u64,
    history: Vec<Request>,
    current: usize,
    title: String,
    loading: bool,
    /// How the last completed load went. `loading` wins over it while a load
    /// is in flight, so the previous page's outcome never shows against the
    /// new one's address.
    outcome: PageOutcome,
    generation: u64,
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
    /// Which part of the browser said it: `net`, `page`, `script`, `wasm`.
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
    active: u64,
    next_tab_id: u64,
    panel: PanelState,
}

struct PageBundle {
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
///
/// An enum rather than an optional module path beside the HTML, so the two
/// cannot both be set or both be missing. A wasm page has no HTML and no
/// scripts, and an HTML page has no module; there is no state in between.
enum PageSource {
    Html {
        html: String,
        scripts: HashMap<Url, String>,
        /// A module the page asked for with `<script type="application/wasm">`,
        /// already fetched and vetted. Fetching happens with the rest of the
        /// page because only that half is async; mounting is done later,
        /// against the parsed document.
        wasm: Option<PageModule>,
    },
    /// Built by a WebAssembly guest. Nothing is fetched and no JavaScript runs.
    Wasm { module: std::path::PathBuf },
    /// The bytes a server sent, shown rather than rendered.
    Source { text: String },
}

/// A module a page declared, fetched alongside it.
struct PageModule {
    bytes: Vec<u8>,
    /// The `mount` selector, resolved against the parsed document later.
    selector: String,
}

/// A page served from a module on disk rather than from the network.
///
/// The URL is kept alongside the path because a tab is addressed by URL
/// everywhere else in the browser: history, the address bar and the back and
/// forward stack all hold `Request`s, and giving the wasm tab a `file:` URL
/// lets it sit in those unchanged. Navigating away from it is then an ordinary
/// navigation, and coming back re-runs the guest.
///
/// Not behind `feature = "wasm"`: a path and a URL need no wasm runtime. Only
/// running the guest does, so that is the only thing gated, and a build without
/// the feature reports the fact on the page instead of silently lacking a flag.
struct WasmPage {
    url: Url,
    module: std::path::PathBuf,
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
    net: Arc<NetProvider>,
    completed: Mutex<VecDeque<PageBundle>>,
    app: Mutex<Option<ChuzzAppHandle>>,
    /// Set by `--wasm`. Immutable for the life of the process.
    wasm: Option<WasmPage>,
}

#[derive(Clone)]
pub struct Browser(Arc<BrowserInner>);

impl Browser {
    pub fn new(startup: Option<Url>) -> Self {
        Self::build(startup, None)
    }

    /// A browser whose first tab is built by the guest in `module` rather than
    /// fetched.
    ///
    /// The tab is an ordinary tab addressed by an ordinary `file:` URL. Only
    /// the source of its document differs, which is why nothing downstream of
    /// the mount — the tab strip, the toolbar, history, layout, paint — needs
    /// to know this flag exists.
    pub fn with_wasm_page(module: std::path::PathBuf) -> Result<Self, String> {
        let module = std::fs::canonicalize(&module)
            .map_err(|error| format!("--wasm: cannot read {}: {error}", module.display()))?;
        let url = Url::from_file_path(&module)
            .map_err(|()| format!("--wasm: {} is not an absolute path", module.display()))?;
        Ok(Self::build(
            Some(url.clone()),
            Some(WasmPage { url, module }),
        ))
    }

    fn build(startup: Option<Url>, wasm: Option<WasmPage>) -> Self {
        let first = startup.unwrap_or_else(|| Url::parse(NEW_TAB_URL).unwrap());
        Self(Arc::new(BrowserInner {
            state: Mutex::new(BrowserState {
                tabs: vec![TabState {
                    id: 0,
                    history: vec![Request::get(first)],
                    current: 0,
                    title: String::new(),
                    loading: false,
                    outcome: PageOutcome::Blank,
                    generation: 0,
                }],
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
            net: Arc::new(NetProvider::with_user_agent(
                None,
                &crate::identity::user_agent_from_env(),
            )),
            completed: Mutex::new(VecDeque::new()),
            app: Mutex::new(None),
            wasm,
        }))
    }

    pub fn attach_app(&self, app: ChuzzAppHandle) {
        *self.0.app.lock().unwrap() = Some(app);
        self.schedule_current(0, false);
    }

    pub fn install_document_lifecycle(&self, document: &mut blitz_script::ScriptDocument) {
        let browser = self.clone();
        let mut pending = VecDeque::new();
        document.add_poll_hook(move |document, _| browser.poll_document(document, &mut pending));
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
        if let Some(window) = self.app().and_then(|app| app.get_webview_window("main")) {
            let _ = window.eval("void 0");
        }
    }

    fn schedule_current(&self, tab_id: u64, revalidate: bool) {
        let (request, generation) = {
            let mut state = self.0.state.lock().unwrap();
            let Some(tab) = state.tabs.iter_mut().find(|tab| tab.id == tab_id) else {
                return;
            };
            tab.generation += 1;
            tab.loading = true;
            (tab.request().clone(), tab.generation)
        };
        self.emit_state();

        // A wasm page has nothing to fetch, so its bundle is complete the
        // moment it is made and never goes near the network or the runtime.
        // Matched on the URL rather than the tab id, so navigating this tab
        // elsewhere is an ordinary navigation and coming back re-runs the guest.
        if let Some(page) = self.0.wasm.as_ref().filter(|page| page.url == request.url) {
            self.0.completed.lock().unwrap().push_back(PageBundle {
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
        tauri::async_runtime::spawn(async move {
            let bundle = fetch_page(&browser, tab_id, generation, request, revalidate).await;
            browser.0.completed.lock().unwrap().push_back(bundle);
            browser.wake_document();
        });
    }

    fn navigate_request(&self, tab_id: u64, request: Request) -> bool {
        {
            let mut state = self.0.state.lock().unwrap();
            let Some(tab) = state.tabs.iter_mut().find(|tab| tab.id == tab_id) else {
                return false;
            };
            tab.history.truncate(tab.current + 1);
            tab.history.push(request);
            tab.current += 1;
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
        let mut changed = false;

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

            let shell_provider = ui.inner().shell_provider.clone();
            // A factory rather than a value: a module that fails to mount is
            // re-parsed from the same bytes, and that second parse needs its own
            // config because `DocumentConfig` is consumed by the first.
            let make_config = || DocumentConfig {
                base_url: Some(bundle.resolved_url.clone()),
                net_provider: Some(Arc::clone(&self.0.net) as _),
                navigation_provider: Some(Arc::new(PageNavigation {
                    browser: Arc::downgrade(&self.0),
                    tab_id: bundle.tab_id,
                })),
                shell_provider: Some(shell_provider.clone()),
                html_parser_provider: Some(Arc::new(HtmlProvider)),
                font_ctx: Some(FontContext::default()),
                ..Default::default()
            };
            // The two document sources meet here and nowhere else. Both end as
            // a `Box<dyn Document>` on the same `chuzz-page-{tab_id}` mount, so
            // everything downstream — layout, paint, the tab strip, the toolbar
            // — cannot tell them apart and did not have to change.
            // What the fetch concluded, which mounting can only make worse.
            let mut outcome = bundle.outcome;
            let (page, title): (Box<dyn blitz_dom::Document>, String) = match bundle.source {
                // A page that declared a module gets it mounted here, against
                // the document as parsed. Everything up to this point is an
                // ordinary page load.
                PageSource::Html {
                    html,
                    scripts,
                    wasm: Some(module),
                } => match mount_page_module(&html, &module, make_config()) {
                    Some(page) => {
                        self.note(
                            "info",
                            "wasm",
                            format!(
                                "guest mounted on {:?}, {} nodes in the document",
                                module.selector,
                                page.tree().len()
                            ),
                        );
                        (Box::new(page), String::new())
                    }
                    None => {
                        // The module fetched and vetted, then would not run.
                        // The page still shows its fallback, so this is the
                        // same "arrived, but not what was asked for" reading as
                        // a script that never came.
                        outcome = outcome.max(PageOutcome::Warning);
                        self.note(
                            "warn",
                            "wasm",
                            format!(
                                "the guest did not mount on {:?}; keeping the page's fallback",
                                module.selector
                            ),
                        );
                        // Re-parsed rather than repaired. The guest appends into
                        // the mount as it builds, so a run that fails part way
                        // leaves a half-built tree next to the fallback, and
                        // there is no way to tell the two apart afterwards.
                        // Parsing the same bytes again is the only way to be
                        // certain the document is exactly what the page said.
                        let mut page =
                            blitz_script::ScriptDocument::from_html(&html, make_config())
                                .with_fetcher(crate::script_fetch::PageScripts::new(
                                    scripts,
                                    Arc::clone(&self.0.net),
                                    WINDOW_SCRIPT_DEADLINE,
                                ));
                        page.eval(WEB_API_SHIM);
                        crate::net_bridge::install(
                            &mut page,
                            Arc::clone(&self.0.net),
                            WINDOW_NETWORK_DEADLINE,
                        );
                        page.execute_scripts();
                        let title = page
                            .inner()
                            .find_title_node()
                            .map(|node| node.text_content())
                            .unwrap_or_default();
                        (Box::new(page), title)
                    }
                },
                PageSource::Html {
                    html,
                    scripts,
                    wasm: None,
                } => {
                    let mut page = blitz_script::ScriptDocument::from_html(&html, make_config())
                        .with_fetcher(crate::script_fetch::PageScripts::new(
                            scripts,
                            Arc::clone(&self.0.net),
                            WINDOW_SCRIPT_DEADLINE,
                        ));
                    page.eval(WEB_API_SHIM);
                    crate::net_bridge::install(
                        &mut page,
                        Arc::clone(&self.0.net),
                        WINDOW_NETWORK_DEADLINE,
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
                PageSource::Wasm { module } => match build_wasm_page(&module, make_config()) {
                    // A bare `BaseDocument`, not a `ScriptDocument`. The mount
                    // takes `Box<dyn Document>` and `BaseDocument` implements
                    // it, so no wrapper is needed and no JavaScript runtime is
                    // attached to a page that has no scripts to run.
                    Ok(page) => (Box::new(page), module_title(&module)),
                    Err(error) => {
                        eprintln!("chuzz: --wasm: {error}");
                        outcome = PageOutcome::Error;
                        let page = blitz_script::ScriptDocument::from_html(
                            &error_html(&error),
                            DocumentConfig::default(),
                        );
                        (Box::new(page), module_title(&module))
                    }
                },
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
            ui.inner_mut().set_sub_document(target, page);

            {
                let mut state = self.0.state.lock().unwrap();
                if let Some(tab) = state.tabs.iter_mut().find(|tab| tab.id == bundle.tab_id) {
                    if let Ok(url) = Url::parse(&bundle.resolved_url) {
                        tab.history[tab.current].url = url;
                    }
                    tab.title = title;
                    tab.loading = false;
                    tab.outcome = outcome;
                }
            }
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
}

impl NavigationProvider for PageNavigation {
    fn navigate_to(&self, options: NavigationOptions) {
        if let Some(inner) = self.browser.upgrade() {
            Browser(inner).navigate_request(self.tab_id, options.into_request());
        }
    }
}

/// Build a page document by letting a WebAssembly guest construct it.
///
/// The config is the page config the HTML path uses, so the guest's document
/// gets the same shell provider and the same navigation provider and sits in
/// the window on the same terms. It gets no html parser and no script runtime
/// because it needs neither.
#[cfg(feature = "wasm")]
fn build_wasm_page(
    module: &std::path::Path,
    config: DocumentConfig,
) -> Result<blitz_dom::BaseDocument, String> {
    let (document, mount) = crate::wasm_page::empty_document(config);
    crate::wasm_page::run_guest(module, document, mount).map_err(|error| error.to_string())
}

/// Without the `wasm` feature there is no interpreter to run the guest, so the
/// page says so rather than coming up blank.
#[cfg(not(feature = "wasm"))]
fn build_wasm_page(
    module: &std::path::Path,
    _config: DocumentConfig,
) -> Result<blitz_dom::BaseDocument, String> {
    Err(format!(
        "this build has no wasm support, so {} cannot be run. \
         Rebuild with the `wasm` feature, which is on by default.",
        module.display()
    ))
}

/// A wasm page has no `<title>`, so the module's file name stands in.
fn module_title(module: &std::path::Path) -> String {
    module
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| module.display().to_string())
}

/// Parse the page, then let the guest take over its mount element.
///
/// Returns `None` on every failure, and the caller then re-parses the same
/// bytes. Nothing here repairs a document: a guest appends into the mount as it
/// builds, so a run that fails part way leaves its own half-built tree beside
/// the fallback with no way to tell them apart.
///
/// The fallback is removed only after the entry export has returned `OK`, which
/// is the whole point. Emptying first and failing after turns a working static
/// page into a blank one.
#[cfg(feature = "wasm")]
fn mount_page_module(
    html: &str,
    module: &PageModule,
    config: DocumentConfig,
) -> Option<blitz_dom::BaseDocument> {
    // Parsed without a script runtime. A page that hands its interface to a
    // guest has no JavaScript in the path by construction, and attaching an
    // engine to run none would be dead weight.
    let document = blitz_html::HtmlDocument::from_html(html, config).into_inner();

    let mount = match document.query_selector(&module.selector) {
        Ok(Some(node)) => node,
        Ok(None) => {
            eprintln!(
                "chuzz: wasm: mount selector {:?} matched nothing; keeping the page as parsed",
                module.selector
            );
            return None;
        }
        Err(error) => {
            eprintln!(
                "chuzz: wasm: mount selector {:?} is not valid: {error:?}",
                module.selector
            );
            return None;
        }
    };

    // What the fallback consists of, recorded before the guest adds anything.
    let fallback: Vec<NodeId> = document.get_node(mount)?.children.to_vec();

    let mut document = match crate::wasm_page::run_guest_bytes(&module.bytes, document, mount) {
        Ok(document) => document,
        Err(error) => {
            eprintln!("chuzz: wasm: {error}");
            return None;
        }
    };

    // Only now. Everything above can fail, and every one of those failures
    // leaves the document with its fallback intact.
    let mut changes = document.mutate();
    let replaced = fallback.len();
    for node in fallback {
        changes.remove_and_drop_node(node);
    }
    drop(changes);
    // Said out loud, because the alternative is indistinguishable from a page
    // that never declared a module: both are silent and both render something.
    eprintln!(
        "chuzz: wasm: mounted a guest on {:?}, replacing {replaced} fallback node(s)",
        module.selector
    );
    Some(document)
}

/// Without the `wasm` feature there is no interpreter, so the page keeps its
/// fallback and says why.
#[cfg(not(feature = "wasm"))]
fn mount_page_module(
    _html: &str,
    _module: &PageModule,
    _config: DocumentConfig,
) -> Option<blitz_dom::BaseDocument> {
    eprintln!("chuzz: wasm: this build has no wasm support; keeping the page as parsed");
    None
}

/// Fetch the module a page's wasm script tag names, and vet it.
///
/// Returns `None` on every failure, having said why. `None` means the page
/// keeps the fallback content inside its mount element, which is the entire
/// degradation story: a browser that cannot run the module shows the static
/// document instead of a broken one.
async fn fetch_page_module(
    browser: &Browser,
    document_url: &str,
    script: &crate::wasm_page::WasmScript,
) -> Option<PageModule> {
    let net = &browser.0.net;
    let base = Url::parse(document_url).ok()?;
    let src = match base.join(&script.src) {
        Ok(src) => src,
        Err(error) => {
            browser.note(
                "error",
                "wasm",
                format!("cannot resolve src {:?}: {error}", script.src),
            );
            return None;
        }
    };

    // Same origin only. The module runs against the host ABI with the whole
    // document reachable, which is a materially different trust decision from a
    // local file named on the command line. Cross-origin loading needs a
    // deliberate policy, not a default.
    if src.origin() != base.origin() {
        eprintln!(
            "chuzz: wasm: refusing to load {src} into {base}: a module must come from \
             the page's own origin"
        );
        return None;
    }

    let bytes = match net.fetch_async(Request::get(src.clone())).await {
        Ok((_, bytes)) => bytes,
        Err(error) => {
            // Covers a non-2xx status: the provider turns that into
            // `ProviderError::HttpStatus` rather than handing back a body.
            browser.note("error", "wasm", format!("could not fetch {src}: {error:?}"));
            return None;
        }
    };

    if let Err(problem) = crate::wasm_page::validate_module(&bytes) {
        browser.note("error", "wasm", format!("{src}: {problem}"));
        return None;
    }

    browser.note(
        "info",
        "wasm",
        format!("module {src} validated, {} bytes", bytes.len()),
    );
    Some(PageModule {
        bytes: bytes.to_vec(),
        selector: script.mount.clone(),
    })
}

/// Wrap a server's bytes in the smallest document that shows them verbatim.
///
/// Escaped and put in a `<pre>`, which is the whole job: the point of view
/// source is that what you read is what arrived, so nothing here may reformat,
/// pretty-print or re-serialise it. A document that showed a parsed and
/// re-emitted tree would be answering a different question, and for a page
/// whose claim is "there is no script here" it would be the wrong answer.
pub(crate) fn source_html(text: &str) -> String {
    let escaped = text
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><title>Source</title></head>
<body style="{INTERNAL_PAGE_STYLE}"><pre style="margin:0;padding:1rem;font:13px ui-monospace,monospace;white-space:pre-wrap;word-break:break-word">{escaped}</pre></body></html>"#
    )
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
    let net = &browser.0.net;
    browser.note("info", "nav", format!("navigating to {}", request.url));
    if request.url.scheme() == "about" {
        return PageBundle {
            tab_id,
            generation,
            resolved_url: request.url.to_string(),
            source: PageSource::Html {
                html: BLANK_HTML.to_owned(),
                scripts: HashMap::new(),
                wasm: None,
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
            Ok(url) => match net.fetch_async(Request::get(url)).await {
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
                    (
                        format!("could not fetch {inner}: {error:?}"),
                        PageOutcome::Error,
                    )
                }
            },
            Err(error) => (format!("{inner} is not a URL: {error}"), PageOutcome::Error),
        };
        return PageBundle {
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

    let (resolved_url, bytes) = match net.fetch_async(request).await {
        Ok(response) => response,
        Err(error) => {
            browser.note(
                "error",
                "net",
                format!("{requested_url} did not load: {error:?}"),
            );
            return PageBundle {
                tab_id,
                generation,
                resolved_url: requested_url,
                source: PageSource::Html {
                    html: error_html(&format!("{error:?}")),
                    scripts: HashMap::new(),
                    wasm: None,
                },
                outcome: PageOutcome::Error,
            };
        }
    };
    // Worst-wins from here: the document arrived, so the floor is Ready, and
    // each thing the page named that did not arrive can only raise it.
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
    // One parse serves both: the external script urls and the wasm script tag.
    // Parsing twice would be wasteful and, worse, could disagree.
    let (urls, wasm_script) = {
        let document = blitz_script::ScriptDocument::from_html(
            &html,
            DocumentConfig {
                base_url: Some(resolved_url.clone()),
                ..Default::default()
            },
        );
        let wasm_script = crate::wasm_page::find_wasm_script(&document.inner());
        (document.external_script_urls(), wasm_script)
    };
    let wasm = match wasm_script {
        Some(script) => {
            browser.note(
                "info",
                "wasm",
                format!(
                    "page declares a module: {} mounting on {}",
                    script.src, script.mount
                ),
            );
            let module = fetch_page_module(browser, &resolved_url, &script).await;
            if module.is_none() {
                // The page declared a module and it did not load. The fallback
                // inside the mount is shown instead, which is the designed
                // degradation and still not what the page asked for.
                outcome = outcome.max(PageOutcome::Warning);
            }
            module
        }
        None => None,
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
        tab_id,
        generation,
        resolved_url,
        source: PageSource::Html {
            html,
            scripts,
            wasm,
        },
        outcome,
    }
}

#[tauri::command]
pub fn list_tabs(browser: State<'_, Browser>) -> Vec<FrontendTab> {
    browser.snapshots().0
}

#[tauri::command]
pub fn active_tab_id(browser: State<'_, Browser>) -> u64 {
    browser.snapshots().1
}

#[tauri::command]
pub fn select_tab(browser: State<'_, Browser>, id: u64) -> Result<(), String> {
    let mut state = browser.0.state.lock().unwrap();
    if !state.tabs.iter().any(|tab| tab.id == id) {
        return Err(format!("unknown tab {id}"));
    }
    state.active = id;
    drop(state);
    browser.emit_state();
    Ok(())
}

#[tauri::command]
pub fn open_tab(browser: State<'_, Browser>, url: Option<String>) -> Result<FrontendTab, String> {
    let request = match url {
        Some(url) => request_from_input(&url).ok_or_else(|| format!("invalid URL {url}"))?,
        None => Request::get(Url::parse(NEW_TAB_URL).unwrap()),
    };
    let id = {
        let mut state = browser.0.state.lock().unwrap();
        let id = state.next_tab_id;
        state.next_tab_id += 1;
        state.tabs.push(TabState {
            id,
            history: vec![request],
            current: 0,
            title: String::new(),
            loading: false,
            outcome: PageOutcome::Blank,
            generation: 0,
        });
        state.active = id;
        id
    };
    browser.schedule_current(id, false);
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

#[tauri::command]
pub fn close_tab(browser: State<'_, Browser>, id: u64) {
    let mut state = browser.0.state.lock().unwrap();
    if state.tabs.len() == 1 {
        return;
    }
    let Some(index) = state.tabs.iter().position(|tab| tab.id == id) else {
        return;
    };
    state.tabs.remove(index);
    if state.active == id {
        state.active = state.tabs[index.min(state.tabs.len() - 1)].id;
    }
    drop(state);
    browser.emit_state();
}

#[tauri::command]
pub fn navigate(browser: State<'_, Browser>, id: u64, input: String) -> bool {
    request_from_input(&input).is_some_and(|request| browser.navigate_request(id, request))
}

#[tauri::command]
pub fn go_back(browser: State<'_, Browser>, id: u64) {
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
        browser.schedule_current(id, false);
    }
}

#[tauri::command]
pub fn go_forward(browser: State<'_, Browser>, id: u64) {
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
        browser.schedule_current(id, false);
    }
}

#[tauri::command]
pub fn reload(browser: State<'_, Browser>, id: u64) {
    browser.schedule_current(id, true);
}

#[tauri::command]
pub fn panel_state(browser: State<'_, Browser>) -> PanelState {
    browser.snapshots().2
}

#[tauri::command]
pub fn set_panel_collapsed(browser: State<'_, Browser>, collapsed: bool) {
    browser.0.state.lock().unwrap().panel.collapsed = collapsed;
    browser.emit_state();
}

#[tauri::command]
pub fn toggle_section(browser: State<'_, Browser>, section: String) -> Result<(), String> {
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
#[tauri::command]
pub fn debug_log(browser: State<'_, Browser>, since: Option<u64>) -> Vec<DebugEntry> {
    browser.0.log.lock().unwrap().since(since)
}

#[tauri::command]
pub fn status(browser: State<'_, Browser>) -> StatusReadout {
    browser.snapshots().3
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
        inspection: tauri_runtime_blitz::agent_control_enabled(),
        profiling: tauri_runtime_blitz::deep_profiling_enabled(),
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

#[tauri::command]
pub fn diagnostics() -> DiagnosticsState {
    diagnostics_now()
}

/// Turn the inspection plane and deep profiling on or off, now.
///
/// Both are live switches: enabling inspection binds the control socket and
/// disabling it unbinds and removes the descriptor, so turning this off in
/// Settings actually closes the door rather than hiding the button for it.
#[tauri::command]
pub fn set_diagnostics(inspection: bool, profiling: bool) -> Result<DiagnosticsState, String> {
    if diagnostics_locked() {
        // Report the truth instead of failing: the switch is simply not the
        // authority this run, and the window shows it as held by the
        // environment.
        return Ok(diagnostics_now());
    }
    tauri_runtime_blitz::apply_runtime_debug_options(tauri_runtime_blitz::RuntimeDebugOptions {
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
