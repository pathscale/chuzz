//! Typed request ownership and document wakes through nagoya notification.

use super::{ControlBridgeRequest, DebugResponse};
use blitz_control_protocol::latest::Once;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Wake, Waker};
use std::time::Duration;

pub(super) struct Reply(Arc<Once<Arc<DebugResponse>>>);
impl Reply {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self(Once::new()))
    }
    pub(super) fn fill(&self, result: DebugResponse) -> bool {
        self.0.fill(Arc::new(result))
    }
    pub(super) async fn recv(&self) -> Option<Arc<DebugResponse>> {
        self.0.recv().await
    }
    async fn abandon(&self) {
        self.0.abandon().await;
    }
}
type DocumentRequest = (ControlBridgeRequest, Arc<Reply>);

struct State {
    producer_open: bool,
    consumer_open: bool,
}
struct Ready {
    state: Mutex<State>,
    notification: nagoya::sync::Notify,
    page: AtomicBool,
}
impl Wake for Ready {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.page.store(true, Ordering::Release);
        self.notification.notify_one();
    }
}

pub(super) struct Sender {
    queue: spsc::Producer<Arc<DocumentRequest>>,
    ready: Arc<Ready>,
}
pub(super) struct Receiver {
    queue: spsc::Consumer<Arc<DocumentRequest>>,
    ready: Arc<Ready>,
}
pub(super) enum TrySendError {
    Full,
    Disconnected,
}
pub(super) enum RecvTimeoutError {
    Timeout,
    Disconnected,
}
pub(super) enum TryRecvError {
    Empty,
    Disconnected,
}

pub(super) fn channel(capacity: usize) -> (Sender, Receiver) {
    let (producer, consumer) = spsc::make(capacity);
    let ready = Arc::new(Ready {
        state: Mutex::new(State {
            producer_open: true,
            consumer_open: true,
        }),
        notification: nagoya::sync::Notify::new(),
        page: AtomicBool::new(false),
    });
    (
        Sender {
            queue: producer,
            ready: Arc::clone(&ready),
        },
        Receiver {
            queue: consumer,
            ready,
        },
    )
}

impl Sender {
    pub(super) fn document_waker(&self) -> Waker {
        Waker::from(Arc::clone(&self.ready))
    }
    pub(super) fn try_send(&self, request: Arc<DocumentRequest>) -> Result<(), TrySendError> {
        let state = self
            .ready
            .state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if !state.consumer_open {
            return Err(TrySendError::Disconnected);
        }
        if self.queue.try_push(request).is_some() {
            return Err(TrySendError::Full);
        }
        if !super::reactor_defect("page-ring-wakeup") {
            self.ready.notification.notify_one();
        }
        Ok(())
    }
}

fn take(request: Arc<DocumentRequest>) -> DocumentRequest {
    match Arc::try_unwrap(request) {
        Ok(request) => request,
        Err(_) => panic!("document request ownership was shared"),
    }
}

impl Receiver {
    #[cfg(feature = "reactor-witness")]
    pub(super) async fn recv_arc(&self) -> Result<Arc<DocumentRequest>, TryRecvError> {
        loop {
            if let Some(request) = self.queue.try_pop() {
                return Ok(request);
            }
            if !self
                .ready
                .state
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .producer_open
            {
                return Err(TryRecvError::Disconnected);
            }
            self.ready.notification.notified().await;
        }
    }
    pub(super) fn recv_timeout(
        &self,
        timeout: Duration,
    ) -> Result<DocumentRequest, RecvTimeoutError> {
        self.receive(Some(timeout))
    }
    pub(super) fn receive(
        &self,
        timeout: Option<Duration>,
    ) -> Result<DocumentRequest, RecvTimeoutError> {
        let next = async {
            loop {
                let notification = self.ready.notification.notified();
                futures::pin_mut!(notification);
                let enabled = notification.as_mut().enable();
                if let Some(request) = self.queue.try_pop() {
                    return Ok(take(request));
                }
                if !self
                    .ready
                    .state
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .producer_open
                {
                    return Err(RecvTimeoutError::Disconnected);
                }
                if self.ready.page.swap(false, Ordering::AcqRel) {
                    return Err(RecvTimeoutError::Timeout);
                }
                if !enabled {
                    notification.await;
                }
            }
        };
        nagoya::block_on(async {
            match timeout {
                Some(timeout) => nagoya::timeout(timeout, next)
                    .await
                    .unwrap_or(Err(RecvTimeoutError::Timeout)),
                None => next.await,
            }
        })
    }
    pub(super) fn try_recv(&self) -> Result<DocumentRequest, TryRecvError> {
        if let Some(request) = self.queue.try_pop() {
            return Ok(take(request));
        }
        if self
            .ready
            .state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .producer_open
        {
            Err(TryRecvError::Empty)
        } else {
            Err(TryRecvError::Disconnected)
        }
    }
}

impl Drop for Sender {
    fn drop(&mut self) {
        self.ready
            .state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .producer_open = false;
        self.ready.notification.notify_one();
    }
}
impl Drop for Receiver {
    fn drop(&mut self) {
        self.ready
            .state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .consumer_open = false;
        while let Some(request) = self.queue.try_pop() {
            nagoya::block_on(request.1.abandon());
        }
    }
}
