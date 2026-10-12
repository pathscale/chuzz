//! Persistent browser cookies.
//!
//! [`BrowserCookieStore`] keeps RFC cookie parsing and request matching in
//! `cookie_store`; WorkTable stores one ordered snapshot of the durable jar.
//! Session cookies stay in memory and therefore disappear when a new store is
//! opened.

use std::fmt;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard, mpsc};

use cookie_store::{
    Cookie as StoredCookie, CookieError, CookieStore as RfcCookieStore, StoreAction,
};
use url::Url;
use worktable::PersistedWorkTable;
use worktable::persistence::PersistenceEngine;
use worktable::prelude::*;
use worktable::worktable;

worktable!(
    name: BrowserCookie,
    version: 2,
    persist: true,
    columns: {
        id: u64 primary_key,
        cookies: String,
    },
);

type CookieKey = (String, String, String);

enum PersistenceCommand {
    Apply(u64),
    Flush {
        snapshot: Option<String>,
        reply: mpsc::Sender<Result<(), String>>,
    },
    Close {
        snapshot: Option<String>,
        reply: mpsc::Sender<Result<(), String>>,
    },
}

struct PendingSnapshot {
    wake: u64,
    value: String,
}

#[derive(Default)]
struct PendingSnapshots {
    next_wake: u64,
    pending: Option<PendingSnapshot>,
}

impl PendingSnapshots {
    fn replace(&mut self, value: String) -> Option<u64> {
        if let Some(pending) = &mut self.pending {
            pending.value = value;
            return None;
        }

        self.next_wake = self
            .next_wake
            .checked_add(1)
            .expect("cookie persistence wake id exhausted");
        let wake = self.next_wake;
        self.pending = Some(PendingSnapshot { wake, value });
        Some(wake)
    }

    fn take_for_wake(&mut self, wake: u64) -> Option<String> {
        // A Flush can take the snapshot while its older Apply is still queued.
        // The wake id keeps that stale Apply from stealing a mutation queued
        // on the far side of the barrier.
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.wake == wake)
        {
            self.take()
        } else {
            None
        }
    }

    fn take(&mut self) -> Option<String> {
        self.pending.take().map(|pending| pending.value)
    }
}

struct CookiePersistence {
    sender: mpsc::Sender<PersistenceCommand>,
    // Cookie callbacks are synchronous, so avoid putting every snapshot on the
    // control queue. Keep one replaceable snapshot beside it instead: a
    // mutation burst then costs one pending jar and one wake, while Flush and
    // Close remain FIFO barriers after that wake.
    latest: Arc<Mutex<PendingSnapshots>>,
}

impl CookiePersistence {
    fn start(table: BrowserCookieWorkTable) -> Self {
        let (sender, receiver) = mpsc::channel();
        let latest = Arc::new(Mutex::new(PendingSnapshots::default()));
        let worker_latest = Arc::clone(&latest);
        std::thread::Builder::new()
            .name("chuzz-cookie-persistence".into())
            .spawn(move || {
                let mut table = Some(table);
                let mut first_error: Option<String> = None;
                while let Ok(command) = receiver.recv() {
                    match command {
                        PersistenceCommand::Apply(wake) => {
                            let snapshot = worker_latest
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner())
                                .take_for_wake(wake);
                            nagoya::block_on(record_durable_snapshot(
                                table.as_ref().expect("the cookie table is open"),
                                snapshot,
                                &mut first_error,
                            ));
                        }
                        PersistenceCommand::Flush { snapshot, reply } => {
                            nagoya::block_on(record_durable_snapshot(
                                table.as_ref().expect("the cookie table is open"),
                                snapshot,
                                &mut first_error,
                            ));
                            let drain = nagoya::block_on(
                                table
                                    .as_ref()
                                    .expect("the cookie table is open")
                                    .wait_for_ops(),
                            )
                            .map_err(|error| error.to_string());
                            let result = first_error.clone().map_or(drain, Err);
                            let _ = reply.send(result);
                        }
                        PersistenceCommand::Close { snapshot, reply } => {
                            nagoya::block_on(record_durable_snapshot(
                                table.as_ref().expect("the cookie table is open"),
                                snapshot,
                                &mut first_error,
                            ));
                            let close = nagoya::block_on(
                                table.take().expect("the cookie table is open").close(),
                            )
                            .map_err(|error| error.to_string());
                            let result = first_error.clone().map_or(close, Err);
                            let _ = reply.send(result);
                            return;
                        }
                    }
                }
                if let Some(table) = table {
                    let _ = nagoya::block_on(table.close());
                }
            })
            .expect("cookie persistence worker thread");
        Self { sender, latest }
    }

    fn enqueue(&self, snapshot: String) -> Result<(), CookieStoreError> {
        let mut latest = self
            .latest
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let wake = latest.replace(snapshot);
        if let Some(wake) = wake
            && self.sender.send(PersistenceCommand::Apply(wake)).is_err()
        {
            // Keep the slot locked across the nonblocking send. Otherwise
            // another callback can replace this unsent generation, return
            // success, and leave the slot permanently occupied after the
            // worker has stopped.
            let _ = latest.take_for_wake(wake);
            return Err(CookieStoreError::Persistence(
                "cookie persistence worker stopped".into(),
            ));
        }
        Ok(())
    }

    fn request_flush(&self) -> Result<mpsc::Receiver<Result<(), String>>, CookieStoreError> {
        let (reply, result) = mpsc::channel();
        let snapshot = self.take_pending();
        self.sender
            .send(PersistenceCommand::Flush { snapshot, reply })
            .map_err(|_| {
                CookieStoreError::Persistence("cookie persistence worker stopped".into())
            })?;
        Ok(result)
    }

    async fn close(self) -> Result<(), CookieStoreError> {
        let (reply, result) = mpsc::channel();
        let snapshot = self.take_pending();
        self.sender
            .send(PersistenceCommand::Close { snapshot, reply })
            .map_err(|_| {
                CookieStoreError::Persistence("cookie persistence worker stopped".into())
            })?;
        Self::wait_for_reply(result).await
    }

    async fn wait_for_reply(
        receiver: mpsc::Receiver<Result<(), String>>,
    ) -> Result<(), CookieStoreError> {
        nagoya::spawn(async move { receiver.recv() })
            .await
            .ok_or_else(|| {
                CookieStoreError::Persistence("cookie persistence worker stopped".into())
            })?
            .map_err(|_| CookieStoreError::Persistence("cookie persistence worker stopped".into()))?
            .map_err(CookieStoreError::Persistence)
    }

    fn take_pending(&self) -> Option<String> {
        self.latest
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
    }
}

/// The profile directory shared by the window, captures and the inspection
/// host.
///
/// `CHUZZ_PROFILE_DIR` gives isolated runs a profile without changing the
/// user's browser state.  Ordinary launches use the platform's application
/// data location.
pub fn profile_directory() -> std::path::PathBuf {
    if let Some(path) = std::env::var_os("CHUZZ_PROFILE_DIR") {
        return path.into();
    }

    #[cfg(target_os = "macos")]
    if let Some(home) = std::env::var_os("HOME") {
        return std::path::PathBuf::from(home).join("Library/Application Support/ai.chuzz.browser");
    }

    #[cfg(target_os = "windows")]
    if let Some(data) = std::env::var_os("APPDATA") {
        return std::path::PathBuf::from(data).join("ai.chuzz.browser");
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        if let Some(data) = std::env::var_os("XDG_DATA_HOME") {
            return std::path::PathBuf::from(data).join("ai.chuzz.browser");
        }
        if let Some(home) = std::env::var_os("HOME") {
            return std::path::PathBuf::from(home).join(".local/share/ai.chuzz.browser");
        }
    }

    std::env::temp_dir().join("ai.chuzz.browser")
}

/// Counts response cookie fields that were accepted or ignored.
///
/// Browsers ignore an invalid `Set-Cookie` field without rejecting the whole
/// response, so individual parse and policy failures are reported as a count.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StoreReport {
    pub accepted: usize,
    pub rejected: usize,
}

/// A cookie codec or persistence error.
#[derive(Debug)]
pub enum CookieStoreError {
    Open(String),
    Codec(serde_json::Error),
    Table(WorkTableError),
    Persistence(String),
}

impl fmt::Display for CookieStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Open(error) => write!(formatter, "could not open cookie store: {error}"),
            Self::Codec(error) => write!(formatter, "could not encode cookie snapshot: {error}"),
            Self::Table(error) => write!(formatter, "cookie table mutation failed: {error}"),
            Self::Persistence(error) => {
                write!(formatter, "cookie persistence worker failed: {error}")
            }
        }
    }
}

impl std::error::Error for CookieStoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Codec(error) => Some(error),
            Self::Table(error) => Some(error),
            Self::Open(_) | Self::Persistence(_) => None,
        }
    }
}

impl From<serde_json::Error> for CookieStoreError {
    fn from(error: serde_json::Error) -> Self {
        Self::Codec(error)
    }
}

impl From<WorkTableError> for CookieStoreError {
    fn from(error: WorkTableError) -> Self {
        Self::Table(error)
    }
}

/// A browser cookie jar backed by a persisted WorkTable.
///
/// Mutations are serialized through `mutation_gate`, so two response paths
/// cannot commit older state after newer state for the same cookie.  Reads use
/// an ordinary `RwLock` because request construction and `document.cookie`
/// getters are synchronous browser operations.
pub struct BrowserCookieStore {
    persistence: Option<CookiePersistence>,
    cookies: RwLock<RfcCookieStore>,
    mutation_gate: Mutex<()>,
}

impl Default for BrowserCookieStore {
    /// Builds a process-local cookie jar.  Production profiles should call
    /// [`Self::open`] with their profile directory.
    fn default() -> Self {
        Self {
            persistence: None,
            cookies: RwLock::new(RfcCookieStore::default()),
            mutation_gate: Mutex::new(()),
        }
    }
}

impl BrowserCookieStore {
    /// Opens the cookie table below `directory`, creating an empty table when
    /// the directory has no cookie data yet.
    pub async fn open(directory: impl AsRef<Path>) -> Result<Self, CookieStoreError> {
        std::fs::create_dir_all(directory.as_ref())
            .map_err(|error| CookieStoreError::Open(error.to_string()))?;
        let directory = directory.as_ref().to_string_lossy().into_owned();
        let config = DiskConfig::new_with_table_name(
            directory,
            BrowserCookieWorkTable::name_snake_case(),
            BrowserCookieWorkTable::version(),
        );
        let engine = BrowserCookiePersistenceEngine::new(config)
            .await
            .map_err(|error| CookieStoreError::Open(format!("{error:#}")))?;
        let table = BrowserCookieWorkTable::load(engine)
            .await
            .map_err(|error| CookieStoreError::Open(format!("{error:#}")))?;

        let joined = read_durable_snapshot(&table);
        let stored = if joined.is_empty() {
            Vec::new()
        } else {
            match serde_json::from_str::<Vec<StoredCookie<'static>>>(&joined) {
                Ok(stored) => stored,
                Err(error) => {
                    // A jar torn between slice writes must not lock the
                    // profile out of cookies; it starts empty and is rewritten.
                    eprintln!("chuzz: stored cookies were unreadable, starting empty: {error}");
                    Vec::new()
                }
            }
        };
        let stored_count = stored.len();
        let loaded = stored
            .into_iter()
            .filter(|cookie| cookie.is_persistent() && !cookie.is_expired())
            .collect::<Vec<_>>();

        if loaded.len() != stored_count
            && let Some(error) =
                apply_durable_snapshot(&table, serde_json::to_string(&loaded)?).await
        {
            return Err(CookieStoreError::Open(error));
        }

        let cookies = RfcCookieStore::from_cookies(
            loaded
                .into_iter()
                .map(Ok::<StoredCookie<'static>, std::convert::Infallible>),
            false,
        )
        .unwrap_or_else(|never| match never {});

        Ok(Self {
            persistence: Some(CookiePersistence::start(table)),
            cookies: RwLock::new(cookies),
            mutation_gate: Mutex::new(()),
        })
    }

    /// Adds `Set-Cookie` response values received from `url`.
    ///
    /// Invalid fields are ignored independently.  A valid expiry field removes
    /// the matching cookie, and a session cookie replaces any durable value
    /// with an in-memory-only value.
    pub fn add_response_cookies<I, S>(
        &self,
        url: &Url,
        values: I,
    ) -> Result<StoreReport, CookieStoreError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let _mutation = self.lock_mutations();
        let mut report = StoreReport::default();

        let snapshot = {
            let mut store = self.write_cookies();
            let mut changed = false;
            for value in values {
                let Ok(mut cookie) = StoredCookie::parse(value.as_ref().to_owned(), url) else {
                    report.rejected += 1;
                    continue;
                };
                if cookie.name().is_empty()
                    || oversized_cookie(&cookie)
                    || (cookie.secure().unwrap_or(false) && !is_secure_url(url))
                    || !valid_cookie_prefix(&cookie, url)
                    || !enforce_cookie_domain(&mut cookie, url)
                {
                    report.rejected += 1;
                    continue;
                }
                let key = cookie_key(&cookie);

                match store.insert(cookie, url) {
                    Ok(StoreAction::Inserted | StoreAction::UpdatedExisting) => {
                        changed = true;
                        report.accepted += 1;
                    }
                    Ok(StoreAction::ExpiredExisting) => {
                        store.remove(&key.0, &key.1, &key.2);
                        changed = true;
                        report.accepted += 1;
                    }
                    Err(CookieError::Expired) => {
                        report.accepted += 1;
                    }
                    Err(_) => {
                        report.rejected += 1;
                    }
                }
            }
            changed.then(|| durable_snapshot(&store)).transpose()?
        };

        if let Some(snapshot) = snapshot {
            self.enqueue_snapshot(snapshot)?;
        }
        Ok(report)
    }

    /// Returns the value for an outgoing HTTP `Cookie` header.
    pub fn cookie_header(&self, url: &Url) -> Option<String> {
        let store = self.read_cookies();
        let mut matches = store.matches(url);
        matches.sort_by(|left, right| {
            right
                .path
                .len()
                .cmp(&left.path.len())
                .then_with(|| left.name().cmp(right.name()))
        });
        let header = matches
            .into_iter()
            .map(|cookie| format!("{}={}", cookie.name(), cookie.value()))
            .collect::<Vec<_>>()
            .join("; ");
        (!header.is_empty()).then_some(header)
    }

    /// Returns the cookies visible through a page's `document.cookie` getter.
    ///
    /// `HttpOnly` cookies still participate in HTTP requests but are omitted
    /// here even for an HTTP(S) page.
    pub fn script_cookies(&self, url: &Url) -> String {
        let store = self.read_cookies();
        let mut matches = store
            .matches(url)
            .into_iter()
            .filter(|cookie| !cookie.http_only().unwrap_or(false))
            .collect::<Vec<_>>();
        matches.sort_by(|left, right| {
            right
                .path
                .len()
                .cmp(&left.path.len())
                .then_with(|| left.name().cmp(right.name()))
        });
        matches
            .into_iter()
            .map(|cookie| format!("{}={}", cookie.name(), cookie.value()))
            .collect::<Vec<_>>()
            .join("; ")
    }

    /// Applies one `document.cookie` assignment.
    ///
    /// Returns `true` when the assignment was accepted.  Script cannot create
    /// an `HttpOnly` cookie or replace an existing one, and a secure cookie is
    /// ignored on a non-secure page.
    pub fn set_script_cookie(&self, url: &Url, assignment: &str) -> Result<bool, CookieStoreError> {
        let _mutation = self.lock_mutations();
        let Ok(mut cookie) = StoredCookie::parse(assignment.to_owned(), url) else {
            return Ok(false);
        };
        let key = cookie_key(&cookie);

        if cookie.name().is_empty()
            || oversized_cookie(&cookie)
            || cookie.http_only().unwrap_or(false)
            || (cookie.secure().unwrap_or(false) && !is_secure_url(url))
            || !valid_cookie_prefix(&cookie, url)
            || !enforce_cookie_domain(&mut cookie, url)
        {
            return Ok(false);
        }

        let snapshot = {
            let mut store = self.write_cookies();
            if store
                .get_any(&key.0, &key.1, &key.2)
                .is_some_and(|existing| existing.http_only().unwrap_or(false))
            {
                return Ok(false);
            }
            match store.insert(cookie, url) {
                Ok(StoreAction::Inserted | StoreAction::UpdatedExisting) => {
                    Some(durable_snapshot(&store)?)
                }
                Ok(StoreAction::ExpiredExisting) => {
                    store.remove(&key.0, &key.1, &key.2);
                    Some(durable_snapshot(&store)?)
                }
                Err(CookieError::Expired) => None,
                Err(_) => return Ok(false),
            }
        };

        if let Some(snapshot) = snapshot {
            self.enqueue_snapshot(snapshot)?;
        }
        Ok(true)
    }

    /// Waits for all cookie mutations currently queued by WorkTable.
    pub async fn flush(&self) -> Result<(), CookieStoreError> {
        let result = {
            let _mutation = self.lock_mutations();
            self.persistence
                .as_ref()
                .map(CookiePersistence::request_flush)
                .transpose()?
        };
        let Some(result) = result else {
            return Ok(());
        };
        CookiePersistence::wait_for_reply(result).await
    }

    /// Stops the WorkTable persistence worker after draining queued mutations.
    pub async fn close(self) -> Result<(), CookieStoreError> {
        if let Some(persistence) = self.persistence {
            persistence.close().await?;
        }
        Ok(())
    }

    fn enqueue_snapshot(&self, snapshot: String) -> Result<(), CookieStoreError> {
        self.persistence
            .as_ref()
            .map_or(Ok(()), |persistence| persistence.enqueue(snapshot))
    }

    fn lock_mutations(&self) -> MutexGuard<'_, ()> {
        self.mutation_gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn read_cookies(&self) -> RwLockReadGuard<'_, RfcCookieStore> {
        self.cookies
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn write_cookies(&self) -> RwLockWriteGuard<'_, RfcCookieStore> {
        self.cookies
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl blitz_traits::net::CookieJar for BrowserCookieStore {
    fn cookies_header(&self, url: &Url) -> Option<blitz_traits::net::http::HeaderValue> {
        self.cookie_header(url)
            .and_then(|value| blitz_traits::net::http::HeaderValue::from_str(&value).ok())
    }

    fn store_from_response(&self, url: &Url, headers: &blitz_traits::net::http::HeaderMap) {
        let values = headers
            .get_all(blitz_traits::net::http::header::SET_COOKIE)
            .iter()
            .filter_map(|header| header.to_str().ok().map(str::to_owned))
            .collect::<Vec<_>>();
        if let Err(error) = self.add_response_cookies(url, values) {
            eprintln!("chuzz: could not queue response cookies: {error}");
        }
    }
}

/// The jar is stored as consecutive rows of at most this many bytes.
///
/// A worktable row has to fit one data page (about 12 KB), and a whole jar
/// outgrows that after a few sites, so one row per jar failed every flush.
const SNAPSHOT_SLICE: usize = 8 * 1024;

fn snapshot_slices(snapshot: &str) -> Vec<&str> {
    let mut slices = Vec::new();
    let mut rest = snapshot;
    while rest.len() > SNAPSHOT_SLICE {
        let mut end = SNAPSHOT_SLICE;
        while !rest.is_char_boundary(end) {
            end -= 1;
        }
        let (slice, tail) = rest.split_at(end);
        slices.push(slice);
        rest = tail;
    }
    slices.push(rest);
    slices
}

fn read_durable_snapshot(table: &BrowserCookieWorkTable) -> String {
    let mut joined = String::new();
    let mut id = 0;
    while let Some(row) = table.select(id) {
        joined.push_str(&row.cookies);
        id += 1;
    }
    joined
}

async fn apply_durable_snapshot(
    table: &BrowserCookieWorkTable,
    snapshot: String,
) -> Option<String> {
    let slices = snapshot_slices(&snapshot);
    for (id, slice) in (0u64..).zip(slices.iter()) {
        if let Err(error) = table
            .upsert(BrowserCookieRow {
                id,
                cookies: (*slice).to_owned(),
            })
            .await
        {
            return Some(error.to_string());
        }
    }
    // A shorter jar leaves the previous tail behind; reading stops at the
    // first missing id, so every row after the new end has to go.
    let mut stale = slices.len() as u64;
    while table.select(stale).is_some() {
        if let Err(error) = table.delete(stale).await {
            return Some(error.to_string());
        }
        stale += 1;
    }
    None
}

async fn record_durable_snapshot(
    table: &BrowserCookieWorkTable,
    snapshot: Option<String>,
    first_error: &mut Option<String>,
) {
    let Some(snapshot) = snapshot else {
        return;
    };
    let error = apply_durable_snapshot(table, snapshot).await;
    if first_error.is_none() {
        *first_error = error;
    }
}

fn cookie_key(cookie: &StoredCookie<'_>) -> CookieKey {
    (
        String::from(&cookie.domain),
        String::from(&cookie.path),
        cookie.name().to_owned(),
    )
}

fn durable_snapshot(store: &RfcCookieStore) -> Result<String, serde_json::Error> {
    serde_json::to_string(
        &store
            .iter_any()
            .filter(|cookie| cookie.is_persistent() && !cookie.is_expired())
            .collect::<Vec<_>>(),
    )
}

/// RFC 6265bis and every shipping browser cap the name plus value of a cookie
/// at 4096 bytes and ignore the whole Set-Cookie line beyond that.
const MAX_COOKIE_NAME_VALUE: usize = 4096;

fn oversized_cookie(cookie: &StoredCookie<'_>) -> bool {
    cookie.name().len() + cookie.value().len() > MAX_COOKIE_NAME_VALUE
}

fn is_secure_url(url: &Url) -> bool {
    matches!(url.scheme(), "https" | "wss")
}

fn valid_cookie_prefix(cookie: &StoredCookie<'_>, url: &Url) -> bool {
    if cookie.name().starts_with("__Secure-") {
        return cookie.secure() == Some(true) && is_secure_url(url);
    }
    if cookie.name().starts_with("__Host-") {
        return cookie.secure() == Some(true)
            && is_secure_url(url)
            && cookie.domain().is_none()
            && cookie.path() == Some("/");
    }
    true
}

/// Reject a Domain attribute naming an ICANN or private public suffix.
///
/// `cookie_store` exposes PSL enforcement but deliberately ships without list
/// data. Chuzz uses the compiled Mozilla list so a response from one site
/// cannot set cookies for every registrable site below the same suffix.
fn enforce_cookie_domain(cookie: &mut StoredCookie<'_>, url: &Url) -> bool {
    let Some(domain) = cookie.domain() else {
        return true;
    };
    let Some(suffix) = psl::suffix(domain.as_bytes()) else {
        return true;
    };
    if suffix.typ().is_none() || !suffix.as_bytes().eq_ignore_ascii_case(domain.as_bytes()) {
        return true;
    }

    // RFC 6265 treats an exact request-host/public-suffix match as host-only.
    let Some(host) = url.host_str() else {
        return false;
    };
    if !host.eq_ignore_ascii_case(domain) {
        return false;
    }
    cookie.domain = cookie_store::CookieDomain::HostOnly(host.to_owned());
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(value: &str) -> Url {
        Url::parse(value).expect("test URL parses")
    }

    #[test]
    fn foreign_domain_expiry_cannot_delete_an_existing_cookie() {
        let store = BrowserCookieStore::default();
        let owner = url("https://account.test.example/");
        store
            .add_response_cookies(&owner, ["session=owner; Domain=test.example; Path=/"])
            .expect("owner cookie stored");

        let foreign = url("https://foreign.invalid/");
        let report = store
            .add_response_cookies(
                &foreign,
                ["session=gone; Domain=test.example; Path=/; Max-Age=0"],
            )
            .expect("foreign field checked");

        assert_eq!(
            report,
            StoreReport {
                accepted: 0,
                rejected: 1
            }
        );
        assert_eq!(
            store.cookie_header(&owner),
            Some("session=owner".to_owned())
        );
        assert!(
            !store
                .set_script_cookie(
                    &foreign,
                    "session=gone; Domain=test.example; Path=/; Max-Age=0",
                )
                .expect("foreign script field checked")
        );
        assert_eq!(
            store.cookie_header(&owner),
            Some("session=owner".to_owned())
        );
    }

    #[test]
    fn public_suffix_domain_is_rejected() {
        let store = BrowserCookieStore::default();
        let page = url("https://account.example.com/");
        let report = store
            .add_response_cookies(&page, ["cross_site=bad; Domain=com; Path=/"])
            .expect("response field checked");

        assert_eq!(
            report,
            StoreReport {
                accepted: 0,
                rejected: 1
            }
        );
        assert_eq!(store.cookie_header(&page), None);
    }

    #[test]
    fn a_stale_wake_cannot_take_a_snapshot_after_a_barrier() {
        let mut pending = PendingSnapshots::default();
        let first_wake = pending
            .replace("first".to_owned())
            .expect("an empty slot queues a wake");
        assert_eq!(pending.replace("before barrier".to_owned()), None);
        assert_eq!(pending.take().as_deref(), Some("before barrier"));

        let second_wake = pending
            .replace("after barrier".to_owned())
            .expect("the emptied slot queues a new wake");
        assert_eq!(pending.take_for_wake(first_wake), None);
        assert_eq!(
            pending.take_for_wake(second_wake).as_deref(),
            Some("after barrier")
        );
    }

    #[test]
    fn a_stopped_worker_does_not_leave_an_unsent_snapshot_pending() {
        let (sender, receiver) = std::sync::mpsc::channel();
        drop(receiver);
        let persistence = CookiePersistence {
            sender,
            latest: Arc::new(Mutex::new(PendingSnapshots::default())),
        };

        for value in ["first", "retry"] {
            assert!(matches!(
                persistence.enqueue(value.to_owned()),
                Err(CookieStoreError::Persistence(message))
                    if message == "cookie persistence worker stopped"
            ));
            assert!(
                persistence
                    .latest
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .pending
                    .is_none()
            );
        }
    }
}
