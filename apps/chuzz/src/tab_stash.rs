use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use zeroize::Zeroize;

pub const SLEEPING_BYTES: usize = 256 * 1024 * 1024;
pub const STATE_BYTES: usize = 64 * 1024;
pub const PREVIEW_TARGET: usize = 128 * 1024;
pub const PREVIEW_HARD: usize = 256 * 1024;

/// Sealed host backing. No Clone, Debug, serializer or guest address.
pub struct Text(Vec<u16>);

impl Text {
    pub fn capture(value: impl Iterator<Item = u16>, limit: usize) -> Option<Self> {
        let capacity = value.size_hint().1.unwrap_or(limit / 2).min(limit / 2);
        let mut result = Self(Vec::with_capacity(capacity));
        for unit in value {
            if result.0.len() >= capacity { return None; }
            result.0.push(unit);
        }
        Some(result)
    }

    pub fn units(&self) -> &[u16] { &self.0 }
    pub fn allocated_bytes(&self) -> usize { self.0.capacity() * 2 }
}

impl Drop for Text {
    fn drop(&mut self) {
        self.zeroize();
    }
}

impl Zeroize for Text {
    fn zeroize(&mut self) { self.0.zeroize(); }
}

pub struct Bytes(Vec<u8>);

impl Bytes {
    pub fn new(bytes: Vec<u8>) -> Self { Self(bytes) }
    pub fn bytes(&self) -> &[u8] { &self.0 }
    pub fn extend(&mut self, bytes: &[u8]) {
        assert!(self.0.len() + bytes.len() <= self.0.capacity());
        self.0.extend_from_slice(bytes);
    }
    pub fn allocated_bytes(&self) -> usize { self.0.capacity() }
}

impl AsRef<[u8]> for Bytes {
    fn as_ref(&self) -> &[u8] { self.bytes() }
}

impl Drop for Bytes {
    fn drop(&mut self) {
        self.0.resize(self.0.capacity(), 0);
        self.0.zeroize();
    }
}

/// Application policy, independent of process-wide CAP_ERROR behavior.
pub struct SleepingBudget {
    used: AtomicUsize,
    cap: usize,
    warn_at: usize,
}

impl SleepingBudget {
    pub fn new(cap: usize) -> Arc<Self> {
        assert!(cap > 0);
        Arc::new(Self { used: AtomicUsize::new(0), cap, warn_at: cap - cap / 5 })
    }

    pub fn used(&self) -> usize { self.used.load(Ordering::Acquire) }
    pub fn warning(&self) -> bool { self.used() >= self.warn_at }

    pub fn reserve(self: &Arc<Self>, bytes: usize) -> Result<Charge, &'static str> {
        self.used.fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
            used.checked_add(bytes).filter(|next| *next <= self.cap)
        }).map_err(|_| "sleeping-tab-budget")?;
        Ok(Charge { budget: Arc::clone(self), bytes })
    }
}

impl Default for SleepingBudget {
    fn default() -> Self {
        Self { used: AtomicUsize::new(0), cap: SLEEPING_BYTES, warn_at: SLEEPING_BYTES - SLEEPING_BYTES / 5 }
    }
}

/// Lives inside the charged owner, so all Arc leases remain accounted.
pub struct Charge { budget: Arc<SleepingBudget>, bytes: usize }

impl Drop for Charge {
    fn drop(&mut self) { self.budget.used.fetch_sub(self.bytes, Ordering::AcqRel); }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub enum FieldKind { Text, Textarea, Checkbox, Radio, Select }

pub struct FormLocator {
    pub form: Text,
    pub field: Text,
    pub action: Text,
    pub kind: FieldKind,
}

pub struct FormState {
    pub locator: FormLocator,
    pub value: Text,
    pub checked: bool,
    pub selection: Option<(u32, u32)>,
}

impl FormState {
    fn allocated_bytes(&self) -> usize {
        self.locator.form.allocated_bytes() + self.locator.field.allocated_bytes()
            + self.locator.action.allocated_bytes() + self.value.allocated_bytes()
    }
}

impl Zeroize for FormState {
    fn zeroize(&mut self) {
        self.locator.form.zeroize();
        self.locator.field.zeroize();
        self.locator.action.zeroize();
        self.value.zeroize();
        self.checked.zeroize();
        self.selection.zeroize();
    }
}

fn scrub_origin(origin: &mut url::Origin) {
    if let url::Origin::Tuple(scheme, host, port) = origin {
        scheme.zeroize();
        if let url::Host::Domain(domain) = host { domain.zeroize(); }
        port.zeroize();
    }
}

pub struct SessionPair { pub key: Text, pub value: Text }

impl Zeroize for SessionPair {
    fn zeroize(&mut self) { self.key.zeroize(); self.value.zeroize(); }
}

pub struct SessionBucket {
    origin: url::Origin,
    pairs: Vec<SessionPair>,
    _charge: Charge,
}

impl SessionBucket {
    pub fn seal(budget: &Arc<SleepingBudget>, origin: url::Origin, pairs: Vec<SessionPair>) -> Result<Arc<Self>, &'static str> {
        if !matches!(origin, url::Origin::Tuple(..)) || pairs.len() > 256
            || pairs.iter().any(|pair| pair.key.allocated_bytes() > 2048 || pair.value.allocated_bytes() > 8 * 1024)
        { return Err("stash-session-budget"); }
        let bytes = pairs.capacity() * size_of::<SessionPair>()
            + pairs.iter().map(|pair| pair.key.allocated_bytes() + pair.value.allocated_bytes()).sum::<usize>();
        if bytes > 32 * 1024 { return Err("stash-session-budget"); }
        let charge = budget.reserve(bytes + size_of::<Self>() + 2 * size_of::<usize>())?;
        Ok(Arc::new(Self { origin, pairs, _charge: charge }))
    }

    pub fn pairs(&self) -> &[SessionPair] { &self.pairs }
    pub fn into_pairs(mut self) -> Vec<SessionPair> { std::mem::take(&mut self.pairs) }

    fn allocated_bytes(&self) -> usize {
        self.pairs.capacity() * size_of::<SessionPair>()
            + self.pairs.iter().map(|pair| pair.key.allocated_bytes() + pair.value.allocated_bytes()).sum::<usize>()
    }
}

impl Drop for SessionBucket {
    fn drop(&mut self) { self.pairs.zeroize(); scrub_origin(&mut self.origin); }
}

pub struct ScrollState { pub x: f64, pub y: f64 }

impl Drop for ScrollState {
    fn drop(&mut self) { self.x.zeroize(); self.y.zeroize(); }
}

/// No Request, upload, authorization header, POST body or guest value.
pub struct PageStash {
    origin: url::Origin,
    route: Text,
    entry: u64,
    generation: u64,
    replayable: bool,
    forms: Vec<FormState>,
    session: Option<Arc<SessionBucket>>,
    scroll: ScrollState,
    _charge: Charge,
}

impl PageStash {
    pub fn seal(
        budget: &Arc<SleepingBudget>, url: &url::Url, entry: u64, generation: u64,
        replayable: bool, forms: Vec<FormState>, session: Option<Arc<SessionBucket>>,
        scroll: ScrollState, record_bytes: usize,
    ) -> Result<Arc<Self>, &'static str> {
        if !matches!(url.origin(), url::Origin::Tuple(..)) { return Err("stash-opaque-origin"); }
        if forms.len() > 256 || record_bytes > 32 * 1024 { return Err("stash-record-budget"); }
        let route = Text::capture(url.as_str().encode_utf16(), 32 * 1024).ok_or("stash-route-budget")?;
        let form_bytes = forms.capacity() * size_of::<FormState>()
            + forms.iter().map(FormState::allocated_bytes).sum::<usize>();
        let session_bytes = session.as_ref().map_or(0, |bucket| bucket.allocated_bytes());
        if form_bytes > 24 * 1024 || session_bytes > 32 * 1024
            || session.as_ref().is_some_and(|bucket| bucket.pairs.len() > 256 || bucket.origin != url.origin()
                || bucket.pairs.iter().any(|pair| pair.value.allocated_bytes() > 8 * 1024))
            || form_bytes + session_bytes > STATE_BYTES
        { return Err("stash-state-budget"); }
        let bytes = size_of::<Self>() + 2 * size_of::<usize>() + route.allocated_bytes()
            + form_bytes + record_bytes;
        let charge = budget.reserve(bytes)?;
        Ok(Arc::new(Self { origin: url.origin(), route, entry, generation, replayable,
            forms, session, scroll, _charge: charge }))
    }

    /// The caller moves this same Arc to the native restore owner.
    fn accepts(&self, destination: &url::Url, entry: u64) -> bool {
        self.entry == entry && self.origin == destination.origin()
            && self.route.units().iter().copied().eq(destination.as_str().encode_utf16())
    }

    pub fn restore(self: Arc<Self>, destination: &url::Url, entry: u64) -> Result<RestoreState, &'static str> {
        if !self.replayable { return Err("reload-requires-resubmission"); }
        if !self.accepts(destination, entry) { return Err("stash-origin-or-entry-mismatch"); }
        Ok(RestoreState(self))
    }
}

impl Drop for PageStash {
    fn drop(&mut self) { self.forms.zeroize(); scrub_origin(&mut self.origin); }
}

pub struct RestoreState(Arc<PageStash>);

impl RestoreState {
    pub fn generation(&self) -> u64 { self.0.generation }
    pub fn forms(&self) -> &[FormState] { &self.0.forms }
    pub fn scroll(&self) -> &ScrollState { &self.0.scroll }
    pub fn session(&self) -> Option<Arc<SessionBucket>> { self.0.session.as_ref().map(Arc::clone) }
}

/// Paint is a separate immutable owner, allowing preview-first eviction.
pub struct PaintSnapshot {
    pub width: u16,
    pub height: u16,
    pub captured_at_ns: u64,
    encoded: Bytes,
    _charge: Charge,
}

impl PaintSnapshot {
    pub fn seal(budget: &Arc<SleepingBudget>, width: u16, height: u16,
        captured_at_ns: u64, encoded: Bytes) -> Result<Arc<Self>, &'static str>
    {
        if width == 0 || height == 0 || width > 512 || height > 288
            || encoded.allocated_bytes() > PREVIEW_HARD
        { return Err("stash-preview-budget"); }
        let charge = budget.reserve(size_of::<Self>() + 2 * size_of::<usize>() + encoded.allocated_bytes())?;
        Ok(Arc::new(Self { width, height, captured_at_ns, encoded, _charge: charge }))
    }

    pub fn encoded(&self) -> &[u8] { self.encoded.bytes() }
}
