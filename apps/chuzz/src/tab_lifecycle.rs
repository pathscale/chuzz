use super::*;
use std::collections::BTreeSet;
use std::future::{Future, poll_fn};
use std::task::{Poll, Waker};

use crate::tab_stash::{Charge, PageStash, PaintSnapshot, ScrollState, StashLocation};
use blitz_dom::release::Teardown;

#[derive(Clone, Copy, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TabPhase {
    Dormant,
    Live,
    Freezing,
    Discarding,
    Sleeping,
    Waking,
    RestoreFailed,
}

pub struct Deadline {
    pub due: u64,
    pub generation: u64,
    pub epoch: u64,
    cancelled: AtomicBool,
    reader: Mutex<Option<Waker>>,
}

impl Deadline {
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        if let Some(reader) = self.reader.lock().unwrap().take() {
            reader.wake();
        }
    }

    async fn expires(&self) -> bool {
        let mut timer = Box::pin(nagoya::sleep_until(self.due));
        poll_fn(|cx| {
            *self.reader.lock().unwrap() = Some(cx.waker().clone());
            if self.cancelled.load(Ordering::Acquire) {
                return Poll::Ready(false);
            }
            timer
                .as_mut()
                .poll(cx)
                .map(|()| !self.cancelled.load(Ordering::Acquire))
        })
        .await
    }
}

pub struct TabLife {
    pub phase: TabPhase,
    pub visible: bool,
    pub epoch: u64,
    pub hidden_at: u64,
    pub deadline: Option<Arc<Deadline>>,
    pub restore_deadline: Option<Arc<Deadline>>,
    pub load_signal: Option<blitz_traits::net::AbortSignal>,
    pub ever_live: bool,
    pub stash: Option<Arc<PageStash>>,
    pub preview: Option<Arc<PaintSnapshot>>,
    pub preview_surface: Option<izumo::PageSurface>,
    pub record_charge: Option<Charge>,
    pub release: Option<Arc<Teardown>>,
    pub wake_requested: bool,
    pub await_preview: bool,
    pub discarded: bool,
    pub reason: &'static str,
}

impl Default for TabLife {
    fn default() -> Self {
        Self {
            phase: TabPhase::Dormant,
            visible: false,
            epoch: 0,
            hidden_at: nagoya::now_ns(),
            deadline: None,
            restore_deadline: None,
            load_signal: None,
            ever_live: false,
            stash: None,
            preview: None,
            preview_surface: None,
            record_charge: None,
            release: None,
            wake_requested: false,
            await_preview: false,
            discarded: false,
            reason: "",
        }
    }
}

impl TabLife {
    pub fn cancel_deadline(&mut self) {
        self.epoch = self
            .epoch
            .checked_add(1)
            .expect("tab deadline token exhausted");
        if let Some(deadline) = self.deadline.take() {
            deadline.cancel();
        }
    }

    pub fn cancel_restore(&mut self) {
        if let Some(deadline) = self.restore_deadline.take() {
            deadline.cancel();
        }
    }

    pub fn navigate(&mut self) {
        self.cancel_deadline();
        self.cancel_restore();
        if let Some(signal) = self.load_signal.take() {
            blitz_traits::net::AbortController { signal }.abort();
        }
        self.stash.take();
        self.preview.take();
        self.preview_surface.take();
        self.record_charge.take();
        self.discarded = false;
        self.await_preview = false;
        if !matches!(self.phase, TabPhase::Discarding | TabPhase::Freezing) {
            self.phase = TabPhase::Dormant;
        }
    }
}

impl Drop for TabLife {
    fn drop(&mut self) {
        if let Some(deadline) = self.deadline.take() {
            deadline.cancel();
        }
        self.cancel_restore();
        if let Some(signal) = self.load_signal.take() {
            blitz_traits::net::AbortController { signal }.abort();
        }
    }
}

pub enum LifecycleMessage {
    Sleep {
        id: u64,
        generation: u64,
        epoch: u64,
        automatic: bool,
    },
    Released {
        id: u64,
        receipt: Arc<Teardown>,
    },
    Surface(Arc<izumo::SurfaceEvent>),
    RestoreTimeout {
        id: u64,
        generation: u64,
    },
}

impl Browser {
    pub(super) fn install_surface_lifecycle(&self) {
        let provider = Arc::downgrade(&self.0);
        let observer = Arc::downgrade(&self.0);
        izumo::set_page_surface_hooks(
            move || {
                let Some(inner) = provider.upgrade() else {
                    return Vec::new();
                };
                let state = inner.state.lock().unwrap();
                state
                    .tabs
                    .iter()
                    .filter_map(|tab| {
                        if let Some(page) = &tab.mounted {
                            Some(izumo::PageSurface {
                                tab_id: tab.id,
                                generation: tab.generation,
                                owner_document_id: page.owner_document_id,
                                mount_node_id: page.mount_node_id,
                                document_id: page.document_id,
                            })
                        } else {
                            tab.life.preview_surface
                        }
                    })
                    .collect()
            },
            move |event| {
                if let Some(inner) = observer.upgrade() {
                    let browser = Browser(inner);
                    browser
                        .0
                        .lifecycle
                        .lock()
                        .unwrap()
                        .push_back(LifecycleMessage::Surface(event));
                    browser.wake_document();
                }
            },
        );
    }

    fn arm_hidden(&self, id: u64) {
        let deadline = {
            let mut state = self.0.state.lock().unwrap();
            let Some(tab) = state.tabs.iter_mut().find(|tab| tab.id == id) else {
                return;
            };
            if tab.life.visible || tab.life.phase != TabPhase::Live || tab.life.deadline.is_some() {
                return;
            }
            let deadline = Arc::new(Deadline {
                due: tab.life.hidden_at.saturating_add(self.0.sleep_delay_ns),
                generation: tab.generation,
                epoch: tab.life.epoch,
                cancelled: AtomicBool::new(false),
                reader: Mutex::new(None),
            });
            tab.life.deadline = Some(Arc::clone(&deadline));
            deadline
        };
        let owner = Arc::downgrade(&self.0);
        nagoya::spawn(async move {
            if !deadline.expires().await {
                return;
            }
            if let Some(inner) = owner.upgrade() {
                let browser = Browser(inner);
                browser
                    .0
                    .lifecycle
                    .lock()
                    .unwrap()
                    .push_back(LifecycleMessage::Sleep {
                        id,
                        generation: deadline.generation,
                        epoch: deadline.epoch,
                        automatic: true,
                    });
                browser.wake_document();
            }
        });
    }

    pub(super) fn arm_restore(&self, id: u64, generation: u64) {
        let deadline = {
            let mut state = self.0.state.lock().unwrap();
            let Some(tab) = state
                .tabs
                .iter_mut()
                .find(|tab| tab.id == id && tab.generation == generation)
            else {
                return;
            };
            if tab.life.phase != TabPhase::Waking {
                return;
            }
            tab.life.cancel_restore();
            let deadline = Arc::new(Deadline {
                due: nagoya::now_ns().saturating_add(30_000_000_000),
                generation,
                epoch: tab.life.epoch,
                cancelled: AtomicBool::new(false),
                reader: Mutex::new(None),
            });
            tab.life.restore_deadline = Some(Arc::clone(&deadline));
            deadline
        };
        let owner = Arc::downgrade(&self.0);
        nagoya::spawn(async move {
            if deadline.expires().await
                && let Some(inner) = owner.upgrade()
            {
                let browser = Browser(inner);
                browser
                    .0
                    .lifecycle
                    .lock()
                    .unwrap()
                    .push_back(LifecycleMessage::RestoreTimeout { id, generation });
                browser.wake_document();
            }
        });
    }

    fn restore_timeout(
        &self,
        ui: &mut blitz_script::ScriptDocument,
        id: u64,
        generation: u64,
    ) -> bool {
        let mount = {
            let mut state = self.0.state.lock().unwrap();
            let Some(tab) = state.tabs.iter_mut().find(|tab| {
                tab.id == id && tab.generation == generation && tab.life.phase == TabPhase::Waking
            }) else {
                return false;
            };
            tab.life.cancel_restore();
            if let Some(signal) = tab.life.load_signal.take() {
                blitz_traits::net::AbortController { signal }.abort();
            }
            tab.generation = tab
                .generation
                .checked_add(1)
                .expect("document generation exhausted");
            tab.loading = false;
            tab.life.phase = TabPhase::RestoreFailed;
            tab.life.visible = false;
            tab.life.reason = "restore-timeout";
            tab.life.preview_surface.take();
            tab.mounted
                .take()
                .map(|page| NodeId::from_u64(page.mount_node_id))
        };
        if let Some(mount) = mount
            && let Some(receipt) = ui.inner_mut().retire_sub_document(mount)
        {
            self.track_release(id, receipt);
        }
        true
    }

    pub(super) fn request_sleep(&self, id: u64) -> Result<(), String> {
        let (generation, epoch) = {
            let state = self.0.state.lock().unwrap();
            let tab = state
                .tabs
                .iter()
                .find(|tab| tab.id == id)
                .ok_or("unknown tab")?;
            if tab.loading || tab.mounted.is_none() || tab.life.phase != TabPhase::Live {
                return Err("tab-is-not-live".into());
            }
            (tab.generation, tab.life.epoch)
        };
        self.0
            .lifecycle
            .lock()
            .unwrap()
            .push_back(LifecycleMessage::Sleep {
                id,
                generation,
                epoch,
                automatic: false,
            });
        self.wake_document();
        Ok(())
    }

    pub(super) fn request_wake(&self, id: u64) -> Result<(), String> {
        let preview = {
            let mut state = self.0.state.lock().unwrap();
            let tab = state
                .tabs
                .iter_mut()
                .find(|tab| tab.id == id)
                .ok_or("unknown tab")?;
            if tab.life.phase == TabPhase::Discarding || tab.life.release.is_some() {
                tab.life.wake_requested = true;
                return Ok(());
            }
            if !matches!(tab.life.phase, TabPhase::Sleeping | TabPhase::RestoreFailed) {
                return Err("tab-is-not-sleeping".into());
            }
            if !matches!(tab.request().method.as_str(), "GET" | "HEAD") {
                tab.life.phase = TabPhase::RestoreFailed;
                tab.life.reason = "reload-requires-resubmission";
                drop(state);
                self.emit_state();
                return Ok(());
            }
            tab.life.phase = TabPhase::Waking;
            tab.life.preview_surface.take();
            tab.life.reason = "";
            tab.life.await_preview = tab.life.preview.is_some();
            tab.life.await_preview
        };
        self.emit_state();
        self.wake_document();
        if !preview {
            self.schedule_current(id, true);
        }
        Ok(())
    }

    pub(super) fn prepare_tab_page(
        &self,
        page: &mut blitz_script::ScriptDocument,
        id: u64,
        generation: u64,
        url: &str,
    ) {
        let destination = Url::parse(url).ok();
        let (discarded, restore) = {
            let state = self.0.state.lock().unwrap();
            let tab = state
                .tabs
                .iter()
                .find(|tab| tab.id == id && tab.generation == generation);
            match tab {
                Some(tab) => (
                    tab.life.discarded,
                    destination.as_ref().and_then(|url| {
                        tab.life.stash.as_ref().and_then(|stash| {
                            Arc::clone(stash).restore(url, tab.current as u64).ok()
                        })
                    }),
                ),
                None => (false, None),
            }
        };
        page.set_was_discarded(discarded);
        let waking = self.0.state.lock().unwrap().tabs.iter().any(|tab| {
            tab.id == id && tab.generation == generation && tab.life.phase == TabPhase::Waking
        });
        page.inner_mut().page_input_blocked = waking;
        if waking {
            page.with_js_context(|context| {
                context.set_execution_time_limit(Some(WINDOW_NETWORK_DEADLINE))
            });
        }
        crate::tab_session::install(page, restore.as_ref().and_then(|restore| restore.session()));
        if let Some(restore) = restore {
            if destination
                .as_ref()
                .is_some_and(|url| url.fragment().is_none())
            {
                page.inner_mut().restore_scroll = Some(blitz_dom::Point {
                    x: restore.scroll().x,
                    y: restore.scroll().y,
                });
            }
            let mut applied = BTreeSet::new();
            page.set_before_script_hook(move |document| {
                crate::tab_native::restore_forms(document, &restore, &mut applied)
            });
        }
    }

    fn commit_sleep(
        &self,
        ui: &mut blitz_script::ScriptDocument,
        id: u64,
        generation: u64,
        epoch: u64,
        automatic: bool,
    ) -> bool {
        let (mount, url, entry, replayable, record_bytes, mut loading) = {
            let mut state = self.0.state.lock().unwrap();
            let Some(tab) = state.tabs.iter_mut().find(|tab| {
                tab.id == id && tab.generation == generation && tab.life.epoch == epoch
            }) else {
                return false;
            };
            if tab.loading && !automatic
                || tab.life.phase != TabPhase::Live
                || automatic
                    && (tab.life.visible
                        || tab
                            .life
                            .deadline
                            .as_ref()
                            .is_none_or(|deadline| nagoya::now_ns() < deadline.due))
            {
                return false;
            }
            let Some(page) = &tab.mounted else {
                return false;
            };
            if ui
                .inner()
                .subdoc(NodeId::from_u64(page.mount_node_id))
                .is_none_or(|document| document.id() as u64 != page.document_id)
            {
                return false;
            }
            let mount = NodeId::from_u64(page.mount_node_id);
            let record_bytes = size_of::<TabState>()
                + tab.title.capacity()
                + tab.history.capacity() * size_of::<Request>()
                + tab
                    .history
                    .iter()
                    .map(|request| request.url.as_str().len())
                    .sum::<usize>();
            (
                mount,
                tab.request().url.clone(),
                tab.current as u64,
                matches!(tab.request().method.as_str(), "GET" | "HEAD"),
                record_bytes,
                tab.loading,
            )
        };
        if record_bytes > 32 * 1024 {
            return false;
        }
        let mut charge = self.0.sleeping_budget.reserve(32 * 1024);
        if charge.is_err() {
            let mut state = self.0.state.lock().unwrap();
            for tab in &mut state.tabs {
                tab.life.preview.take();
            }
            charge = self.0.sleeping_budget.reserve(32 * 1024);
        }
        let Ok(charge) = charge else {
            return false;
        };
        {
            let mut state = self.0.state.lock().unwrap();
            if let Some(tab) = state.tabs.iter_mut().find(|tab| tab.id == id) {
                tab.life.phase = TabPhase::Freezing;
            }
        }
        let (stash, preview) = {
            let mut root = ui.inner_mut();
            let Some(document) = root.subdoc_mut(mount) else {
                return false;
            };
            document.set_page_visible(false);
            document.freeze();
            let still_current =
                self.0.state.lock().unwrap().tabs.iter().any(|tab| {
                    tab.id == id && tab.generation == generation && tab.life.epoch == epoch
                });
            if !still_current {
                let navigation_only = self.0.state.lock().unwrap().tabs.iter().any(|tab| {
                    tab.id == id
                        && tab.generation == generation
                        && tab.life.phase == TabPhase::Freezing
                        && tab.loading
                });
                if navigation_only {
                    loading = true;
                } else {
                    document.resume();
                    return false;
                }
            }
            document.inner_mut().classify_stash();
            let forms = if loading {
                Vec::new()
            } else {
                crate::tab_native::capture_forms(&mut document.inner_mut())
            };
            let scroll = document.inner().viewport_scroll();
            let disabled = loading || document.inner().stash_disabled();
            let session = if disabled {
                None
            } else {
                (document as &mut dyn std::any::Any)
                    .downcast_mut::<blitz_script::ScriptDocument>()
                    .and_then(|page| crate::tab_session::capture(page, &self.0.sleeping_budget))
            };
            let stash = if disabled {
                None
            } else {
                PageStash::seal(
                    &self.0.sleeping_budget,
                    StashLocation {
                        url: &url,
                        entry,
                        generation,
                    },
                    replayable,
                    forms,
                    session,
                    ScrollState {
                        x: scroll.x,
                        y: scroll.y,
                    },
                    0,
                )
                .ok()
            };
            let preview = if matches!(url.origin(), url::Origin::Tuple(..)) {
                crate::tab_native::capture_paint(&mut document.inner_mut(), &self.0.sleeping_budget)
            } else {
                None
            };
            (stash, preview)
        };
        {
            let mut state = self.0.state.lock().unwrap();
            let Some(tab) = state
                .tabs
                .iter_mut()
                .find(|tab| tab.id == id && tab.generation == generation)
            else {
                return false;
            };
            tab.life.cancel_deadline();
            tab.generation = tab
                .generation
                .checked_add(1)
                .expect("document generation exhausted");
            tab.life.phase = TabPhase::Discarding;
            tab.life.visible = false;
            tab.life.discarded = true;
            tab.life.cancel_restore();
            if let Some(signal) = tab.life.load_signal.take() {
                blitz_traits::net::AbortController { signal }.abort();
            }
            tab.life.stash = stash;
            tab.life.preview = preview;
            tab.life.record_charge = Some(charge);
            tab.life.reason = if tab.life.stash.is_some() {
                ""
            } else {
                "stash-incomplete"
            };
            tab.mounted.take();
            tab.loading = false;
            // Retain method/URL only. Sleeping history cannot retain POST bodies or credentials.
            for request in &mut tab.history {
                let mut record = Request::get(request.url.clone());
                record.method = request.method.clone();
                record.destination = request.destination;
                record.initiator = request.initiator.clone();
                *request = record;
            }
            let next = tab.generation;
            state.records.loading(TabId(id), DocumentGeneration(next));
        }
        let Some(receipt) = ui.inner_mut().retire_sub_document(mount) else {
            return false;
        };
        self.track_release(id, receipt);
        ui.inner().shell_provider.request_redraw();
        true
    }

    pub(super) fn track_release(&self, id: u64, receipt: Arc<Teardown>) {
        {
            let mut state = self.0.state.lock().unwrap();
            if let Some(tab) = state.tabs.iter_mut().find(|tab| tab.id == id) {
                tab.life.release = Some(Arc::clone(&receipt));
            }
        }
        let owner = Arc::downgrade(&self.0);
        nagoya::spawn(async move {
            Arc::clone(&receipt).released().await;
            if let Some(inner) = owner.upgrade() {
                let browser = Browser(inner);
                browser
                    .0
                    .lifecycle
                    .lock()
                    .unwrap()
                    .push_back(LifecycleMessage::Released { id, receipt });
                browser.wake_document();
            }
        });
    }

    pub(super) fn poll_lifecycle(&self, ui: &mut blitz_script::ScriptDocument) -> bool {
        let mut messages: Vec<_> = self.0.lifecycle.lock().unwrap().drain(..).collect();
        messages.sort_by_key(|message| {
            if matches!(message, LifecycleMessage::Surface(_)) {
                0
            } else {
                1
            }
        });
        let mut changed = false;
        for message in messages {
            match message {
                LifecycleMessage::RestoreTimeout { id, generation } => {
                    changed |= self.restore_timeout(ui, id, generation);
                }
                LifecycleMessage::Sleep {
                    id,
                    generation,
                    epoch,
                    automatic,
                } => {
                    changed |= self.commit_sleep(ui, id, generation, epoch, automatic);
                }
                LifecycleMessage::Released { id, receipt } => {
                    let wake = {
                        let mut state = self.0.state.lock().unwrap();
                        let Some(tab) = state.tabs.iter_mut().find(|tab| tab.id == id) else {
                            continue;
                        };
                        if tab
                            .life
                            .release
                            .as_ref()
                            .is_none_or(|pending| !Arc::ptr_eq(pending, &receipt))
                            || !receipt.complete()
                        {
                            continue;
                        }
                        tab.life.release.take();
                        if tab.life.phase != TabPhase::RestoreFailed {
                            tab.life.phase = TabPhase::Sleeping;
                        }
                        let wake = std::mem::take(&mut tab.life.wake_requested);
                        state.records.sleeping(TabId(id));
                        wake
                    };
                    changed = true;
                    if wake {
                        let _ = self.request_wake(id);
                    }
                }
                LifecycleMessage::Surface(event) => match *event {
                    izumo::SurfaceEvent::Visibility { page, visible } => {
                        let mut state = self.0.state.lock().unwrap();
                        let Some(tab) = state.tabs.iter_mut().find(|tab| {
                            tab.id == page.tab_id
                                && tab.generation == page.generation
                                && tab
                                    .mounted
                                    .as_ref()
                                    .is_some_and(|mount| mount.document_id == page.document_id)
                        }) else {
                            continue;
                        };
                        if tab.life.visible == visible {
                            continue;
                        }
                        tab.life.visible = visible;
                        tab.life.cancel_deadline();
                        if !visible {
                            tab.life.hidden_at = nagoya::now_ns();
                        }
                        drop(state);
                        self.arm_hidden(page.tab_id);
                        changed = true;
                    }
                    izumo::SurfaceEvent::Painted { page } => {
                        let mut state = self.0.state.lock().unwrap();
                        let Some(tab) = state
                            .tabs
                            .iter_mut()
                            .find(|tab| tab.id == page.tab_id && tab.generation == page.generation)
                        else {
                            continue;
                        };
                        if tab.life.phase != TabPhase::Waking {
                            continue;
                        }
                        if tab.life.await_preview && tab.life.preview_surface == Some(page) {
                            tab.life.await_preview = false;
                            drop(state);
                            self.schedule_current(page.tab_id, true);
                            changed = true;
                            continue;
                        }
                        if tab
                            .mounted
                            .as_ref()
                            .is_none_or(|mount| mount.document_id != page.document_id)
                        {
                            continue;
                        }
                        let mount = tab.mounted.as_ref().unwrap().mount_node_id;
                        tab.life.phase = TabPhase::Live;
                        tab.life.cancel_restore();
                        tab.life.stash.take();
                        tab.life.preview.take();
                        tab.life.preview_surface.take();
                        tab.life.record_charge.take();
                        drop(state);
                        if let Some(document) = ui.inner_mut().subdoc_mut(NodeId::from_u64(mount)) {
                            document.finish_restore();
                        }
                        changed = true;
                    }
                },
            }
        }
        let selected = {
            let state = self.0.state.lock().unwrap();
            state
                .tabs
                .iter()
                .find(|tab| {
                    tab.id == state.active
                        && tab.life.preview.is_some()
                        && tab.life.preview_surface.is_none()
                })
                .map(|tab| {
                    (
                        tab.id,
                        tab.generation,
                        Arc::clone(tab.life.preview.as_ref().unwrap()),
                    )
                })
        };
        if let Some((id, generation, preview)) = selected {
            let node = ui.inner().get_element_by_id(&format!("chuzz-saved-{id}"));
            if let Some(node) = node
                && crate::tab_native::show_preview(&mut ui.inner_mut(), node, &preview)
            {
                let surface = izumo::PageSurface {
                    tab_id: id,
                    generation,
                    owner_document_id: ui.id() as u64,
                    mount_node_id: node.as_u64(),
                    document_id: ui.id() as u64,
                };
                if let Some(tab) = self
                    .0
                    .state
                    .lock()
                    .unwrap()
                    .tabs
                    .iter_mut()
                    .find(|tab| tab.id == id && tab.generation == generation)
                {
                    tab.life.preview_surface = Some(surface);
                }
                ui.inner().shell_provider.request_redraw();
                changed = true;
            }
        }
        changed
    }

    pub(super) fn attached_lifecycle(&self, id: u64) {
        self.arm_hidden(id);
    }
}

pub(super) fn sleep_delay_ns() -> u64 {
    std::env::var("CHUZZ_TAB_SLEEP_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|delay| *delay > 0 && *delay <= 86_400_000)
        .unwrap_or(300_000)
        .saturating_mul(1_000_000)
}
