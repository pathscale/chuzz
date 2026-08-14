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
const EMPTY_HTML: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>Empty response</title></head><body><h1>Empty response</h1><p>The server returned no content.</p></body></html>"#;

fn error_html(error: &str) -> String {
    let escaped = error
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><title>Page not available</title></head><body><h1>Page not available</h1><p>{escaped}</p></body></html>"#
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
            status: if self.loading { "loading" } else { "idle" },
            can_go_back: self.current > 0,
            can_go_forward: self.current + 1 < self.history.len(),
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
    html: String,
    scripts: HashMap<Url, String>,
}

struct BrowserInner {
    state: Mutex<BrowserState>,
    net: Arc<NetProvider>,
    completed: Mutex<VecDeque<PageBundle>>,
    app: Mutex<Option<ChuzzAppHandle>>,
}

#[derive(Clone)]
pub struct Browser(Arc<BrowserInner>);

impl Browser {
    pub fn new(startup: Option<Url>) -> Self {
        let first = startup.unwrap_or_else(|| Url::parse(NEW_TAB_URL).unwrap());
        Self(Arc::new(BrowserInner {
            state: Mutex::new(BrowserState {
                tabs: vec![TabState {
                    id: 0,
                    history: vec![Request::get(first)],
                    current: 0,
                    title: String::new(),
                    loading: false,
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
                    },
                },
            }),
            net: Arc::new(NetProvider::new(None)),
            completed: Mutex::new(VecDeque::new()),
            app: Mutex::new(None),
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
            status: if active_tab.loading {
                "loading"
            } else {
                "idle"
            },
            url: active_tab.request().url.to_string(),
            tab_count: tabs.len(),
            node_count: 0,
            transferred: "0 B",
        };
        (tabs, active, state.panel.clone(), status)
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

        let browser = self.clone();
        tauri::async_runtime::spawn(async move {
            let bundle = fetch_page(&browser.0.net, tab_id, generation, request, revalidate).await;
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
            let config = DocumentConfig {
                base_url: Some(bundle.resolved_url.clone()),
                net_provider: Some(Arc::clone(&self.0.net) as _),
                navigation_provider: Some(Arc::new(PageNavigation {
                    browser: Arc::downgrade(&self.0),
                    tab_id: bundle.tab_id,
                })),
                shell_provider: Some(shell_provider),
                html_parser_provider: Some(Arc::new(HtmlProvider)),
                font_ctx: Some(FontContext::default()),
                ..Default::default()
            };
            let mut page = blitz_script::ScriptDocument::from_html(&bundle.html, config)
                .with_fetcher(PrefetchedScripts {
                    scripts: bundle.scripts,
                });
            page.eval(WEB_API_SHIM);
            page.execute_scripts();
            let title = page
                .inner()
                .find_title_node()
                .map(|node| node.text_content())
                .unwrap_or_default();
            ui.inner_mut().set_sub_document(target, Box::new(page));

            {
                let mut state = self.0.state.lock().unwrap();
                if let Some(tab) = state.tabs.iter_mut().find(|tab| tab.id == bundle.tab_id) {
                    if let Ok(url) = Url::parse(&bundle.resolved_url) {
                        tab.history[tab.current].url = url;
                    }
                    tab.title = title;
                    tab.loading = false;
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

struct PrefetchedScripts {
    scripts: HashMap<Url, String>,
}

impl blitz_script::ScriptFetcher for PrefetchedScripts {
    fn fetch(&self, url: &Url) -> Result<String, blitz_script::FetchError> {
        self.scripts
            .get(url)
            .cloned()
            .map(Ok)
            .unwrap_or_else(|| blitz_script::DefaultScriptFetcher.fetch(url))
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
    net: &Arc<NetProvider>,
    tab_id: u64,
    generation: u64,
    mut request: Request,
    force_revalidate: bool,
) -> PageBundle {
    if request.url.scheme() == "about" {
        return PageBundle {
            tab_id,
            generation,
            resolved_url: request.url.to_string(),
            html: BLANK_HTML.to_owned(),
            scripts: HashMap::new(),
        };
    }
    if force_revalidate {
        revalidate(&mut request);
    }
    let requested_url = request.url.to_string();

    let (resolved_url, bytes) = match net.fetch_async(request).await {
        Ok(response) => response,
        Err(error) => {
            return PageBundle {
                tab_id,
                generation,
                resolved_url: requested_url,
                html: error_html(&format!("{error:?}")),
                scripts: HashMap::new(),
            };
        }
    };
    let html = if bytes.is_empty() {
        EMPTY_HTML.to_owned()
    } else {
        decode_body(&bytes)
    };
    let urls = {
        let document = blitz_script::ScriptDocument::from_html(
            &html,
            DocumentConfig {
                base_url: Some(resolved_url.clone()),
                ..Default::default()
            },
        );
        document.external_script_urls()
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
        if let Ok((_, bytes)) = net.fetch_async(script_request).await {
            scripts.insert(url, decode_body(&bytes));
        }
    }
    PageBundle {
        tab_id,
        generation,
        resolved_url,
        html,
        scripts,
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
        _ => return Err(format!("unknown panel section {section}")),
    };
    *value = !*value;
    drop(state);
    browser.emit_state();
    Ok(())
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
    let Some(path) = diagnostics_path() else { return };
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

    #[test]
    fn browser_starts_with_one_blank_tab() {
        let browser = Browser::new(None);
        let (tabs, active, _, status) = browser.snapshots();
        assert_eq!(tabs.len(), 1);
        assert_eq!(active, 0);
        assert_eq!(status.url, NEW_TAB_URL);
    }
}
