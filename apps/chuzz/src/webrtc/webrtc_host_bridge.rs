use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::task::Waker;

use serde_json::Value;

use crate::script_host::{
    ChuzzScriptHost, HostCallbackId, HostError, HostInstallContext, HostObject, HostValue,
};
use crate::webrtc::host::{self, Page, Resources};

const STATE_KEY: &str = "chuzz.webrtc";
const MAX_COMMAND_BYTES: usize = 192 * 1024;
const MAX_ID: u64 = 9_007_199_254_740_991;
const POLL_BATCH: usize = 64;

struct BridgeState {
    page: Arc<Page>,
    callbacks: RefCell<BTreeMap<u64, HostCallbackId>>,
}

impl Drop for BridgeState {
    fn drop(&mut self) {
        self.page.close();
    }
}

struct HostResources<'a> {
    host: &'a dyn ChuzzScriptHost,
}

impl Resources for HostResources<'_> {
    fn media_epoch(&self) -> Result<std::time::Instant, String> {
        crate::audio::native_host::media_epoch(self.host)
            .map_err(|error| format!("{}: {}", error.name, error.message))
    }

    fn media(&self) -> Result<Arc<crate::media::Host>, String> {
        crate::media::native_host::capture_host(self.host)
            .map_err(|error| format!("{}: {}", error.name, error.message))
    }

    fn register_audio(&self, track: Arc<crate::audio::pcm::Track>) -> Result<u64, String> {
        crate::audio::native_host::register_track(self.host, track)
            .map_err(|error| format!("{}: {}", error.name, error.message))
    }

    fn audio(&self, id: u64) -> Result<Arc<crate::audio::pcm::Track>, String> {
        crate::audio::native_host::registered_track(self.host, id)
            .map_err(|error| format!("{}: {}", error.name, error.message))
    }
}

fn bridge_state(host: &dyn ChuzzScriptHost) -> Result<HostObject, HostError> {
    host.state(STATE_KEY)
        .ok_or_else(|| HostError::new("InvalidStateError", "peer host is unavailable"))
}

fn native_error(error: String) -> HostError {
    match error.split_once(": ") {
        Some((name, message)) => HostError::new(name, message),
        None => HostError::new("OperationError", error),
    }
}

fn control(host: &mut dyn ChuzzScriptHost, args: &[HostValue]) -> Result<HostValue, HostError> {
    let request = args
        .first()
        .ok_or_else(|| HostError::new("TypeError", "a peer operation is required"))?;
    let data = crate::script_host::to_json(request)?;
    if data.to_string().len() > MAX_COMMAND_BYTES {
        return Err(HostError::new(
            "OperationError",
            "peer command exceeds 192 KiB",
        ));
    }

    let state = bridge_state(host)?;
    let state = state
        .downcast_ref::<BridgeState>()
        .ok_or_else(|| HostError::new("InvalidStateError", "peer host state is invalid"))?;
    let closing = data.get("op").and_then(Value::as_str) == Some("close");
    let peer_id = data.get("peer").and_then(Value::as_u64);

    match host::execute(&state.page, data, &HostResources { host }) {
        Ok(value) => {
            let callback = if closing {
                peer_id.and_then(|peer_id| state.callbacks.borrow_mut().remove(&peer_id))
            } else {
                None
            };
            if let Some(callback) = callback {
                host.release_callback(callback)?;
            }
            Ok(crate::script_host::from_json(value))
        }
        Err(error) => Err(native_error(error)),
    }
}

fn subscribe(host: &mut dyn ChuzzScriptHost, args: &[HostValue]) -> Result<HostValue, HostError> {
    let peer = crate::script_host::number_arg(args, 0)?;
    if !peer.is_finite() || peer.fract() != 0.0 || peer < 1.0 || peer > MAX_ID as f64 {
        return Err(HostError::new("TypeError", "invalid peer identifier"));
    }
    let peer = peer as u64;
    let callback = match args.get(1) {
        Some(HostValue::Object(object)) => object
            .downcast_ref::<HostCallbackId>()
            .cloned()
            .ok_or_else(|| HostError::new("TypeError", "expected a peer callback"))?,
        _ => return Err(HostError::new("TypeError", "expected a peer callback")),
    };

    let incoming = callback.clone();
    let state = bridge_state(host)?;
    let state = state
        .downcast_ref::<BridgeState>()
        .ok_or_else(|| HostError::new("InvalidStateError", "peer host state is invalid"))?;
    let previous = {
        let mut callbacks = state.callbacks.borrow_mut();
        if callbacks.len() >= 1024 && !callbacks.contains_key(&peer) {
            return Err(HostError::new(
                "QuotaExceededError",
                "too many peer subscriptions",
            ));
        }
        callbacks.insert(peer, callback)
    };
    if let Some(previous) = previous.filter(|previous| !previous.same_callback(&incoming)) {
        host.release_callback(previous)?;
    }
    Ok(HostValue::Undefined)
}

fn crypto_probe(_: &mut dyn ChuzzScriptHost, _: &[HostValue]) -> Result<HostValue, HostError> {
    super::qa::run_fixture_json()
        .map(|json| HostValue::String(json.into()))
        .map_err(|error| HostError::new("OperationError", error))
}

pub fn install(
    host: &mut dyn ChuzzScriptHost,
    _config: &HostInstallContext,
) -> Result<(), HostError> {
    let page = Arc::new(Page {
        state: Mutex::new(Default::default()),
    });
    host.set_state(
        STATE_KEY,
        HostObject::new(BridgeState {
            page,
            callbacks: RefCell::new(BTreeMap::new()),
        }),
    )?;
    host.register_intrinsic("__chuzzPeerControl", 1, control)?;
    host.register_intrinsic("__chuzzPeerSubscribe", 2, subscribe)?;
    host.register_intrinsic("__chuzzWebrtcCryptoProbe", 0, crypto_probe)?;
    Ok(())
}

pub fn poll(host: &mut dyn ChuzzScriptHost, waker: Option<&Waker>) -> Result<bool, HostError> {
    let state = bridge_state(host)?;
    let state = state
        .downcast_ref::<BridgeState>()
        .ok_or_else(|| HostError::new("InvalidStateError", "peer host state is invalid"))?;
    let page = Arc::clone(&state.page);

    let events: VecDeque<Value> = {
        let mut page_state = host::lock(&page.state);
        if let Some(waker) = waker {
            page_state.reader = Some(waker.clone());
        }
        let count = page_state.events.len().min(POLL_BATCH);
        page_state.events.drain(..count).collect()
    };

    if events.is_empty() {
        return Ok(false);
    }

    let mut first_error = None;
    for envelope in events {
        let Some(peer) = envelope.get("peer").and_then(Value::as_u64) else {
            continue;
        };
        let Some(event) = envelope.get("event") else {
            continue;
        };
        let callback = state.callbacks.borrow().get(&peer).cloned();
        let Some(callback) = callback else {
            continue;
        };

        if let Err(error) =
            host.dispatch_callback(callback, &[crate::script_host::from_json(event.clone())])
            && first_error.is_none()
        {
            first_error = Some(error);
        }

        if event.get("type").and_then(Value::as_str) == Some("fatal") {
            let callback = state.callbacks.borrow_mut().remove(&peer);
            if let Some(callback) = callback
                && let Err(error) = host.release_callback(callback)
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
    }

    let page_state = host::lock(&page.state);
    if !page_state.events.is_empty()
        && let Some(reader) = &page_state.reader
    {
        reader.wake_by_ref();
    }

    match first_error {
        Some(error) => Err(error),
        None => Ok(true),
    }
}
