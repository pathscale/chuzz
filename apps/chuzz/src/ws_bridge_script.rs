use blitz_script::boa_engine::{Context, JsString, JsValue};

use super::{Bridge, Event, SocketReply, SocketRequest};

#[derive(serde::Deserialize)]
struct WireCommand {
    /// Present only on this bridge's messages. The IPC channel is shared, so
    /// this is what distinguishes a socket command from a fetch.
    ws: String,
    id: u64,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    protocols: Vec<String>,
    /// Accepted for wire compatibility. Native document identity supplies Origin.
    #[serde(default, rename = "origin")]
    _origin: Option<String>,
    #[serde(default)]
    data: Option<String>,
    #[serde(default)]
    code: Option<u16>,
}

pub(super) fn handle(bridge: &Bridge, message: &str) -> bool {
    let Ok(command) = serde_json::from_str::<WireCommand>(message) else {
        return false;
    };
    let request = match command.ws.as_str() {
        "connect" => Some(SocketRequest::Connect {
            id: command.id,
            url: command.url,
            protocols: command.protocols,
        }),
        "send" => Some(SocketRequest::Send {
            id: command.id,
            data: command.data,
        }),
        "close" => Some(SocketRequest::Close {
            id: command.id,
            code: command.code,
        }),
        // Claim commands addressed to this bridge, even when their operation
        // is not implemented.
        _ => None,
    };
    if let Some(request) = request {
        let id = match &request {
            SocketRequest::Connect { id, .. }
            | SocketRequest::Send { id, .. }
            | SocketRequest::Close { id, .. } => *id,
        };
        if let Err(error) = bridge.operate(request) {
            bridge.reject(id, error);
        }
    }
    true
}

fn reply_value(reply: &SocketReply) -> serde_json::Value {
    match reply {
        SocketReply::Open { protocol } => {
            serde_json::json!({"type": "open", "protocol": protocol})
        }
        SocketReply::Message { data } => serde_json::json!({"type": "message", "data": data}),
        SocketReply::Error { message } => {
            serde_json::json!({"type": "error", "message": message})
        }
        SocketReply::Close { code, reason } => {
            serde_json::json!({"type": "close", "code": code, "reason": reason})
        }
    }
}

pub(super) fn drain_into(bridge: &Bridge, document: &mut blitz_script::ScriptDocument) -> bool {
    let ready = match bridge.events.lock() {
        Ok(mut queue) => std::mem::take(&mut *queue),
        Err(_) => Vec::new(),
    };
    if ready.is_empty() {
        return false;
    }

    for Event { id, reply } in ready {
        crate::serve::verbose(&format!("socket {id} delivering {reply:?}"));
        let payload = reply_value(&reply);
        document.with_js_context(|context| deliver(id, &payload, context));
    }
    true
}

fn deliver(id: u64, payload: &serde_json::Value, context: &mut Context) {
    let callback = context
        .global_object()
        .get(JsString::from("__chuzzWsEvent"), context)
        .ok()
        .and_then(|value| value.as_object())
        .filter(|function| function.is_callable());
    let Some(callback) = callback else {
        return;
    };
    let Ok(event) = JsValue::from_json(payload, context) else {
        return;
    };
    if let Err(error) = callback.call(
        &JsValue::undefined(),
        &[JsValue::from(id as f64), event],
        context,
    ) {
        eprintln!("WebSocket event callback: {error}");
    }
    if let Err(error) = context.run_jobs() {
        eprintln!("WebSocket event jobs: {error}");
    }
}
