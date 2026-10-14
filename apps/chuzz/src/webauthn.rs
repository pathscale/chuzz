//! WebAuthn client policy over one process-wide CTAP2 platform authenticator.
//!
//! The page thread validates requests and takes notified Arc results. Secure
//! Enclave signing and credential persistence run on a blocking host thread.

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use ctap2::cbor::{Out, Value, decode, encode};
use ctap2::soft::{MemoryStore, ScriptedPresence, SoftBackend};
use ctap2::{Authenticator, AuthenticatorConfig, Bytes, Entropy};
use p256::pkcs8::EncodePublicKey;
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use sha2::{Digest, Sha256};
use url::{Host, Url};

/// Host policy. Software credentials are ephemeral and intended for headless QA.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthenticatorChoice {
    SecureEnclave,
    Software,
}

static CHOICE: OnceLock<AuthenticatorChoice> = OnceLock::new();
static AUTHENTICATOR: OnceLock<Mutex<Option<Device>>> = OnceLock::new();
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
static REQUESTS: LazyLock<Mutex<Requests>> = LazyLock::new(Mutex::default);

/// Set host policy before creating a realm or querying authenticator availability.
///
/// The default is SecureEnclave. On non-macOS hosts that choice is unavailable.
/// Changing policy after it has been observed is a host programming error.
pub fn set_authenticator(choice: AuthenticatorChoice) {
    CHOICE
        .set(choice)
        .expect("set WebAuthn authenticator before first use");
}

fn choice() -> AuthenticatorChoice {
    *CHOICE.get_or_init(|| AuthenticatorChoice::SecureEnclave)
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

#[derive(Clone, Debug, Serialize)]
struct Error {
    name: &'static str,
    message: &'static str,
}

impl Error {
    fn new(name: &'static str, message: &'static str) -> Self {
        Self { name, message }
    }

    fn denied() -> Self {
        Self::new(
            "NotAllowedError",
            "The authenticator did not allow the request",
        )
    }

    fn unknown() -> Self {
        Self::new(
            "UnknownError",
            "The authenticator could not complete the request",
        )
    }

    fn security() -> Self {
        Self::new(
            "SecurityError",
            "The relying party is not valid for this origin",
        )
    }

    fn wire(self) -> Json {
        serde_json::json!({ "error": self })
    }
}

type Result<T> = std::result::Result<T, Error>;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Rp {
    id: Option<String>,
    name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct User {
    id: Vec<u8>,
    name: String,
    display_name: String,
}

#[derive(Deserialize)]
pub(crate) struct Parameter {
    #[serde(rename = "type")]
    kind: String,
    alg: i64,
}

#[derive(Deserialize)]
pub(crate) struct Descriptor {
    #[serde(rename = "type")]
    kind: String,
    id: Vec<u8>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Selection {
    authenticator_attachment: Option<String>,
    resident_key: Option<String>,
    #[serde(default)]
    require_resident_key: bool,
    user_verification: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreationOptions {
    challenge: Vec<u8>,
    rp: Rp,
    user: User,
    #[serde(default)]
    pub_key_cred_params: Vec<Parameter>,
    #[serde(default)]
    exclude_credentials: Vec<Descriptor>,
    #[serde(default)]
    authenticator_selection: Selection,
    attestation: Option<String>,
    timeout: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RequestOptions {
    challenge: Vec<u8>,
    rp_id: Option<String>,
    #[serde(default)]
    allow_credentials: Vec<Descriptor>,
    user_verification: Option<String>,
    timeout: u64,
}

struct Prepared {
    request: Bytes,
    client_data: Vec<u8>,
    client_hash: [u8; 32],
    rp_id: String,
    rp_hash: [u8; 32],
    create: bool,
    anonymize: bool,
    require_uv: bool,
    allow: Vec<Vec<u8>>,
    timeout: u64,
}

fn base64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let word = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        output.push(char::from(ALPHABET[((word >> 18) & 63) as usize]));
        output.push(char::from(ALPHABET[((word >> 12) & 63) as usize]));
        if chunk.len() > 1 {
            output.push(char::from(ALPHABET[((word >> 6) & 63) as usize]));
        }
        if chunk.len() > 2 {
            output.push(char::from(ALPHABET[(word & 63) as usize]));
        }
    }
    output
}

fn effective_rp(origin: &str, secure: bool, requested: Option<&str>) -> Result<String> {
    if !secure {
        return Err(Error::security());
    }
    let url = Url::parse(origin).map_err(|_| Error::security())?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(Error::security());
    }
    let Some(Host::Domain(host)) = url.host() else {
        return Err(Error::security());
    };
    let requested = requested.unwrap_or(host);
    let Host::Domain(rp) = Host::parse(requested).map_err(|_| Error::security())? else {
        return Err(Error::security());
    };
    if rp.is_empty()
        || rp.split('.').any(|label| {
            label.is_empty()
                || label.starts_with('-')
                || label.ends_with('-')
                || !label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
    {
        return Err(Error::security());
    }
    if rp == host || (host.ends_with(&format!(".{rp}")) && psl::domain(rp.as_bytes()).is_some()) {
        Ok(rp)
    } else {
        Err(Error::security())
    }
}

fn require_uv(value: Option<&str>) -> Result<bool> {
    match value.unwrap_or("preferred") {
        "required" => Ok(true),
        "preferred" | "discouraged" => Ok(false),
        _ => Err(Error::new("TypeError", "Invalid userVerification value")),
    }
}

fn descriptors(values: &[Descriptor]) -> Result<Out<'_>> {
    let mut result = Vec::with_capacity(values.len());
    for descriptor in values {
        if descriptor.kind != "public-key" {
            return Err(Error::new(
                "TypeError",
                "Invalid credential descriptor type",
            ));
        }
        result.push(Out::Map(vec![
            (Out::Text("type"), Out::Text("public-key")),
            (Out::Text("id"), Out::Bytes(&descriptor.id)),
        ]));
    }
    Ok(Out::Array(result))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ClientData<'a> {
    #[serde(rename = "type")]
    kind: &'a str,
    challenge: String,
    origin: &'a str,
    cross_origin: bool,
}

fn client_data(kind: &str, challenge: &[u8], origin: &str) -> Result<(Vec<u8>, [u8; 32])> {
    if challenge.len() > 65_536 {
        return Err(Error::new("ConstraintError", "The challenge is too large"));
    }
    // Struct field order is the WebAuthn clientDataJSON key order.
    let data = serde_json::to_vec(&ClientData {
        kind,
        challenge: base64url(challenge),
        origin,
        cross_origin: false,
    })
    .map_err(|_| Error::unknown())?;
    let hash = Sha256::digest(&data).into();
    Ok((data, hash))
}

fn command(command: u8, parameters: Out<'_>) -> Result<Bytes> {
    let mut request = vec![command];
    request.extend(encode(&parameters));
    if request.len() > AuthenticatorConfig::default().max_msg_size as usize {
        return Err(Error::new(
            "ConstraintError",
            "The authenticator request is too large",
        ));
    }
    Ok(Bytes::from(request))
}

fn prepare_create(origin: &str, secure: bool, options: CreationOptions) -> Result<Prepared> {
    let rp_id = effective_rp(origin, secure, options.rp.id.as_deref())?;
    if options.user.id.is_empty() || options.user.id.len() > 64 {
        return Err(Error::new(
            "TypeError",
            "user.id must contain 1 through 64 bytes",
        ));
    }
    if !options.pub_key_cred_params.is_empty()
        && !options
            .pub_key_cred_params
            .iter()
            .any(|parameter| parameter.kind == "public-key" && parameter.alg == -7)
    {
        return Err(Error::new("NotSupportedError", "ES256 is required"));
    }
    let selection = &options.authenticator_selection;
    match selection.authenticator_attachment.as_deref() {
        None | Some("platform") => {}
        Some("cross-platform") => return Err(Error::denied()),
        Some(_) => {
            return Err(Error::new(
                "TypeError",
                "Invalid authenticatorAttachment value",
            ));
        }
    }
    let resident = match selection.resident_key.as_deref() {
        Some("required" | "preferred") => true,
        None | Some("discouraged") => selection.require_resident_key,
        Some(_) => return Err(Error::new("TypeError", "Invalid residentKey value")),
    };
    let require_uv = require_uv(selection.user_verification.as_deref())?;
    let anonymize = match options.attestation.as_deref().unwrap_or("none") {
        "none" | "indirect" => true,
        "direct" | "enterprise" => false,
        _ => return Err(Error::new("TypeError", "Invalid attestation value")),
    };
    let (client_data, client_hash) = client_data("webauthn.create", &options.challenge, origin)?;
    let request = command(
        ctap2::command::MAKE_CREDENTIAL,
        Out::Map(vec![
            (Out::Int(1), Out::Bytes(&client_hash)),
            (
                Out::Int(2),
                Out::Map(vec![
                    (Out::Text("id"), Out::Text(&rp_id)),
                    (Out::Text("name"), Out::Text(&options.rp.name)),
                ]),
            ),
            (
                Out::Int(3),
                Out::Map(vec![
                    (Out::Text("id"), Out::Bytes(&options.user.id)),
                    (Out::Text("name"), Out::Text(&options.user.name)),
                    (
                        Out::Text("displayName"),
                        Out::Text(&options.user.display_name),
                    ),
                ]),
            ),
            (
                Out::Int(4),
                Out::Array(vec![Out::Map(vec![
                    (Out::Text("type"), Out::Text("public-key")),
                    (Out::Text("alg"), Out::Int(-7)),
                ])]),
            ),
            (Out::Int(5), descriptors(&options.exclude_credentials)?),
            (
                Out::Int(7),
                Out::Map(vec![
                    (Out::Text("rk"), Out::Bool(resident)),
                    (Out::Text("uv"), Out::Bool(require_uv)),
                ]),
            ),
        ]),
    )?;
    Ok(Prepared {
        request,
        client_data,
        client_hash,
        rp_hash: Sha256::digest(rp_id.as_bytes()).into(),
        rp_id,
        create: true,
        anonymize,
        require_uv,
        allow: Vec::new(),
        timeout: options.timeout,
    })
}

fn prepare_get(origin: &str, secure: bool, options: RequestOptions) -> Result<Prepared> {
    let rp_id = effective_rp(origin, secure, options.rp_id.as_deref())?;
    let require_uv = require_uv(options.user_verification.as_deref())?;
    let (client_data, client_hash) = client_data("webauthn.get", &options.challenge, origin)?;
    let request = command(
        ctap2::command::GET_ASSERTION,
        Out::Map(vec![
            (Out::Int(1), Out::Text(&rp_id)),
            (Out::Int(2), Out::Bytes(&client_hash)),
            (Out::Int(3), descriptors(&options.allow_credentials)?),
            (
                Out::Int(5),
                Out::Map(vec![
                    (Out::Text("up"), Out::Bool(true)),
                    (Out::Text("uv"), Out::Bool(require_uv)),
                ]),
            ),
        ]),
    )?;
    Ok(Prepared {
        request,
        client_data,
        client_hash,
        rp_hash: Sha256::digest(rp_id.as_bytes()).into(),
        rp_id,
        create: false,
        anonymize: false,
        require_uv,
        allow: options
            .allow_credentials
            .into_iter()
            .map(|descriptor| descriptor.id)
            .collect(),
        timeout: options.timeout,
    })
}

struct RandomEntropy(Arc<rustls::crypto::CryptoProvider>);

impl Entropy for RandomEntropy {
    fn fill(&mut self, bytes: &mut [u8]) {
        self.0
            .secure_random
            .fill(bytes)
            .expect("WebAuthn secure random generation failed");
    }
}

type Software = Authenticator<SoftBackend<RandomEntropy>, MemoryStore, ScriptedPresence>;

#[cfg(target_os = "macos")]
type SecureEnclave = Authenticator<
    backend_se::SeBackend,
    se_fido_store::FileStore<backend_se::SystemEntropy>,
    EnclavePresence,
>;

#[cfg(target_os = "macos")]
struct EnclavePresence;

#[cfg(target_os = "macos")]
impl ctap2::UserPresence for EnclavePresence {
    fn request(
        &mut self,
        _: &ctap2::PresenceRequest,
    ) -> std::result::Result<ctap2::Presence, ctap2::PresenceError> {
        // The SE ACL enforces Touch ID or the device password at signing.
        // Device::process also signs a presence key before CTAP dispatch, so
        // excluded and missing credentials cannot bypass user presence.
        Ok(ctap2::Presence {
            user_verified: true,
        })
    }
}

enum Device {
    Software(Box<Software>),
    #[cfg(target_os = "macos")]
    SecureEnclave(Box<SecureEnclave>),
}

impl Device {
    fn open() -> Result<Self> {
        match choice() {
            AuthenticatorChoice::Software => {
                let provider = rustls::crypto::CryptoProvider::get_default()
                    .cloned()
                    .ok_or_else(Error::unknown)?;
                Ok(Self::Software(Box::new(Authenticator::new(
                    SoftBackend::new(RandomEntropy(provider)),
                    MemoryStore::new(),
                    ScriptedPresence::approve(),
                    AuthenticatorConfig::default(),
                ))))
            }
            AuthenticatorChoice::SecureEnclave => {
                #[cfg(target_os = "macos")]
                {
                    use backend_se::{AccessPolicy, SeBackend, StorageMode, SystemEntropy};
                    let directory = crate::cookie_store::profile_directory().join("webauthn");
                    let key = backend_se::storage_key(&directory).map_err(|_| Error::unknown())?;
                    let store = se_fido_store::FileStore::open(&directory, &key, SystemEntropy)
                        .map_err(|_| Error::unknown())?;
                    Ok(Self::SecureEnclave(Box::new(Authenticator::new(
                        SeBackend::new(StorageMode::Blob, AccessPolicy::None),
                        store,
                        EnclavePresence,
                        AuthenticatorConfig::default(),
                    ))))
                }
                #[cfg(not(target_os = "macos"))]
                {
                    Err(Error::denied())
                }
            }
        }
    }

    fn process(&mut self, prepared: &Prepared, job: &Job) -> Result<Vec<u8>> {
        job.check()?;
        match self {
            Self::Software(authenticator) => {
                let response = authenticator.process(&prepared.request, 0);
                authenticator.presence_mut().requests.clear();
                Ok(response)
            }
            #[cfg(target_os = "macos")]
            Self::SecureEnclave(authenticator) => {
                use ctap2::Backend;
                // CTAP's excluded/no-credential paths do not sign a credential
                // key. Gate every request before lookup to avoid a silent
                // credential-existence probe. The ephemeral blob is never saved.
                let mut gate = backend_se::SeBackend::new(
                    backend_se::StorageMode::Blob,
                    backend_se::AccessPolicy::None,
                );
                let key = gate
                    .create_key(&prepared.rp_id, &[], ctap2::UvPolicy::Presence)
                    .map_err(|_| Error::denied())?;
                let approved = gate.sign(&key.handle, &prepared.client_hash);
                let _ = gate.delete_key(&key.handle);
                approved.map_err(|_| Error::denied())?;
                job.check()?;
                Ok(authenticator.process(&prepared.request, 0))
            }
        }
    }
}

/// Availability describes the real platform authenticator, never the QA backend.
/// Creating an unprotected ephemeral key checks SE capability without prompting
/// or opening the persisted credential index.
pub(crate) fn platform_available() -> bool {
    if choice() != AuthenticatorChoice::SecureEnclave {
        return false;
    }
    #[cfg(target_os = "macos")]
    {
        backend_se::SeBackend::new(
            backend_se::StorageMode::Blob,
            backend_se::AccessPolicy::None,
        )
        .create()
        .is_ok()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

#[derive(Default)]
struct Requests {
    active: Option<u64>,
    jobs: HashMap<u64, Arc<Job>>,
}

struct Job {
    id: u64,
    owner: u64,
    deadline: Instant,
    discarded: AtomicBool,
    result: Mutex<Option<Arc<Result<Json>>>>,
    notifications: std::sync::Weak<Notifications>,
    completed: nagoya::sync::Notify,
}

struct CompletionRing {
    producer: spsc::Producer<Arc<Job>>,
    consumer: spsc::Consumer<Arc<Job>>,
    closed: bool,
}

pub(crate) struct Notifications {
    ring: Mutex<CompletionRing>,
    wake: crate::net_bridge::PageWake,
    ready: nagoya::sync::Notify,
}

impl Notifications {
    pub(crate) fn new(wake: crate::net_bridge::PageWake) -> Arc<Self> {
        let (producer, consumer) = spsc::make(128);
        Arc::new(Self {
            ring: Mutex::new(CompletionRing {
                producer,
                consumer,
                closed: false,
            }),
            wake,
            ready: nagoya::sync::Notify::new(),
        })
    }

    fn enqueue(&self, job: Arc<Job>) {
        let ring = lock(&self.ring);
        if ring.closed {
            return;
        }
        if let Some(job) = ring.producer.try_push(job) {
            // Native status/abort callers may have consumed old completions.
            // At most 64 requests still own a result, so compact those entries.
            let mut live = Vec::new();
            while let Some(old) = ring.consumer.try_pop() {
                if !old.discarded.load(Ordering::Acquire) && lock(&old.result).is_some() {
                    live.push(old);
                }
            }
            for old in live {
                assert!(
                    ring.producer.try_push(old).is_none(),
                    "WebAuthn live completion limit"
                );
            }
            assert!(
                ring.producer.try_push(job).is_none(),
                "WebAuthn live completion limit"
            );
        }
        drop(ring);
        self.ready.notify_one();
        self.wake.wake();
    }

    pub(crate) async fn changed(&self) {
        self.ready.notified().await;
    }

    pub(crate) fn bind(&self, waker: Option<&std::task::Waker>) {
        self.wake.bind(waker);
    }

    pub(crate) fn close(&self) {
        let mut ring = lock(&self.ring);
        ring.closed = true;
        while ring.consumer.try_pop().is_some() {}
        drop(ring);
        self.ready.notify_one();
    }

    pub(crate) fn take(&self, owner: u64) -> Vec<(u64, Arc<Json>)> {
        let completed = {
            let ring = lock(&self.ring);
            let mut completed = Vec::new();
            while let Some(job) = ring.consumer.try_pop() {
                completed.push(job);
            }
            completed
        };
        let mut requests = lock(&REQUESTS);
        completed
            .into_iter()
            .filter_map(|job| {
                if job.owner != owner
                    || !requests
                        .jobs
                        .get(&job.id)
                        .is_some_and(|current| Arc::ptr_eq(current, &job))
                {
                    return None;
                }
                let result = lock(&job.result).take()?;
                requests.jobs.remove(&job.id);
                let response = match result.as_ref() {
                    Ok(value) => serde_json::json!({ "result": value }),
                    Err(error) => error.clone().wire(),
                };
                Some((job.id, Arc::new(response)))
            })
            .collect()
    }
}

fn complete(job: Arc<Job>, result: Arc<Result<Json>>) {
    {
        let mut current = lock(&job.result);
        if current.is_some() || job.discarded.load(Ordering::Acquire) {
            return;
        }
        *current = Some(result);
    }
    job.completed.notify_one();
    if cfg!(feature = "reactor-witness")
        && std::env::var("CHUZZ_REACTOR_DEFECT").as_deref() == Ok("webauthn-notification")
    {
        return;
    }
    if let Some(notifications) = job.notifications.upgrade() {
        notifications.enqueue(job);
    }
}

impl Job {
    fn check(&self) -> Result<()> {
        if Instant::now() >= self.deadline {
            return Err(Error::denied());
        }
        if self.discarded.load(Ordering::Acquire) {
            return Err(Error::new("AbortError", "The request was aborted"));
        }
        Ok(())
    }
}

pub(crate) fn create(
    owner: u64,
    origin: &str,
    secure: bool,
    options: CreationOptions,
    notifications: &Arc<Notifications>,
) -> Json {
    match prepare_create(origin, secure, options) {
        Ok(prepared) => begin(owner, Arc::new(prepared), notifications),
        Err(error) => error.wire(),
    }
}

pub(crate) fn get(
    owner: u64,
    origin: &str,
    secure: bool,
    options: RequestOptions,
    notifications: &Arc<Notifications>,
) -> Json {
    match prepare_get(origin, secure, options) {
        Ok(prepared) => begin(owner, Arc::new(prepared), notifications),
        Err(error) => error.wire(),
    }
}

fn begin(owner: u64, prepared: Arc<Prepared>, notifications: &Arc<Notifications>) -> Json {
    let mut requests = lock(&REQUESTS);
    if requests.active.is_some() || requests.jobs.len() >= 64 {
        return Error::denied().wire();
    }
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let job = Arc::new(Job {
        id,
        owner,
        deadline: Instant::now() + Duration::from_millis(prepared.timeout.clamp(1, 120_000)),
        discarded: AtomicBool::new(false),
        result: Mutex::new(None),
        notifications: Arc::downgrade(notifications),
        completed: nagoya::sync::Notify::new(),
    });
    requests.active = Some(id);
    requests.jobs.insert(id, Arc::clone(&job));
    drop(requests);

    if choice() == AuthenticatorChoice::Software {
        let worker_job = Arc::clone(&job);
        let prepared = Arc::clone(&prepared);
        drop(nagoya::spawn(async move {
            let result = catch_unwind(AssertUnwindSafe(|| perform(&prepared, &worker_job)))
                .unwrap_or_else(|_| Err(Error::unknown()));
            {
                let mut requests = lock(&REQUESTS);
                if requests.active == Some(worker_job.id) {
                    requests.active = None;
                }
                if worker_job.discarded.load(Ordering::Acquire) {
                    requests.jobs.remove(&worker_job.id);
                }
            }
            complete(Arc::clone(&worker_job), Arc::new(result));
            worker_job
        }));
        drop(nagoya::spawn(async move {
            if nagoya::timeout(
                job.deadline.saturating_duration_since(Instant::now()),
                job.completed.notified(),
            )
            .await
            .is_err()
            {
                complete(Arc::clone(&job), Arc::new(Err(Error::denied())));
            }
            job
        }));
        return serde_json::json!(id);
    }
    let worker_job = Arc::clone(&job);
    // owner ruling: typed Arc<T> hand-off, concurrency only via nagoya/ps-st3; owner exception: blocking platform authenticator.
    #[allow(clippy::disallowed_methods)]
    let spawned = std::thread::Builder::new()
        .name("chuzz-webauthn".into())
        .spawn(move || {
            let result = catch_unwind(AssertUnwindSafe(|| perform(&prepared, &worker_job)))
                .unwrap_or_else(|_| Err(Error::unknown()));
            {
                let mut requests = lock(&REQUESTS);
                if requests.active == Some(worker_job.id) {
                    requests.active = None;
                }
                if worker_job.discarded.load(Ordering::Acquire) {
                    requests.jobs.remove(&worker_job.id);
                }
            }
            complete(Arc::clone(&worker_job), Arc::new(result));
            worker_job
        });
    if spawned.is_err() {
        let mut requests = lock(&REQUESTS);
        requests.jobs.remove(&id);
        if requests.active == Some(id) {
            requests.active = None;
        }
        return Error::unknown().wire();
    }
    drop(nagoya::spawn(async move {
        if nagoya::timeout(
            job.deadline.saturating_duration_since(Instant::now()),
            job.completed.notified(),
        )
        .await
        .is_err()
        {
            complete(Arc::clone(&job), Arc::new(Err(Error::denied())));
        }
        job
    }));
    serde_json::json!(id)
}

fn perform(prepared: &Prepared, job: &Job) -> Result<Json> {
    job.check()?;
    let mut device = lock(AUTHENTICATOR.get_or_init(|| Mutex::new(None)));
    if device.is_none() {
        *device = Some(Device::open()?);
    }
    job.check()?;
    let response = device
        .as_mut()
        .ok_or_else(Error::unknown)?
        .process(prepared, job)?;
    job.check()?;
    let (&status, body) = response.split_first().ok_or_else(Error::unknown)?;
    if status != 0 {
        return Err(match status {
            0x19 => Error::new("InvalidStateError", "A credential is excluded"),
            0x26 => Error::new("NotSupportedError", "The algorithm is unavailable"),
            0x05 | 0x27 | 0x2d | 0x2e | 0x2f | 0x30 => Error::denied(),
            _ => Error::unknown(),
        });
    }
    let body = decode(&Bytes::copy_from_slice(body)).map_err(|_| Error::unknown())?;
    if prepared.create {
        registration(prepared, &body)
    } else {
        assertion(prepared, &body)
    }
}

pub(crate) fn status(owner: u64, id: u64) -> Json {
    let mut requests = lock(&REQUESTS);
    let Some(job) = requests
        .jobs
        .get(&id)
        .filter(|job| job.owner == owner)
        .cloned()
    else {
        return Error::denied().wire();
    };
    let result = lock(&job.result).take();
    if let Some(result) = result {
        requests.jobs.remove(&id);
        return match result.as_ref() {
            Ok(result) => serde_json::json!({ "result": result }),
            Err(error) => error.clone().wire(),
        };
    }
    if Instant::now() >= job.deadline {
        job.discarded.store(true, Ordering::Release);
        requests.jobs.remove(&id);
        return Error::denied().wire();
    }
    serde_json::json!({ "pending": true })
}

pub(crate) fn abort(owner: u64, id: u64) {
    let mut requests = lock(&REQUESTS);
    if requests.jobs.get(&id).is_some_and(|job| job.owner == owner)
        && let Some(job) = requests.jobs.remove(&id)
    {
        job.discarded.store(true, Ordering::Release);
        job.completed.notify_one();
    }
    // A blocking SE call still owns active until its worker returns.
}

pub(crate) fn release_realm(owner: u64) {
    let mut requests = lock(&REQUESTS);
    requests.jobs.retain(|_, job| {
        if job.owner == owner {
            job.discarded.store(true, Ordering::Release);
            job.completed.notify_one();
            false
        } else {
            true
        }
    });
}

fn field(value: &Value, key: i64) -> Result<&Value> {
    value.get(key).ok_or_else(Error::unknown)
}

fn bytes(value: &Value) -> Result<&Bytes> {
    value.as_bytes().map_err(|_| Error::unknown())
}

fn text(value: &Value) -> Result<&str> {
    value
        .as_text()
        .map(|text| text.as_str())
        .map_err(|_| Error::unknown())
}

fn authenticator_data(prepared: &Prepared, data: &[u8], create: bool) -> Result<()> {
    if data.len() < 37
        || data[..32] != prepared.rp_hash
        || data[32] & ctap2::authdata::FLAG_UP == 0
        || (prepared.require_uv && data[32] & ctap2::authdata::FLAG_UV == 0)
        || (create && data[32] & ctap2::authdata::FLAG_AT == 0)
        || data[32] & ctap2::authdata::FLAG_ED != 0
    {
        return Err(Error::unknown());
    }
    Ok(())
}

fn registration(prepared: &Prepared, body: &Value) -> Result<Json> {
    let mut data = bytes(field(body, 2)?)?.to_vec();
    authenticator_data(prepared, &data, true)?;
    if data.len() < 55 {
        return Err(Error::unknown());
    }
    let length = usize::from(u16::from_be_bytes([data[53], data[54]]));
    if length == 0 {
        return Err(Error::unknown());
    }
    let raw_id = data
        .get(55..55 + length)
        .ok_or_else(Error::unknown)?
        .to_vec();
    let cose = decode(&Bytes::copy_from_slice(
        data.get(55 + length..).ok_or_else(Error::unknown)?,
    ))
    .map_err(|_| Error::unknown())?;
    let integer = |key| cose.get(key).and_then(|value| value.as_int().ok());
    if integer(1) != Some(2) || integer(3) != Some(-7) || integer(-1) != Some(1) {
        return Err(Error::unknown());
    }
    let x = bytes(field(&cose, -2)?)?;
    let y = bytes(field(&cose, -3)?)?;
    if x.len() != 32 || y.len() != 32 {
        return Err(Error::unknown());
    }
    let mut point = vec![4];
    point.extend_from_slice(x);
    point.extend_from_slice(y);
    let public_key = p256::PublicKey::from_sec1_bytes(&point)
        .map_err(|_| Error::unknown())?
        .to_public_key_der()
        .map_err(|_| Error::unknown())?;

    let format = text(field(body, 1)?)?;
    let statement = field(body, 3)?;
    if format != "packed"
        || statement
            .get_text("alg")
            .and_then(|value| value.as_int().ok())
            != Some(-7)
        || statement.get_text("x5c").is_some()
    {
        return Err(Error::unknown());
    }
    let signature = bytes(statement.get_text("sig").ok_or_else(Error::unknown)?)?;
    let (format, statement) = if prepared.anonymize {
        data[37..53].fill(0);
        ("none", Out::Map(Vec::new()))
    } else {
        (
            "packed",
            Out::Map(vec![
                (Out::Text("alg"), Out::Int(-7)),
                (Out::Text("sig"), Out::Bytes(signature)),
            ]),
        )
    };
    let attestation = encode(&Out::Map(vec![
        (Out::Text("fmt"), Out::Text(format)),
        (Out::Text("attStmt"), statement),
        (Out::Text("authData"), Out::Bytes(&data)),
    ]));
    Ok(serde_json::json!({
        "id": base64url(&raw_id),
        "rawId": raw_id,
        "clientDataJSON": prepared.client_data,
        "attestationObject": attestation,
        "authenticatorData": data,
        "publicKey": public_key.as_bytes(),
        "publicKeyAlgorithm": -7,
        "transports": ["internal"],
        "authenticatorAttachment": "platform",
    }))
}

fn assertion(prepared: &Prepared, body: &Value) -> Result<Json> {
    let descriptor = field(body, 1)?;
    if text(descriptor.get_text("type").ok_or_else(Error::unknown)?)? != "public-key" {
        return Err(Error::unknown());
    }
    let raw_id = bytes(descriptor.get_text("id").ok_or_else(Error::unknown)?)?;
    if raw_id.is_empty()
        || (!prepared.allow.is_empty()
            && !prepared
                .allow
                .iter()
                .any(|allowed| allowed.as_slice() == &raw_id[..]))
    {
        return Err(Error::unknown());
    }
    let data = bytes(field(body, 2)?)?;
    authenticator_data(prepared, data, false)?;
    if data.len() != 37 || data[32] & ctap2::authdata::FLAG_AT != 0 {
        return Err(Error::unknown());
    }
    let signature = bytes(field(body, 3)?)?;
    let user_handle = body
        .get(4)
        .map(|user| {
            let handle = bytes(user.get_text("id").ok_or_else(Error::unknown)?)?;
            if handle.is_empty() || handle.len() > 64 {
                return Err(Error::unknown());
            }
            Ok(handle.to_vec())
        })
        .transpose()?;
    Ok(serde_json::json!({
        "id": base64url(raw_id),
        "rawId": raw_id.to_vec(),
        "clientDataJSON": prepared.client_data,
        "authenticatorData": data.to_vec(),
        "signature": signature.to_vec(),
        "userHandle": user_handle,
        "authenticatorAttachment": "platform",
    }))
}
