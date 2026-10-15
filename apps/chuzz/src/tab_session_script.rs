use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use blitz_script::ScriptDocument;
use blitz_script::boa_engine::{
    Context, JsNativeError, JsResult, JsString, JsValue, NativeFunction,
};

use crate::tab_stash::{SessionBucket, SleepingBudget, Text};

use super::{SessionOperation, SessionReply, SessionStore};

#[derive(Clone)]
struct Host(Rc<RefCell<SessionStore>>);

fn units(value: JsValue, context: &mut Context, limit: usize) -> JsResult<Text> {
    Text::capture(value.to_string(context)?.iter(), limit).ok_or_else(|| {
        JsNativeError::range()
            .with_message("sessionStorage quota exceeded")
            .into()
    })
}

fn session(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = context
        .get_data::<Host>()
        .cloned()
        .ok_or_else(|| JsNativeError::typ().with_message("sessionStorage owner unavailable"))?;
    let op = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_number(context)? as u8;

    let operation = match op {
        0 => SessionOperation::Length,
        1 => {
            let index = args
                .get(1)
                .cloned()
                .unwrap_or_default()
                .to_number(context)? as usize;
            SessionOperation::Key { index }
        }
        5 => SessionOperation::Clear,
        2 => SessionOperation::Get {
            key: units(args.get(1).cloned().unwrap_or_default(), context, 2048)?,
        },
        3 => {
            let key = units(args.get(1).cloned().unwrap_or_default(), context, 2048)?;
            let value = units(args.get(2).cloned().unwrap_or_default(), context, 8192)?;
            SessionOperation::Set { key, value }
        }
        6 => SessionOperation::MarkSensitive {
            key: units(args.get(1).cloned().unwrap_or_default(), context, 2048)?,
        },
        _ => SessionOperation::Remove {
            key: units(args.get(1).cloned().unwrap_or_default(), context, 2048)?,
        },
    };

    let reply = host
        .0
        .borrow_mut()
        .operate(operation)
        .map_err(|_| JsNativeError::range().with_message("sessionStorage quota exceeded"))?;

    match reply {
        SessionReply::Length(length) => Ok(JsValue::from(length as u32)),
        SessionReply::Key(key) | SessionReply::Value(key) => {
            Ok(key.map_or_else(JsValue::null, |text| JsString::from(text.units()).into()))
        }
        SessionReply::Complete => Ok(JsValue::undefined()),
    }
}

pub(super) fn install(document: &mut ScriptDocument, backing: Option<Arc<SessionBucket>>) {
    let origin = document
        .current_url()
        .map(url::Url::origin)
        .unwrap_or_else(url::Origin::new_opaque);
    document.with_js_context(|context| {
        context.insert_data(Host(Rc::new(RefCell::new(SessionStore::new(
            origin, backing,
        )))));
        context
            .register_global_builtin_callable(
                JsString::from("__chuzzSession"),
                3,
                NativeFunction::from_fn_ptr(session),
            )
            .expect("native session owner");
    });
    document.eval_named(r#"Object.defineProperty(globalThis, 'sessionStorage', {value: Object.freeze({
      get length() { return __chuzzSession(0); }, key(i) { return __chuzzSession(1, i); },
      getItem(k) { return __chuzzSession(2, String(k)); }, setItem(k, v) { __chuzzSession(3, String(k), String(v)); },
      removeItem(k) { __chuzzSession(4, String(k)); }, clear() { __chuzzSession(5); },
      markSensitive(k) { __chuzzSession(6, String(k)); }
    })});"#, "<native-session-storage>");
}

pub(super) fn capture(
    document: &mut ScriptDocument,
    budget: &Arc<SleepingBudget>,
) -> Option<Arc<SessionBucket>> {
    document
        .with_js_context(|context| context.get_data::<Host>().cloned())
        .and_then(|host| host.0.borrow_mut().capture(budget))
}
