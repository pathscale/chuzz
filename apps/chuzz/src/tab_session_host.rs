use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use super::{SessionOperation, SessionReply, SessionStore};
use crate::script_host::{ChuzzScriptHost, HostError, HostInstallContext, HostObject, HostValue};
use crate::tab_stash::{SessionBucket, SleepingBudget, Text};

const STATE_KEY: &str = "chuzz.session";

fn error(name: &'static str, message: &'static str) -> HostError {
    HostError::new(name, message)
}

fn session_owner(host: &dyn ChuzzScriptHost) -> Result<Rc<RefCell<SessionStore>>, HostError> {
    let state = host
        .state(STATE_KEY)
        .ok_or_else(|| error("TypeError", "sessionStorage owner unavailable"))?;
    state
        .downcast_ref::<Rc<RefCell<SessionStore>>>()
        .cloned()
        .ok_or_else(|| error("TypeError", "sessionStorage owner unavailable"))
}

fn text_arg(args: &[HostValue], index: usize, limit: usize) -> Result<Text, HostError> {
    let value = args
        .get(index)
        .ok_or_else(|| error("TypeError", "sessionStorage key or value must be a string"))?;

    let text = match value {
        HostValue::Utf16(units) => Text::capture(units.iter().copied(), limit),
        HostValue::String(value) => Text::capture(value.encode_utf16(), limit),
        _ => {
            return Err(error(
                "TypeError",
                "sessionStorage key or value must be a string",
            ));
        }
    };

    text.ok_or_else(|| error("RangeError", "sessionStorage quota exceeded"))
}

fn number_arg(args: &[HostValue], index: usize) -> Result<f64, HostError> {
    match args.get(index) {
        Some(HostValue::Number(value)) => Ok(*value),
        _ => Err(error("TypeError", "sessionStorage index must be a number")),
    }
}

fn quota_result<T>(result: Result<T, crate::tab_session::SessionError>) -> Result<T, HostError> {
    result.map_err(|_| error("RangeError", "sessionStorage quota exceeded"))
}

fn length(host: &mut dyn ChuzzScriptHost, _args: &[HostValue]) -> Result<HostValue, HostError> {
    let owner = session_owner(host)?;
    let reply = owner.borrow_mut().operate(SessionOperation::Length);
    match quota_result(reply)? {
        SessionReply::Length(length) => Ok(HostValue::Number(length as f64)),
        _ => Err(error("TypeError", "invalid sessionStorage response")),
    }
}

fn key(host: &mut dyn ChuzzScriptHost, args: &[HostValue]) -> Result<HostValue, HostError> {
    let index = number_arg(args, 0)? as usize;
    let owner = session_owner(host)?;
    let reply = owner.borrow_mut().operate(SessionOperation::Key { index });
    match quota_result(reply)? {
        SessionReply::Key(Some(value)) => Ok(HostValue::Utf16(Rc::from(value.units().to_vec()))),
        SessionReply::Key(None) => Ok(HostValue::Null),
        _ => Err(error("TypeError", "invalid sessionStorage response")),
    }
}

fn get_item(host: &mut dyn ChuzzScriptHost, args: &[HostValue]) -> Result<HostValue, HostError> {
    let key = text_arg(args, 0, 2048)?;
    let owner = session_owner(host)?;
    let reply = owner.borrow_mut().operate(SessionOperation::Get { key });
    match quota_result(reply)? {
        SessionReply::Value(Some(value)) => Ok(HostValue::Utf16(Rc::from(value.units().to_vec()))),
        SessionReply::Value(None) => Ok(HostValue::Null),
        _ => Err(error("TypeError", "invalid sessionStorage response")),
    }
}

fn set_item(host: &mut dyn ChuzzScriptHost, args: &[HostValue]) -> Result<HostValue, HostError> {
    let key = text_arg(args, 0, 2048)?;
    let value = text_arg(args, 1, 8192)?;
    let owner = session_owner(host)?;
    let reply = owner
        .borrow_mut()
        .operate(SessionOperation::Set { key, value });
    match quota_result(reply)? {
        SessionReply::Complete => Ok(HostValue::Undefined),
        _ => Err(error("TypeError", "invalid sessionStorage response")),
    }
}

fn remove_item(host: &mut dyn ChuzzScriptHost, args: &[HostValue]) -> Result<HostValue, HostError> {
    let key = text_arg(args, 0, 2048)?;
    let owner = session_owner(host)?;
    let reply = owner.borrow_mut().operate(SessionOperation::Remove { key });
    match quota_result(reply)? {
        SessionReply::Complete => Ok(HostValue::Undefined),
        _ => Err(error("TypeError", "invalid sessionStorage response")),
    }
}

fn clear(host: &mut dyn ChuzzScriptHost, _args: &[HostValue]) -> Result<HostValue, HostError> {
    let owner = session_owner(host)?;
    let reply = owner.borrow_mut().operate(SessionOperation::Clear);
    match quota_result(reply)? {
        SessionReply::Complete => Ok(HostValue::Undefined),
        _ => Err(error("TypeError", "invalid sessionStorage response")),
    }
}

fn mark_sensitive(
    host: &mut dyn ChuzzScriptHost,
    args: &[HostValue],
) -> Result<HostValue, HostError> {
    let key = text_arg(args, 0, 2048)?;
    let owner = session_owner(host)?;
    let reply = owner
        .borrow_mut()
        .operate(SessionOperation::MarkSensitive { key });
    match quota_result(reply)? {
        SessionReply::Complete => Ok(HostValue::Undefined),
        _ => Err(error("TypeError", "invalid sessionStorage response")),
    }
}

pub fn install(
    host: &mut dyn ChuzzScriptHost,
    config: &HostInstallContext,
) -> Result<(), HostError> {
    let origin = config
        .base
        .as_ref()
        .map(url::Url::origin)
        .unwrap_or_else(url::Origin::new_opaque);
    let store = Rc::new(RefCell::new(SessionStore::new(
        origin,
        config.session_backing.as_ref().map(Arc::clone),
    )));
    host.set_state(STATE_KEY, HostObject::new(store))?;

    host.register_intrinsic("__chuzzSession", 3, dispatch)?;
    host.register_intrinsic("__chuzzSessionLength", 0, length)?;
    host.register_intrinsic("__chuzzSessionKey", 1, key)?;
    host.register_intrinsic("__chuzzSessionGetItem", 1, get_item)?;
    host.register_intrinsic("__chuzzSessionSetItem", 2, set_item)?;
    host.register_intrinsic("__chuzzSessionRemoveItem", 1, remove_item)?;
    host.register_intrinsic("__chuzzSessionClear", 0, clear)?;
    host.register_intrinsic("__chuzzSessionMarkSensitive", 1, mark_sensitive)?;
    Ok(())
}

fn dispatch(host: &mut dyn ChuzzScriptHost, args: &[HostValue]) -> Result<HostValue, HostError> {
    let operation = number_arg(args, 0)? as u8;
    let rest = &args[1..];
    match operation {
        0 => length(host, rest),
        1 => key(host, rest),
        2 => get_item(host, rest),
        3 => set_item(host, rest),
        5 => clear(host, rest),
        6 => mark_sensitive(host, rest),
        _ => remove_item(host, rest),
    }
}

pub fn capture(
    host: &dyn ChuzzScriptHost,
    budget: &Arc<SleepingBudget>,
) -> Result<Option<Arc<SessionBucket>>, HostError> {
    let owner = session_owner(host)?;
    Ok(owner.borrow_mut().capture(budget))
}
