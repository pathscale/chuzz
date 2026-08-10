//! Per-tab session history.
//!
//! This is browser policy, deliberately kept out of the engine: the engine
//! reports that a navigation was requested, and this decides what the back and
//! forward stacks look like afterwards.

use blitz_traits::navigation::{NavigationOptions, NavigationProvider};
use blitz_traits::net::Request;
use dioxus_native::prelude::*;

/// History must be reachable from the engine's navigation provider, which is
/// `Send + Sync`, so the store uses sync storage.
pub type SyncStore<T> = Store<T, CopyValue<T, SyncStorage>>;

#[derive(Store)]
pub struct History {
    entries: Vec<Request>,
    current: usize,
}

impl History {
    pub fn new(initial_request: Request) -> Self {
        Self {
            entries: vec![initial_request],
            current: 0,
        }
    }
}

#[store]
impl<Lens> Store<History, Lens> {
    fn current_idx(&self) -> usize {
        *self.current().read()
    }

    fn current_request(&self) -> impl Readable<Target = Request> {
        // `current` is only ever moved within bounds, and `entries` is never
        // emptied, so this index is always occupied.
        #[allow(clippy::expect_used)]
        self.entries()
            .get(self.current_idx())
            .expect("current index is in bounds")
    }

    fn has_back(&self) -> bool {
        self.current_idx() > 0
    }

    fn has_forward(&self) -> bool {
        self.current_idx() < self.entries().len() - 1
    }

    fn go_back(&mut self) {
        if self.has_back() {
            *self.current().write() -= 1;
        }
    }

    fn go_forward(&mut self) {
        if self.has_forward() {
            *self.current().write() += 1;
        }
    }

    /// Navigating from the middle of the stack drops everything ahead of the
    /// current entry, which is what every browser does.
    fn navigate(&self, req: Request)
    where
        Lens: Writable,
    {
        let idx = self.current_idx();
        self.entries().write().truncate(idx + 1);
        self.entries().push(req);
        *self.current().write() += 1;
    }
}

/// The `#[store]` macro generates a module-private extension trait. This
/// re-exposes it so the toolbar and tab modules can drive history.
pub trait HistoryNav {
    fn current_request(&self) -> impl Readable<Target = Request>;
    fn has_back(&self) -> bool;
    fn has_forward(&self) -> bool;
    fn go_back(&mut self);
    fn go_forward(&mut self);
    fn navigate(&self, req: Request);
}

impl HistoryNav for SyncStore<History> {
    fn current_request(&self) -> impl Readable<Target = Request> {
        HistoryStoreImplExt::current_request(self)
    }

    fn has_back(&self) -> bool {
        HistoryStoreImplExt::has_back(self)
    }

    fn has_forward(&self) -> bool {
        HistoryStoreImplExt::has_forward(self)
    }

    fn go_back(&mut self) {
        HistoryStoreImplExt::go_back(self)
    }

    fn go_forward(&mut self) {
        HistoryStoreImplExt::go_forward(self)
    }

    fn navigate(&self, req: Request) {
        HistoryStoreImplExt::navigate(self, req)
    }
}

/// Handed to every page document so that link clicks and form submissions
/// inside a page land in that tab's history instead of being dropped.
pub struct TabNavProvider {
    pub history: SyncStore<History>,
}

impl NavigationProvider for TabNavProvider {
    fn navigate_to(&self, options: NavigationOptions) {
        HistoryNav::navigate(&self.history, options.into_request());
    }
}
