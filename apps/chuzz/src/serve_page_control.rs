//! Typed page requests on the existing SPSC ring, woken by Nagoya.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use blitz_control_protocol::latest::Once;
use super::{ControlBridgeRequest, DebugResponse};

type DocumentRequest = (ControlBridgeRequest, Arc<Once<DebugResponse>>);

struct State {
    producer_open: bool,
    consumer_open: bool,
}

struct Wake {
    state: Mutex<State>,
    ready: nagoya::sync::Notify,
}

pub(super) struct Sender {
    queue: spsc::Producer<Arc<DocumentRequest>>,
    wake: Arc<Wake>,
}

pub(super) struct Receiver {
    queue: spsc::Consumer<Arc<DocumentRequest>>,
    wake: Arc<Wake>,
}

pub(super) enum TrySendError { Full, Disconnected }
pub(super) enum RecvTimeoutError { Timeout, Disconnected }
pub(super) enum TryRecvError { Empty, Disconnected }

pub(super) fn channel(capacity: usize) -> (Sender, Receiver) {
    let (producer, consumer) = spsc::make(capacity);
    let wake = Arc::new(Wake {
        state: Mutex::new(State { producer_open: true, consumer_open: true }),
        ready: nagoya::sync::Notify::new(),
    });
    (Sender { queue: producer, wake: Arc::clone(&wake) }, Receiver { queue: consumer, wake })
}

impl Sender {
    pub(super) fn try_send(&self, request: Arc<DocumentRequest>) -> Result<(), TrySendError> {
        {
            let state = self.wake.state.lock().unwrap_or_else(|error| error.into_inner());
            if !state.consumer_open { return Err(TrySendError::Disconnected); }
            if self.queue.try_push(request).is_some() { return Err(TrySendError::Full); }
        }
        if !super::reactor_defect("page-ring-wakeup") { self.wake.ready.notify_one(); }
        Ok(())
    }
}

impl Receiver {
    pub(super) async fn recv_arc(&self) -> Result<Arc<DocumentRequest>, TryRecvError> {
        loop {
            // notify_one retains a permit when publication precedes this wait.
            match self.try_recv_arc() {
                Err(TryRecvError::Empty) => self.wake.ready.notified().await,
                result => return result,
            }
        }
    }

    pub(super) fn recv_timeout(&self, timeout: Duration) -> Result<DocumentRequest, RecvTimeoutError> {
        match nagoya::block_on(nagoya::timeout(timeout, self.recv_arc())) {
            Ok(Ok(request)) => Ok(Arc::try_unwrap(request).unwrap_or_else(|_| panic!("page request ownership was retained after enqueue"))),
            Ok(Err(_)) => Err(RecvTimeoutError::Disconnected),
            Err(_) => Err(RecvTimeoutError::Timeout),
        }
    }

    pub(super) fn try_recv(&self) -> Result<DocumentRequest, TryRecvError> {
        self.try_recv_arc().map(|request| Arc::try_unwrap(request).unwrap_or_else(|_| panic!("page request ownership was retained after enqueue")))
    }

    fn try_recv_arc(&self) -> Result<Arc<DocumentRequest>, TryRecvError> {
        let state = self.wake.state.lock().unwrap_or_else(|error| error.into_inner());
        match self.queue.try_pop() {
            Some(request) => Ok(request),
            None if state.producer_open => Err(TryRecvError::Empty),
            None => Err(TryRecvError::Disconnected),
        }
    }
}

impl Drop for Sender {
    fn drop(&mut self) {
        self.wake.state.lock().unwrap_or_else(|error| error.into_inner()).producer_open = false;
        self.wake.ready.notify_one();
    }
}

impl Drop for Receiver {
    fn drop(&mut self) {
        self.wake.state.lock().unwrap_or_else(|error| error.into_inner()).consumer_open = false;
        while let Some(request) = self.queue.try_pop() {
            nagoya::block_on(request.1.abandon());
        }
    }
}
