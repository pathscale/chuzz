use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use blitz_script::ScriptDocument;
use blitz_script::boa_engine::{Context, JsNativeError, JsResult, JsString, JsValue, NativeFunction};

use crate::tab_stash::{SessionBucket, SessionPair, SleepingBudget, Text};

struct Store {
    origin: url::Origin,
    backing: Option<Arc<SessionBucket>>,
    entries: Vec<SessionPair>,
    sensitive: Vec<Text>,
    no_stash: bool,
}

#[derive(Clone)]
struct Host(Rc<RefCell<Store>>);

impl Store {
    fn pairs(&self) -> &[SessionPair] { self.backing.as_ref().map_or(&self.entries, |bucket| bucket.pairs()) }

    fn make_mut(&mut self) {
        if let Some(bucket) = self.backing.take() {
            self.entries = match Arc::try_unwrap(bucket) {
                Ok(bucket) => bucket.into_pairs(),
                Err(bucket) => bucket.pairs().iter().map(|pair| SessionPair {
                    key: Text::capture(pair.key.units().iter().copied(), 2048).expect("bounded native key"),
                    value: Text::capture(pair.value.units().iter().copied(), 8192).expect("bounded native value"),
                }).collect(),
            };
        }
    }

    fn capture(&mut self, budget: &Arc<SleepingBudget>) -> Option<Arc<SessionBucket>> {
        if self.no_stash { self.backing.take(); self.entries.clear(); return None; }
        if self.sensitive.is_empty() && self.backing.is_some() { return self.backing.take(); }
        self.make_mut();
        self.entries.retain(|pair| !self.sensitive.iter().any(|key| key.units() == pair.key.units()));
        SessionBucket::seal(budget, self.origin.clone(), std::mem::take(&mut self.entries)).ok()
    }
}

fn text(value: JsValue, context: &mut Context, limit: usize) -> JsResult<Text> {
    let value = value.to_string(context)?;
    Text::capture(value.iter(), limit).ok_or_else(|| JsNativeError::range().with_message("sessionStorage quota exceeded").into())
}

fn session(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = context.get_data::<Host>().cloned().ok_or_else(|| JsNativeError::typ().with_message("sessionStorage owner unavailable"))?;
    let op = args.first().cloned().unwrap_or_default().to_number(context)? as u8;
    if op == 0 { return Ok(JsValue::from(host.0.borrow().pairs().len() as u32)); }
    if op == 1 {
        let index = args.get(1).cloned().unwrap_or_default().to_number(context)? as usize;
        return Ok(host.0.borrow().pairs().get(index).map_or_else(JsValue::null, |pair| JsString::from(pair.key.units()).into()));
    }
    if op == 5 {
        let mut store = host.0.borrow_mut();
        store.backing.take();
        store.entries.clear();
        return Ok(JsValue::undefined());
    }
    let key = text(args.get(1).cloned().unwrap_or_default(), context, 2048)?;
    if op == 2 {
        return Ok(host.0.borrow().pairs().iter().find(|pair| pair.key.units() == key.units())
            .map_or_else(JsValue::null, |pair| JsString::from(pair.value.units()).into()));
    }
    let value = if op == 3 { Some(text(args.get(2).cloned().unwrap_or_default(), context, 8192)?) } else { None };
    let mut store = host.0.borrow_mut();
    if op == 6 {
        if !store.sensitive.iter().any(|old| old.units() == key.units()) {
            if store.sensitive.len() >= 256 { store.no_stash = true; return Ok(JsValue::undefined()); }
            store.sensitive.push(key);
        }
        return Ok(JsValue::undefined());
    }
    store.make_mut();
    if let Some(value) = value {
        let bytes = store.entries.iter().filter(|pair| pair.key.units() != key.units())
            .map(|pair| pair.key.allocated_bytes() + pair.value.allocated_bytes()).sum::<usize>();
        let replacing = store.entries.iter().position(|pair| pair.key.units() == key.units());
        if bytes + key.allocated_bytes() + value.allocated_bytes() > 24 * 1024 || replacing.is_none() && store.entries.len() >= 256 {
            return Err(JsNativeError::range().with_message("sessionStorage quota exceeded").into());
        }
        let pair = SessionPair { key, value };
        if let Some(index) = replacing { store.entries[index] = pair; } else { store.entries.push(pair); }
    } else { store.entries.retain(|pair| pair.key.units() != key.units()); }
    Ok(JsValue::undefined())
}

pub fn install(document: &mut ScriptDocument, backing: Option<Arc<SessionBucket>>) {
    let origin = document.current_url().map(url::Url::origin).unwrap_or_else(|| url::Origin::new_opaque());
    document.with_js_context(|context| {
        context.insert_data(Host(Rc::new(RefCell::new(Store { origin, backing, entries: Vec::new(), sensitive: Vec::new(), no_stash: false }))));
        context.register_global_builtin_callable(JsString::from("__chuzzSession"), 3, NativeFunction::from_fn_ptr(session)).expect("native session owner");
    });
    document.eval_named(r#"Object.defineProperty(globalThis, 'sessionStorage', {value: Object.freeze({
      get length() { return __chuzzSession(0); }, key(i) { return __chuzzSession(1, i); },
      getItem(k) { return __chuzzSession(2, String(k)); }, setItem(k, v) { __chuzzSession(3, String(k), String(v)); },
      removeItem(k) { __chuzzSession(4, String(k)); }, clear() { __chuzzSession(5); },
      markSensitive(k) { __chuzzSession(6, String(k)); }
    })});"#, "<native-session-storage>");
}

pub fn capture(document: &mut ScriptDocument, budget: &Arc<SleepingBudget>) -> Option<Arc<SessionBucket>> {
    document.with_js_context(|context| context.get_data::<Host>().cloned()).and_then(|host| host.0.borrow_mut().capture(budget))
}
