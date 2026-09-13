//! Persistent browser cookies.
//!
//! [`BrowserCookieStore`] keeps RFC cookie parsing and request matching in
//! `cookie_store`; WorkTable stores one ordered snapshot of the durable jar.
//! Session cookies stay in memory and therefore disappear when a new store is
//! opened.

use std::fmt;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};

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
    Apply,
    Flush(tokio::sync::oneshot::Sender<Result<(), String>>),
    Close(tokio::sync::oneshot::Sender<Result<(), String>>),
}

struct CookiePersistence {
    sender: tokio::sync::mpsc::UnboundedSender<PersistenceCommand>,
    // Cookie callbacks are synchronous, so they cannot wait on a bounded
    // async channel. Keep one replaceable snapshot beside the ordered control
    // queue instead: a mutation burst then costs one pending jar and one wake,
    // while Flush and Close remain FIFO barriers after that wake.
    latest: Arc<Mutex<Option<String>>>,
}

impl CookiePersistence {
    fn start(table: BrowserCookieWorkTable) -> Self {
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let latest = Arc::new(Mutex::new(None));
        let worker_latest = Arc::clone(&latest);
        tokio::spawn(async move {
            let mut table = Some(table);
            let mut first_error: Option<String> = None;
            while let Some(command) = receiver.recv().await {
                match command {
                    PersistenceCommand::Apply => {
                        let snapshot = worker_latest
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .take();
                        if let Some(snapshot) = snapshot {
                            let error = apply_durable_snapshot(
                                table.as_ref().expect("the cookie table is open"),
                                snapshot,
                            )
                            .await;
                            if first_error.is_none() {
                                first_error = error;
                            }
                        }
                    }
                    PersistenceCommand::Flush(reply) => {
                        let drain = table
                            .as_ref()
                            .expect("the cookie table is open")
                            .wait_for_ops()
                            .await
                            .map_err(|error| error.to_string());
                        let result = first_error.clone().map_or(drain, Err);
                        let _ = reply.send(result);
                    }
                    PersistenceCommand::Close(reply) => {
                        let close = table
                            .take()
                            .expect("the cookie table is open")
                            .close()
                            .await
                            .map_err(|error| error.to_string());
                        let result = first_error.clone().map_or(close, Err);
                        let _ = reply.send(result);
                        return;
                    }
                }
            }
            if let Some(table) = table {
                let _ = table.close().await;
            }
        });
        Self { sender, latest }
    }

    fn enqueue(&self, snapshot: String) -> Result<(), CookieStoreError> {
        let notify = {
            let mut latest = self
                .latest
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let notify = latest.is_none();
            *latest = Some(snapshot);
            notify
        };
        if notify {
            self.sender.send(PersistenceCommand::Apply).map_err(|_| {
                CookieStoreError::Persistence("cookie persistence worker stopped".into())
            })?;
        }
        Ok(())
    }

    fn request_flush(
        &self,
    ) -> Result<tokio::sync::oneshot::Receiver<Result<(), String>>, CookieStoreError> {
        let (reply, result) = tokio::sync::oneshot::channel();
        self.sender
            .send(PersistenceCommand::Flush(reply))
            .map_err(|_| {
                CookieStoreError::Persistence("cookie persistence worker stopped".into())
            })?;
        Ok(result)
    }

    async fn close(self) -> Result<(), CookieStoreError> {
        let (reply, result) = tokio::sync::oneshot::channel();
        self.sender
            .send(PersistenceCommand::Close(reply))
            .map_err(|_| {
                CookieStoreError::Persistence("cookie persistence worker stopped".into())
            })?;
        result
            .await
            .map_err(|_| CookieStoreError::Persistence("cookie persistence worker stopped".into()))?
            .map_err(CookieStoreError::Persistence)
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

        let stored = table
            .select(0)
            .map(|row| serde_json::from_str::<Vec<StoredCookie<'static>>>(&row.cookies))
            .transpose()?
            .unwrap_or_default();
        let stored_count = stored.len();
        let loaded = stored
            .into_iter()
            .filter(|cookie| cookie.is_persistent() && !cookie.is_expired())
            .collect::<Vec<_>>();

        if loaded.len() != stored_count {
            table
                .upsert(BrowserCookieRow {
                    id: 0,
                    cookies: serde_json::to_string(&loaded)?,
                })
                .await?;
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
        result
            .await
            .map_err(|_| CookieStoreError::Persistence("cookie persistence worker stopped".into()))?
            .map_err(CookieStoreError::Persistence)
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

impl blitz_net::CookieStore for BrowserCookieStore {
    fn set_cookies(
        &self,
        cookie_headers: &mut dyn Iterator<Item = &blitz_traits::net::http::HeaderValue>,
        url: &Url,
    ) {
        let values = cookie_headers
            .filter_map(|header| header.to_str().ok().map(str::to_owned))
            .collect::<Vec<_>>();
        if let Err(error) = self.add_response_cookies(url, values) {
            eprintln!("chuzz: could not queue response cookies: {error}");
        }
    }

    fn cookies(&self, url: &Url) -> Option<blitz_traits::net::http::HeaderValue> {
        self.cookie_header(url)
            .and_then(|value| blitz_traits::net::http::HeaderValue::from_str(&value).ok())
    }
}

async fn apply_durable_snapshot(
    table: &BrowserCookieWorkTable,
    snapshot: String,
) -> Option<String> {
    table
        .upsert(BrowserCookieRow {
            id: 0,
            cookies: snapshot,
        })
        .await
        .err()
        .map(|error| error.to_string())
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
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn url(value: &str) -> Url {
        Url::parse(value).expect("valid test URL")
    }

    fn test_directory(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "chuzz-cookie-store-{label}-{}-{nonce}",
            std::process::id()
        ))
    }

    #[tokio::test]
    async fn response_cookie_matches_domain_path_and_security_rules() {
        let store = BrowserCookieStore::default();
        let response_url = url("https://account.test.example/app/start");
        let report = store
            .add_response_cookies(
                &response_url,
                [
                    "host=one; Path=/; Secure; HttpOnly",
                    "section=two; Path=/app",
                    "shared=three; Domain=test.example; Path=/",
                ],
            )
            .expect("cookies stored");
        assert_eq!(
            report,
            StoreReport {
                accepted: 3,
                rejected: 0
            }
        );

        assert_eq!(
            store.cookie_header(&url("https://account.test.example/app/page")),
            Some("section=two; host=one; shared=three".to_string())
        );
        assert_eq!(
            store.script_cookies(&url("https://account.test.example/app/page")),
            "section=two; shared=three"
        );
        assert_eq!(
            store.cookie_header(&url("https://other.test.example/")),
            Some("shared=three".to_string())
        );
        assert_eq!(
            store.cookie_header(&url("http://account.test.example/")),
            Some("shared=three".to_string())
        );
    }

    #[tokio::test]
    async fn script_assignment_cannot_observe_or_replace_http_only_cookie() {
        let store = BrowserCookieStore::default();
        let page = url("https://private.test.example/");
        store
            .add_response_cookies(&page, ["token=server; Path=/; HttpOnly"])
            .expect("response cookie stored");

        assert!(
            !store
                .set_script_cookie(&page, "token=script; Path=/")
                .expect("assignment checked")
        );
        assert_eq!(store.script_cookies(&page), "");
        assert_eq!(store.cookie_header(&page), Some("token=server".to_string()));
    }

    #[tokio::test]
    async fn expiry_removes_a_cookie_and_invalid_fields_do_not_stop_the_batch() {
        let store = BrowserCookieStore::default();
        let page = url("https://www.test.example/area");
        store
            .add_response_cookies(&page, ["theme=dark; Path=/"])
            .expect("initial cookie stored");
        let report = store
            .add_response_cookies(
                &page,
                [
                    "theme=gone; Path=/; Max-Age=0",
                    "broken; Domain=elsewhere.invalid",
                ],
            )
            .expect("batch processed");

        assert_eq!(
            report,
            StoreReport {
                accepted: 1,
                rejected: 1
            }
        );
        assert_eq!(store.cookie_header(&page), None);

        let suffix_host = url("https://com/");
        let report = store
            .add_response_cookies(&suffix_host, ["host_only=good; Domain=com; Path=/"])
            .expect("exact-host response field checked");
        assert_eq!(report.accepted, 1);
        assert_eq!(
            store.cookie_header(&suffix_host),
            Some("host_only=good".to_owned())
        );
        assert_eq!(store.cookie_header(&page), None);
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

    #[tokio::test]
    async fn script_cookie_prefixes_require_their_security_invariants() {
        let store = BrowserCookieStore::default();
        let secure = url("https://www.test.example/");
        assert!(
            !store
                .set_script_cookie(&secure, "__Secure-id=one; Path=/")
                .expect("assignment checked")
        );
        assert!(
            !store
                .set_script_cookie(
                    &secure,
                    "__Host-id=one; Secure; Domain=test.example; Path=/",
                )
                .expect("assignment checked")
        );
        assert!(
            store
                .set_script_cookie(&secure, "__Host-id=one; Secure; Path=/")
                .expect("assignment accepted")
        );

        let report = store
            .add_response_cookies(
                &secure,
                [
                    "__Secure-response=bad; Path=/",
                    "__Host-response=bad; Secure; Domain=test.example; Path=/",
                    "__Host-response=good; Secure; Path=/",
                ],
            )
            .expect("response fields checked");
        assert_eq!(
            report,
            StoreReport {
                accepted: 1,
                rejected: 2
            }
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

    #[tokio::test]
    async fn persistent_cookies_survive_reopen_and_session_cookies_do_not() {
        let directory = test_directory("reopen");
        let page = url("https://profile.test.example/");
        {
            let store = BrowserCookieStore::open(&directory)
                .await
                .expect("cookie table opens");
            store
                .add_response_cookies(
                    &page,
                    ["durable=yes; Path=/; Max-Age=3600", "session=yes; Path=/"],
                )
                .expect("cookies stored");
            store.close().await.expect("cookie table closes");
        }

        {
            let store = BrowserCookieStore::open(&directory)
                .await
                .expect("cookie table reopens");
            assert_eq!(store.cookie_header(&page), Some("durable=yes".to_string()));
            store.close().await.expect("cookie table closes");
        }
        std::fs::remove_dir_all(directory).expect("test cookie directory removed");
    }

    #[tokio::test]
    async fn a_mutation_burst_closes_with_its_latest_snapshot() {
        let directory = test_directory("burst");
        let page = url("https://profile.test.example/");
        {
            let store = BrowserCookieStore::open(&directory)
                .await
                .expect("cookie table opens");
            for value in 0..512 {
                store
                    .add_response_cookies(&page, [format!("counter={value}; Path=/; Max-Age=3600")])
                    .expect("cookie update queued");
            }
            store.close().await.expect("cookie table closes");
        }

        let store = BrowserCookieStore::open(&directory)
            .await
            .expect("cookie table reopens");
        assert_eq!(store.cookie_header(&page), Some("counter=511".to_owned()));
        store.close().await.expect("cookie table closes");
        std::fs::remove_dir_all(directory).expect("test cookie directory removed");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn provider_redirect_and_error_cookies_persist_in_exchange_order() {
        use blitz_traits::net::Request;
        use std::io::{Read, Write};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("loopback listener binds");
        let port = listener.local_addr().expect("listener has address").port();
        let server = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for index in 0..2 {
                let (mut stream, _) = listener.accept().expect("request arrives");
                let mut head = Vec::new();
                let mut byte = [0_u8; 1];
                while !head.ends_with(b"\r\n\r\n") {
                    match stream.read(&mut byte) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => head.push(byte[0]),
                    }
                }
                requests.push(String::from_utf8_lossy(&head).to_ascii_lowercase());
                let response = if index == 0 {
                    "HTTP/1.1 302 Found\r\nLocation: /finish\r\nSet-Cookie: redirected=one; Path=/; Max-Age=3600\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                } else {
                    "HTTP/1.1 418 I'm a teapot\r\nSet-Cookie: final=two; Path=/; Max-Age=3600\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                };
                stream
                    .write_all(response.as_bytes())
                    .expect("response writes");
            }
            requests
        });

        let directory = test_directory("provider-redirect");
        let cookies = Arc::new(
            BrowserCookieStore::open(&directory)
                .await
                .expect("cookie table opens"),
        );
        let provider = blitz_net::Provider::with_user_agent_and_cookie_provider(
            None,
            "FixtureBrowser/1.0",
            Arc::clone(&cookies),
        );
        let origin = format!("http://127.0.0.1:{port}");
        let result = provider
            .fetch_response_async(Request::get(
                Url::parse(&format!("{origin}/start")).expect("request URL parses"),
            ))
            .await;
        assert!(matches!(
            result,
            Err(blitz_net::ProviderError::HttpStatus { status, .. }) if status.as_u16() == 418
        ));

        let requests = server.join().expect("server finishes");
        assert!(!requests[0].contains("cookie:"));
        assert!(requests[1].contains("cookie: redirected=one"));
        let page = Url::parse(&format!("{origin}/finish")).expect("page URL parses");
        assert_eq!(
            cookies.cookie_header(&page),
            Some("final=two; redirected=one".to_owned())
        );

        drop(provider);
        let cookies = Arc::try_unwrap(cookies).unwrap_or_else(|_| panic!("provider released jar"));
        cookies.close().await.expect("cookie table closes");
        let reopened = BrowserCookieStore::open(&directory)
            .await
            .expect("cookie table reopens");
        assert_eq!(
            reopened.cookie_header(&page),
            Some("final=two; redirected=one".to_owned())
        );
        reopened.close().await.expect("cookie table closes");
        std::fs::remove_dir_all(directory).expect("test cookie directory removed");
    }
}
