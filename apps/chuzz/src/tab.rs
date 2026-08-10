//! Tabs: the unit of browsing state.
//!
//! A tab owns its history, its loader, and the page document currently swapped
//! in. The page is a child document rendered inside the `web-view` element, so
//! page markup and browser UI markup never share a DOM.

use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use blitz_traits::net::{Request, Url};
use dioxus_native::{SubDocumentAttr, prelude::*};

use crate::document_loader::{DocumentLoader, LoadStatus, LoadedDocument, NetProvider};
use crate::history::{History, HistoryNav, SyncStore};
use crate::nav::display_title;

pub type TabId = u64;

static NEXT_TAB_ID: AtomicU64 = AtomicU64::new(0);

fn next_tab_id() -> TabId {
    NEXT_TAB_ID.fetch_add(1, Ordering::Relaxed)
}

#[derive(Store)]
pub struct Tab {
    pub id: TabId,
    pub history: SyncStore<History>,
    pub loader: Option<Rc<DocumentLoader>>,
    pub document: Option<SubDocumentAttr>,
    pub title: String,
}

#[store(pub)]
impl<Lens> Store<Tab, Lens> {
    fn nav_history(&self) -> SyncStore<History> {
        *self.history().read()
    }

    fn loader_rc(&self) -> Rc<DocumentLoader> {
        // `open_tab` sets the loader immediately after pushing the tab, before
        // any view code can observe it.
        #[allow(clippy::expect_used)]
        self.loader()
            .cloned()
            .expect("loader is set at tab creation")
    }

    fn tab_id(&self) -> TabId {
        *self.id().read()
    }

    fn current_url(&self) -> Url {
        self.nav_history().current_request().read().url.clone()
    }

    fn navigate(&self, req: Request) {
        self.nav_history().navigate(req);
    }

    fn reload(&self) {
        self.loader_rc().reload();
    }

    fn go_back(&self) {
        self.nav_history().go_back();
    }

    fn go_forward(&self) {
        self.nav_history().go_forward();
    }

    fn can_go_back(&self) -> bool {
        self.nav_history().has_back()
    }

    fn can_go_forward(&self) -> bool {
        self.nav_history().has_forward()
    }

    fn is_loading(&self) -> bool {
        *self.loader_rc().status.read() == LoadStatus::Loading
    }
}

pub fn open_tab(
    mut tabs: Store<Vec<Tab>>,
    url: Url,
    net_provider: Arc<NetProvider>,
) -> Store<Tab, impl Writable<Target = Tab> + Copy> {
    let history: SyncStore<History> = Store::new_maybe_sync(History::new(Request::get(url)));

    tabs.push(Tab {
        id: next_tab_id(),
        history,
        loader: None,
        document: None,
        title: String::new(),
    });

    // The tab we just pushed is the last one.
    #[allow(clippy::expect_used)]
    let tab = tabs.iter().last().expect("tab was just pushed");
    *tab.loader().write() = Some(Rc::new(DocumentLoader::new(net_provider, history)));
    tab
}

pub fn active_tab(tabs: Store<Vec<Tab>>, active_id: TabId) -> Store<Tab> {
    // At least one tab is always open, and closing a tab reassigns the active
    // id before removing the row.
    #[allow(clippy::expect_used)]
    tabs.iter()
        .find(|tab| tab.tab_id() == active_id)
        .expect("active tab id always refers to an open tab")
        .into()
}

pub fn tab_display_title<L>(tab: Store<Tab, L>) -> String
where
    L: Copy + Readable<Target = Tab> + 'static,
{
    let title = tab.title().cloned();
    display_title(&title, &tab.nav_history().current_request().read().url)
}

fn commit_loaded(tab: Store<Tab>, loaded: LoadedDocument) {
    // Applied as a straight-line block so no render can sample a state where
    // the new document is showing under the previous page's title.
    *tab.title().write_unchecked() = loaded.title;
    *tab.document().write_unchecked() = Some(loaded.document);
}

/// Renders one tab's page. Inactive tabs stay mounted but hidden, so switching
/// back to a tab does not refetch it.
#[component]
pub fn TabView(tab: Store<Tab>, active_tab_id: Signal<TabId>) -> Element {
    let loader = tab.loader_rc();

    let loaded = use_resource(move || {
        let req = (*tab.nav_history().current_request().read()).clone();
        // Read so a reload re-runs this resource even when the URL is equal.
        let _generation = loader.reload_generation();
        let loader = loader.clone();
        async move { loader.load(req).await }
    });

    use_effect(move || {
        let mut status = tab.loader_rc().status;
        match loaded.state().cloned() {
            UseResourceState::Pending => status.set(LoadStatus::Loading),
            UseResourceState::Ready | UseResourceState::Stopped | UseResourceState::Paused => {
                status.set(LoadStatus::Idle)
            }
        }
    });

    use_effect(move || {
        if loaded.read().is_some()
            && let Some(document) = loaded.write_unchecked().take()
        {
            commit_loaded(tab, document);
        }
    });

    let id = tab.tab_id();
    let document = tab.document().cloned();
    let visibility = if id == active_tab_id() {
        "display: block"
    } else {
        "display: none"
    };

    rsx!(web-view {
        key: "{id}",
        class: "page",
        style: visibility,
        "__webview_document": document,
    })
}
