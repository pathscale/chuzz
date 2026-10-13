//! A single socket reader hands requests to the page without polling.

use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use blitz_control_protocol::latest::Once;

use super::{ControlBridgeRequest, DebugResponse};

type DocumentRequest = (ControlBridgeRequest, Arc<Once<DebugResponse>>);

struct State {
    producer_open: bool,
    consumer_open: bool,
}

struct Wake {
    state: Mutex<State>,
    ready: Condvar,
}

pub(super) struct Sender {
    queue: spsc::Producer<DocumentRequest>,
    wake: Arc<Wake>,
}

pub(super) struct Receiver {
    queue: spsc::Consumer<DocumentRequest>,
    wake: Arc<Wake>,
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
    let wake = Arc::new(Wake {
        state: Mutex::new(State {
            producer_open: true,
            consumer_open: true,
        }),
        ready: Condvar::new(),
    });
    (
        Sender {
            queue: producer,
            wake: Arc::clone(&wake),
        },
        Receiver {
            queue: consumer,
            wake,
        },
    )
}

impl Sender {
    pub(super) fn try_send(&self, request: DocumentRequest) -> Result<(), TrySendError> {
        let state = self
            .wake
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if !state.consumer_open {
            return Err(TrySendError::Disconnected);
        }
        match self.queue.try_push(request) {
            Some(_) => Err(TrySendError::Full),
            None => {
                self.wake.ready.notify_one();
                Ok(())
            }
        }
    }
}

impl Receiver {
    pub(super) fn recv_timeout(
        &self,
        timeout: Duration,
    ) -> Result<DocumentRequest, RecvTimeoutError> {
        if let Some(request) = self.queue.try_pop() {
            return Ok(request);
        }
        let deadline = Instant::now() + timeout;
        let mut state = self
            .wake
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        loop {
            // Recheck under the notification lock before parking or reporting
            // closure. A producer may have published after the fast check.
            if let Some(request) = self.queue.try_pop() {
                return Ok(request);
            }
            if !state.producer_open {
                return Err(RecvTimeoutError::Disconnected);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(RecvTimeoutError::Timeout);
            }
            let (next, _) = self
                .wake
                .ready
                .wait_timeout(state, remaining)
                .unwrap_or_else(|error| error.into_inner());
            state = next;
        }
    }

    pub(super) fn try_recv(&self) -> Result<DocumentRequest, TryRecvError> {
        if let Some(request) = self.queue.try_pop() {
            return Ok(request);
        }
        let state = self
            .wake
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(request) = self.queue.try_pop() {
            Ok(request)
        } else if state.producer_open {
            Err(TryRecvError::Empty)
        } else {
            Err(TryRecvError::Disconnected)
        }
    }
}

impl Drop for Sender {
    fn drop(&mut self) {
        let mut state = self
            .wake
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.producer_open = false;
        self.wake.ready.notify_one();
    }
}

impl Drop for Receiver {
    fn drop(&mut self) {
        {
            let mut state = self
                .wake
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            state.consumer_open = false;
        }
        // Publication is now disabled. Finish each outstanding reply without
        // pretending its action ran, using the existing reply-close path.
        while let Some((_, answer)) = self.queue.try_pop() {
            nagoya::block_on(answer.abandon());
        }
    }
}
