//! The headless runtime for Blitz: one page, no window, driven over the
//! control socket.
//!
//! The pair to `izumo`, which is the same engine embedded in a
//! Tauri window. That one is an adapter, about 4,000 lines letting Tauri host
//! a Blitz document. This is the browser itself with the window left off, so
//! it is a module here rather than a crate of its own: it needs
//! `document_loader`, `page_server`, `nav` and `identity`, which is most of a
//! browser, and the last attempt to package it separately is the cautionary
//! tale below.
//!
//!
//! # Why this is here and not in a second crate
//!
//! It used to be `qa-inspect-host`, a separate binary in ps-observability that
//! built its own document out of a dist directory. That made two headless
//! browsers: one that a person browses with and one that QA drives, and the web
//! platform existed in only the first of them. `document_loader`'s shim is what
//! makes `@solidjs/router` reach its first render at all, and a host without it
//! reported every routed page in the fleet as blank. Every gap closed for the
//! browser had to be closed a second time for the harness, by hand, or the
//! harness kept measuring a browser nobody ships.
//!
//! So the host is a mode of the browser instead. What `ps-qa` drives is the
//! same loader, the same shim and the same engine a tab uses; the only thing
//! missing is the window.
//!
//! `ps-qa` still links none of this. It speaks `blitz-control-protocol` over
//! the socket and is forbidden from depending on blitz, tauri, winit or wgpu,
//! which is satisfied by a socket, not by a crate boundary.
//!
//! # Use
//!
//! ```sh
//! chuzz-headless /path/to/dist            # or a file, or an http(s) URL
//! QA_INSPECT_PAGE=/path/to/dist chuzz-headless
//! QA_INSPECT_PAGE=/path/to/dist QA_INSPECT_STORAGE=/path/to/storage.json chuzz-headless
//! ```
//!
//! `QA_INSPECT_STORAGE` is a JSON object of string values. The host installs
//! them into `localStorage` for the page origin before any page script runs,
//! through the same prelude a navigation uses to restore storage. A missing,
//! unreadable, or invalid file prints
//! `chuzz-headless: QA_INSPECT_STORAGE: ...` and exits 2.
//!
//! It prints its descriptor path on stdout when it is ready, then serves until
//! killed. That line, and one page per process, are the two properties
//! `ps-qa sweep-components` is built on: it launches one host per component and
//! waits for the line, so a component that wedges the engine cannot poison the
//! next one's verdict.

use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use blitz_control_protocol::condense;
use blitz_control_protocol::document::{DocumentCapture, inspect_document, snapshot_document};
use blitz_control_protocol::in_process::DocumentControl;
use blitz_control_protocol::latest::Latest;
use blitz_control_protocol::server::ControlBridgeRequest;
use blitz_control_protocol::{
    AgentAction, AgentControlRequest, DebugError, DebugEvent, DebugResponse, DiagnosticsRequest,
    RendererMetrics, RevisionSet, WindowComposition,
};
use blitz_dom::Document as _;
use blitz_dom::NodeId;
use blitz_script::ScriptDocument;
use blitz_traits::net::Url;
use blitz_traits::shell::{ColorScheme, Viewport};

#[path = "serve_page_control.rs"]
mod page_control;
#[path = "serve_shadow.rs"]
mod shadow_inspect;
#[path = "single_process_control.rs"]
mod supervisor;

fn reactor_defect(name: &str) -> bool {
    cfg!(feature = "reactor-witness")
        && std::env::var("CHUZZ_REACTOR_DEFECT").as_deref() == Ok(name)
}

fn trace(message: &str) {
    eprintln!("chuzz-headless: {message}");
}

/// Timestamped tracing of the loop and the socket, off unless `CHUZZ_TRACE` is
/// set in the environment.
///
/// The question this exists for is not "did it happen" but "when did it happen
/// relative to what the harness was doing", which no snapshot of the tree can
/// answer. It is what found the outcome poll that went blind to the rest of the
/// document, and it is deliberately millisecond-stamped so its lines can be
/// read against a driver's own timings.
pub(crate) fn verbose(message: &str) {
    if std::env::var_os("CHUZZ_TRACE").is_none() {
        return;
    }
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_millis() % 1_000_000)
        .unwrap_or_default();
    eprintln!("chuzz-headless: [{at}] {message}");
}

/// A positive integer from the environment, or `default`.
fn dimension(variable: &str, default: u32) -> Result<u32, String> {
    let Some(value) = std::env::var_os(variable) else {
        return Ok(default);
    };
    let text = value
        .into_string()
        .map_err(|_| format!("{variable} is not valid UTF-8"))?;
    text.parse::<u32>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| format!("{variable} must be a positive integer, got {text:?}"))
}

const BOT_CHILD_MAX_WALL_TIME: Duration = Duration::from_secs(20);
const BOT_CHILD_MAX_ACTIONS: usize = 8;
const BOT_CHILD_MAX_INSPECTIONS: usize = 16;
const BOT_CHILD_MAX_REQUESTS: usize = 24;
const BOT_CHILD_MAX_PENDING_REQUESTS: usize = 16;
const BOT_CHILD_MAX_INSPECT_DEPTH: u32 = 12;
const BOT_CHILD_MAX_INSPECT_NODES: usize = 512;
const BOT_CHILD_MAX_INSPECT_BYTES: usize = 60 * 1024;
const BOT_CHILD_MAX_PAGE_NODES: usize = 20_000;
const BOT_CHILD_MAX_FILL_BYTES: usize = 512;
const BOT_CHILD_MAX_VIEWPORT_WIDTH: u32 = 1600;
const BOT_CHILD_MAX_VIEWPORT_HEIGHT: u32 = 1000;
const BOT_CHILD_MAX_VIEWPORT_PIXELS: u64 = 1_500_000;

/// Runtime policy for the QA host and for a short-lived browser child.
///
/// QA stays permissive for fixture directories, files, and remote URLs. The
/// bot-child mode is explicit so a parent process cannot accidentally inherit
/// the QA host's broader target and control surface.
#[derive(Clone, Debug)]
struct HeadlessPolicy {
    bot_child: bool,
    allowed_domains: Vec<String>,
    allow_form_submit: bool,
    wall_time: Duration,
}

impl HeadlessPolicy {
    fn from_env() -> Result<Self, String> {
        let mode = optional_env("CHUZZ_HEADLESS_MODE")?.unwrap_or_else(|| "qa".to_owned());
        let bot_child = match mode.trim() {
            "qa" => false,
            "bot" => true,
            _ => {
                return Err("CHUZZ_HEADLESS_MODE must be either qa or bot".to_owned());
            }
        };

        let allowed_domains = if bot_child {
            let domains = optional_env("CHUZZ_HEADLESS_ALLOWED_DOMAINS")?;
            parse_allowed_domains(domains.as_deref())?
        } else {
            Vec::new()
        };
        let allow_form_submit = if bot_child {
            parse_bool_env("CHUZZ_HEADLESS_ALLOW_FORM_SUBMIT", false)?
        } else {
            false
        };
        let wall_time = if bot_child {
            let timeout = optional_env("CHUZZ_HEADLESS_WALL_TIMEOUT_MS")?;
            parse_wall_time(timeout.as_deref())?
        } else {
            Duration::ZERO
        };

        Ok(Self {
            bot_child,
            allowed_domains,
            allow_form_submit,
            wall_time,
        })
    }

    #[cfg(test)]
    fn bot_child_for_tests(allowed_domains: &[&str], allow_form_submit: bool) -> Self {
        Self {
            bot_child: true,
            allowed_domains: allowed_domains
                .iter()
                .map(|host| (*host).to_owned())
                .collect(),
            allow_form_submit,
            wall_time: BOT_CHILD_MAX_WALL_TIME,
        }
    }

    fn validate_target(&self, target: &Target) -> Result<(), String> {
        if !self.bot_child {
            return Ok(());
        }
        match target {
            Target::Page(url) => self
                .validate_url(url)
                .map_err(|_| "bot mode requires a public HTTPS page URL".to_owned()),
            Target::Directory(_) | Target::File { .. } => {
                Err("bot mode accepts one public HTTPS page URL".to_owned())
            }
        }
    }

    fn validate_url(&self, url: &Url) -> Result<(), String> {
        if !self.bot_child {
            return Ok(());
        }
        self.resolve_public_addresses(url).map(|_| ())
    }

    fn resolve_public_addresses(&self, url: &Url) -> Result<Vec<SocketAddr>, String> {
        if url.scheme() != "https" {
            return Err("only HTTPS navigation is allowed in bot mode".to_owned());
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err("URLs with credentials are not allowed in bot mode".to_owned());
        }
        let host = url
            .host_str()
            .ok_or_else(|| "the URL has no host".to_owned())?;
        let normalized_host = host.trim_end_matches('.').to_ascii_lowercase();
        if normalized_host.is_empty() || is_local_hostname(&normalized_host) {
            return Err("local hostnames are not allowed in bot mode".to_owned());
        }
        if !domain_is_allowed(&normalized_host, &self.allowed_domains) {
            return Err("the URL host is outside the configured allowlist".to_owned());
        }

        let port = url
            .port_or_known_default()
            .ok_or_else(|| "the URL has no known port".to_owned())?;
        if let Ok(address) = normalized_host.parse::<IpAddr>() {
            if !is_public_address(address) {
                return Err("local and non-public IP addresses are not allowed".to_owned());
            }
            return Ok(vec![SocketAddr::new(address, port)]);
        }

        let addresses = (normalized_host.as_str(), port)
            .to_socket_addrs()
            .map_err(|_| "the URL host could not be resolved".to_owned())?
            .collect::<Vec<SocketAddr>>();
        if addresses.is_empty()
            || addresses
                .iter()
                .any(|address| !is_public_address(address.ip()))
        {
            return Err(
                "the URL host does not resolve exclusively to public IP addresses".to_owned(),
            );
        }
        Ok(addresses)
    }

    fn view_dimensions(&self) -> Result<(u32, u32), String> {
        let width = dimension("QA_HOST_WIDTH", 1344)?;
        let height = dimension("QA_HOST_HEIGHT", 900)?;
        if self.bot_child
            && (width > BOT_CHILD_MAX_VIEWPORT_WIDTH
                || height > BOT_CHILD_MAX_VIEWPORT_HEIGHT
                || u64::from(width) * u64::from(height) > BOT_CHILD_MAX_VIEWPORT_PIXELS)
        {
            return Err(format!(
                "bot viewport exceeds the {}x{} and {}-pixel limits",
                BOT_CHILD_MAX_VIEWPORT_WIDTH,
                BOT_CHILD_MAX_VIEWPORT_HEIGHT,
                BOT_CHILD_MAX_VIEWPORT_PIXELS
            ));
        }
        Ok((width, height))
    }
}

fn optional_env(variable: &str) -> Result<Option<String>, String> {
    std::env::var_os(variable)
        .map(|value| {
            value
                .into_string()
                .map_err(|_| format!("{variable} is not valid UTF-8"))
        })
        .transpose()
}

fn parse_bool_env(variable: &str, default: bool) -> Result<bool, String> {
    let Some(value) = std::env::var_os(variable) else {
        return Ok(default);
    };
    let value = value
        .into_string()
        .map_err(|_| format!("{variable} is not valid UTF-8"))?;
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" => Ok(true),
        "0" | "false" | "no" => Ok(false),
        _ => Err(format!("{variable} must be a boolean value")),
    }
}

fn parse_wall_time(value: Option<&str>) -> Result<Duration, String> {
    let Some(value) = value else {
        return Ok(BOT_CHILD_MAX_WALL_TIME);
    };
    let milliseconds = value
        .parse::<u64>()
        .ok()
        .filter(|milliseconds| *milliseconds > 0)
        .ok_or_else(|| "CHUZZ_HEADLESS_WALL_TIMEOUT_MS must be a positive integer".to_owned())?;
    if milliseconds > BOT_CHILD_MAX_WALL_TIME.as_millis() as u64 {
        return Err(format!(
            "CHUZZ_HEADLESS_WALL_TIMEOUT_MS cannot exceed {}ms",
            BOT_CHILD_MAX_WALL_TIME.as_millis()
        ));
    }
    Ok(Duration::from_millis(milliseconds))
}

fn parse_allowed_domains(value: Option<&str>) -> Result<Vec<String>, String> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let mut domains = Vec::new();
    for raw in value
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
    {
        let normalized = raw.to_ascii_lowercase();
        let parsed = Url::parse(&format!("https://{normalized}/"))
            .map_err(|_| "CHUZZ_HEADLESS_ALLOWED_DOMAINS contains an invalid host".to_owned())?;
        if parsed.host_str() != Some(normalized.as_str())
            || parsed.port().is_some()
            || !matches!(parsed.host(), Some(url::Host::Domain(_)))
            || is_local_hostname(&normalized)
            || !is_dns_hostname(&normalized)
        {
            return Err(
                "CHUZZ_HEADLESS_ALLOWED_DOMAINS must contain DNS hostnames only".to_owned(),
            );
        }
        domains.push(normalized);
    }
    domains.sort();
    domains.dedup();
    Ok(domains)
}

fn is_dns_hostname(host: &str) -> bool {
    host.len() <= 253
        && host.split('.').count() >= 2
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label
                    .as_bytes()
                    .first()
                    .is_some_and(u8::is_ascii_alphanumeric)
                && label
                    .as_bytes()
                    .last()
                    .is_some_and(u8::is_ascii_alphanumeric)
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

fn domain_is_allowed(host: &str, allowed_domains: &[String]) -> bool {
    allowed_domains.is_empty()
        || allowed_domains.iter().any(|domain| {
            host == domain
                || host
                    .strip_suffix(domain)
                    .is_some_and(|prefix| prefix.ends_with('.'))
        })
}

fn is_local_hostname(host: &str) -> bool {
    host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host.ends_with(".internal")
        || host.ends_with(".lan")
}

fn is_public_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => is_public_ipv4(address),
        IpAddr::V6(address) => {
            if let Some(mapped) = address.to_ipv4() {
                return is_public_address(IpAddr::V4(mapped));
            }
            let segments = address.segments();
            let special_purpose = (segments[0] == 0x2001 && matches!(segments[1], 0x0000 | 0x0db8))
                || segments[0] == 0x2002
                || (segments[0] == 0x0064 && segments[1] == 0xff9b);
            !(address.is_loopback()
                || address.is_unique_local()
                || address.is_unicast_link_local()
                || address.is_unspecified()
                || address.is_multicast()
                || special_purpose)
        }
    }
}

fn is_public_ipv4(address: std::net::Ipv4Addr) -> bool {
    let [first, second, third, _] = address.octets();
    let shared = first == 100 && (64..=127).contains(&second);
    let documentation = (first == 192 && second == 0 && third == 2)
        || (first == 198 && second == 51 && third == 100)
        || (first == 203 && second == 0 && third == 113);
    let special_purpose = first == 0
        || first == 10
        || first == 127
        || shared
        || (first == 169 && second == 254)
        || (first == 172 && (16..=31).contains(&second))
        || (first == 192 && second == 168)
        || (first == 192 && second == 0 && third == 0)
        || (first == 192 && second == 88 && third == 99)
        || (first == 198 && (18..=19).contains(&second))
        || documentation
        || first >= 224
        || address == std::net::Ipv4Addr::BROADCAST;
    !(special_purpose || address.is_private() || address.is_loopback() || address.is_multicast())
}

struct WallTimeoutGuard(Option<nagoya::JoinHandle<Arc<()>>>);

impl WallTimeoutGuard {
    fn start(timeout: Duration) -> Result<Self, String> {
        if reactor_defect("wall-timeout") { return Ok(Self(None)); }
        let task = nagoya::spawn(async move {
            nagoya::sleep(timeout).await;
            eprintln!("chuzz-headless: bot child reached its wall-time limit");
            std::process::exit(124);
        });
        Ok(Self(Some(task)))
    }
}

impl Drop for WallTimeoutGuard {
    fn drop(&mut self) {
        if let Some(task) = self.0.take() {
            task.cancel();
        }
    }
}

const MAX_PROXY_HEADER_BYTES: usize = 16 * 1024;
const MAX_PROXY_CONNECTIONS: usize = 16;
const MAX_PROXY_BYTES: usize = 32 * 1024 * 1024;
const PROXY_ENVIRONMENT: [&str; 8] = [
    "HTTP_PROXY",
    "http_proxy",
    "HTTPS_PROXY",
    "https_proxy",
    "ALL_PROXY",
    "all_proxy",
    "NO_PROXY",
    "no_proxy",
];

#[path = "serve_egress.rs"]
mod egress;
use egress::EgressProxy;

fn proxy_connect_url(request_head: &str) -> Option<Url> {
    let first_line = request_head.lines().next()?;
    let mut fields = first_line.split_ascii_whitespace();
    if fields.next()? != "CONNECT" {
        return None;
    }
    let authority = fields.next()?;
    Url::parse(&format!("https://{authority}/")).ok()
}

fn reserve_proxy_bytes(
    transferred_bytes: &std::sync::atomic::AtomicUsize,
    requested: usize,
) -> usize {
    loop {
        let current = transferred_bytes.load(Ordering::Acquire);
        if current >= MAX_PROXY_BYTES {
            return 0;
        }
        let allowed = requested.min(MAX_PROXY_BYTES - current);
        if transferred_bytes
            .compare_exchange_weak(
                current,
                current + allowed,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
        {
            return allowed;
        }
    }
}

/// What to load, from the command line or the environment.
///
/// `QA_INSPECT_PAGE` is how `ps-qa` tells a host which page to serve: it runs
/// the binary named by `--host` with no arguments and that variable set. An
/// explicit argument wins, so the same binary is usable by hand.
#[cfg(feature = "reactor-witness")]
#[path = "serve_witness.rs"]
mod reactor_witness_impl;

#[cfg(feature = "reactor-witness")]
pub fn reactor_witness() -> Result<(), String> { reactor_witness_impl::run() }

pub fn target_from(args: &[String]) -> Result<String, String> {
    if let Some(argument) = args.iter().skip(1).find(|arg| !arg.starts_with("--")) {
        return Ok(argument.clone());
    }
    std::env::var("QA_INSPECT_PAGE")
        .map_err(|_| "no page to serve: pass one as an argument or set QA_INSPECT_PAGE".to_owned())
}

/// What the caller named.
#[derive(Debug, PartialEq, Eq)]
pub enum Target {
    /// A built site, which needs an origin before it is a site at all. See
    /// [`crate::page_server`].
    Directory(std::path::PathBuf),
    /// A built page inside a directory: served as a site, and opened at that
    /// page rather than at `index.html`.
    ///
    /// A page in a build is no more a file than a whole site is. The component
    /// sweep names one page per component, `button.html` beside `button.js`,
    /// and each of those references `/static/js/button.js` absolutely, which a
    /// `file://` base resolves to the filesystem root. The bundle is then never
    /// fetched and the page renders as an empty mount point, which reads as a
    /// component that draws nothing.
    File {
        root: std::path::PathBuf,
        page: String,
    },
    /// A single file, or a page already being served somewhere.
    Page(Url),
}

/// Decide what the caller named.
///
/// A filesystem path is tried first, and only a string that is not a path is
/// handed to the address-bar rule. That order matters: `dist` is a directory
/// here and a bare hostname to `nav::request_from_input`, and guessing wrong
/// means fetching `https://dist/` and reporting whatever comes back as the page
/// under test.
pub fn classify(target: &str) -> Result<Target, String> {
    let path = Path::new(target);
    if path.exists() {
        let canonical = path
            .canonicalize()
            .map_err(|error| format!("could not resolve {target}: {error}"))?;
        if canonical.is_dir() {
            if !canonical.join("index.html").is_file() {
                return Err(format!(
                    "{} is a directory with no index.html in it",
                    canonical.display()
                ));
            }
            return Ok(Target::Directory(canonical));
        }
        // A built page needs an origin exactly as a whole site does, so its
        // directory is served and the page opened on it. Only a document:
        // anything else named directly is taken as given.
        if canonical
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("html") || ext.eq_ignore_ascii_case("htm"))
            && let (Some(root), Some(page)) = (
                canonical.parent(),
                canonical.file_name().and_then(|name| name.to_str()),
            )
        {
            return Ok(Target::File {
                root: root.to_path_buf(),
                page: page.to_owned(),
            });
        }
        return Url::from_file_path(&canonical)
            .map(Target::Page)
            .map_err(|()| format!("{} is not a path a URL can name", canonical.display()));
    }

    crate::nav::request_from_input(target)
        .map(|request| Target::Page(request.url))
        .ok_or_else(|| format!("{target} is neither a path that exists nor a URL"))
}

#[derive(Default)]
struct ActionContext {
    target_tag: Option<String>,
    target_type: String,
    has_form: bool,
    may_submit: bool,
    sensitive: bool,
    download: bool,
    new_window: bool,
    href: Option<String>,
    form_action: Option<String>,
}

fn bounded_descendant_text(
    document: &blitz_dom::BaseDocument,
    root: NodeId,
    max_chars: usize,
) -> String {
    let mut text = String::new();
    let mut pending = vec![(root, 0_u8)];
    while let Some((node_id, depth)) = pending.pop() {
        if text.chars().count() >= max_chars || pending.len() >= 96 {
            break;
        }
        let Some(node) = document.get_node(node_id) else {
            continue;
        };
        match &node.data {
            blitz_dom::NodeData::Text(text_node) => {
                for character in text_node.content.chars() {
                    if text.chars().count() >= max_chars {
                        break;
                    }
                    text.push(character);
                }
            }
            _ if depth < 3 => {
                for child in node.children.iter().rev().take(96) {
                    pending.push((*child, depth + 1));
                }
            }
            _ => {}
        }
    }
    text
}

fn sensitive_surface(value: &str) -> bool {
    fn word_is(word: &str, expected: &str) -> bool {
        word.eq_ignore_ascii_case(expected)
    }

    fn pair_is(first: &str, second: &str, expected: (&str, &str)) -> bool {
        word_is(first, expected.0) && word_is(second, expected.1)
    }

    let mut previous = "";
    let mut older = "";
    for word in value
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
    {
        let restricted_word = [
            "signin",
            "login",
            "password",
            "payment",
            "checkout",
            "reserve",
            "reservation",
            "purchase",
            "register",
            "billing",
            "email",
            "username",
            "phone",
            "telephone",
            "passport",
            "otp",
            "cc",
            "cvv",
            "cvc",
            "book",
        ]
        .iter()
        .any(|restricted| word_is(word, restricted))
            || word.len() >= "booking".len()
                && word
                    .get(.."booking".len())
                    .is_some_and(|prefix| word_is(prefix, "booking"));
        let restricted_pair = [
            ("sign", "in"),
            ("log", "in"),
            ("pay", "now"),
            ("place", "order"),
            ("create", "account"),
            ("account", "creation"),
            ("sign", "up"),
            ("current", "password"),
            ("new", "password"),
            ("cc", "number"),
            ("credit", "card"),
            ("card", "number"),
            ("first", "name"),
            ("last", "name"),
            ("full", "name"),
            ("given", "name"),
            ("family", "name"),
            ("passenger", "name"),
            ("traveler", "name"),
            ("traveller", "name"),
            ("birth", "date"),
        ]
        .iter()
        .any(|expected| pair_is(previous, word, *expected));
        let restricted_triple = (pair_is(older, previous, ("one", "time"))
            && word_is(word, "code"))
            || (pair_is(older, previous, ("date", "of")) && word_is(word, "birth"));
        if restricted_word || restricted_pair || restricted_triple {
            return true;
        }
        older = previous;
        previous = word;
    }
    false
}

fn action_context(document: &ScriptDocument, node_id: u64) -> Option<ActionContext> {
    const POLICY_ATTRIBUTES: [&str; 13] = [
        "type",
        "name",
        "id",
        "role",
        "aria-label",
        "title",
        "placeholder",
        "autocomplete",
        "href",
        "action",
        "formaction",
        "method",
        "formmethod",
    ];

    let inner = document.inner();
    let mut context = ActionContext::default();
    let mut current = Some(NodeId::from_u64(node_id));
    let mut depth = 0_u8;
    let mut has_button_role = false;
    let mut has_click_handler = false;
    let mut has_submit_control = false;

    while let Some(id) = current {
        if depth >= 16 {
            break;
        }
        let node = inner.get_node(id)?;
        if let Some(element) = node.data.downcast_element() {
            let tag = element.name.local.to_string().to_ascii_lowercase();
            if context.target_tag.is_none() {
                context.target_tag = Some(tag.clone());
            }
            let attribute = |name: &str| {
                element
                    .attrs
                    .iter()
                    .find(|attribute| attribute.name.local.as_ref() == name)
                    .map(|attribute| attribute.value.as_ref())
            };
            let element_type = attribute("type").unwrap_or_default().to_ascii_lowercase();
            if depth == 0 {
                context.target_type = element_type.clone();
            }
            for name in POLICY_ATTRIBUTES {
                if let Some(value) = attribute(name) {
                    context.sensitive |= sensitive_surface(value);
                }
            }
            context.sensitive |= sensitive_surface(&bounded_descendant_text(&inner, id, 192));

            context.has_form |= tag == "form" || attribute("form").is_some();
            if context.form_action.is_none() {
                context.form_action = attribute("formaction").map(str::to_owned);
            }
            if tag == "form" && context.form_action.is_none() {
                context.form_action = attribute("action").map(str::to_owned);
            }
            if context.href.is_none() && tag == "a" {
                context.href = attribute("href").map(str::to_owned);
            }
            context.download |= attribute("download").is_some();
            context.new_window |= attribute("target") == Some("_blank");
            has_button_role |= attribute("role").is_some_and(|role| {
                role.eq_ignore_ascii_case("button") || role.eq_ignore_ascii_case("menuitem")
            });
            has_click_handler |= attribute("onclick").is_some();
            has_submit_control |= (tag == "button"
                && !matches!(element_type.as_str(), "button" | "reset"))
                || (tag == "input" && matches!(element_type.as_str(), "submit" | "image"));
        }
        current = node.parent;
        depth = depth.saturating_add(1);
    }

    context.may_submit =
        context.has_form && (has_submit_control || has_button_role || has_click_handler);
    Some(context)
}

fn policy_denial(message: &str) -> DebugError {
    DebugError {
        code: "policyDenied".to_owned(),
        message: message.to_owned(),
    }
}

fn submit_needs_permission(policy: &HeadlessPolicy, context: &ActionContext) -> bool {
    context.may_submit && !policy.allow_form_submit
}

fn validate_action_url(
    policy: &HeadlessPolicy,
    document: &ScriptDocument,
    value: &str,
) -> Result<(), DebugError> {
    let destination = document
        .inner()
        .url()
        .join(value)
        .map_err(|_| policy_denial("the action URL is invalid"))?;
    policy
        .validate_url(&destination)
        .map_err(|_| policy_denial("the action URL is outside the browser policy"))
}

fn authorize_action(
    policy: &HeadlessPolicy,
    document: &ScriptDocument,
    action: &AgentAction,
) -> Result<(), DebugError> {
    if !policy.bot_child {
        return Ok(());
    }
    match action {
        AgentAction::Click { node_id } => {
            let context = action_context(document, *node_id)
                .ok_or_else(|| policy_denial("the live target could not be inspected"))?;
            if context.sensitive {
                return Err(policy_denial("this control belongs to a restricted action"));
            }
            if context.download || context.new_window {
                return Err(policy_denial(
                    "downloads and new browsing contexts are disabled",
                ));
            }
            if submit_needs_permission(policy, &context) {
                return Err(policy_denial(
                    "form submission requires explicit permission from the caller",
                ));
            }
            if let Some(href) = context.href.as_deref() {
                validate_action_url(policy, document, href)?;
            }
            if context.may_submit
                && let Some(action_url) = context.form_action.as_deref()
                && !action_url.is_empty()
            {
                validate_action_url(policy, document, action_url)?;
            }
            Ok(())
        }
        AgentAction::SetValue { node_id, value } => {
            if value.len() > BOT_CHILD_MAX_FILL_BYTES {
                return Err(policy_denial("the field value exceeds the input limit"));
            }
            let context = action_context(document, *node_id)
                .ok_or_else(|| policy_denial("the live target could not be inspected"))?;
            if context.sensitive {
                return Err(policy_denial("this field belongs to a restricted action"));
            }
            let editable = match context.target_tag.as_deref() {
                Some("textarea" | "select") => true,
                Some("input") => matches!(
                    context.target_type.as_str(),
                    "" | "text" | "search" | "date" | "month" | "week" | "time" | "number"
                ),
                _ => false,
            };
            if !editable {
                return Err(policy_denial("only ordinary search fields may be filled"));
            }
            Ok(())
        }
        AgentAction::ScrollIntoView { .. } | AgentAction::ScrollBy { .. } => Ok(()),
        _ => Err(policy_denial(
            "this action is not part of the bot browser control surface",
        )),
    }
}

fn sanitize_inspection(
    policy: &HeadlessPolicy,
    document: &ScriptDocument,
    response: DebugResponse,
) -> DebugResponse {
    if !policy.bot_child {
        return response;
    }
    let mut snapshot = match response {
        DebugResponse::AgentSnapshot(snapshot) => snapshot,
        response => return response,
    };
    snapshot.nodes.truncate(BOT_CHILD_MAX_INSPECT_NODES);
    for node in &mut snapshot.nodes {
        if action_context(document, node.id).is_some_and(|context| context.sensitive) {
            node.value = None;
        }
    }
    bound_inspection_snapshot(&mut snapshot);
    DebugResponse::AgentSnapshot(snapshot)
}

fn condense_response(
    policy: &HeadlessPolicy,
    level: condense::Level,
    options: condense::CondenseOptions,
    inspect: impl FnOnce() -> DebugResponse,
) -> DebugResponse {
    // Condense reads raw semantic-node values. Do not let it bypass the
    // bot-child depth, redaction, and output-size restrictions on Inspect.
    if policy.bot_child {
        return DebugResponse::Error(policy_denial(
            "page condensation is disabled in bot-child mode",
        ));
    }

    match inspect() {
        DebugResponse::AgentSnapshot(snapshot) => {
            DebugResponse::Condensation(level.select(condense::condense(&snapshot.nodes, options)))
        }
        _ => DebugResponse::Error(DebugError {
            code: "unsupported".into(),
            message: "the document has no semantic tree to condense".into(),
        }),
    }
}

fn bound_inspection_snapshot(snapshot: &mut blitz_control_protocol::AgentSnapshot) {
    fn trim_text(value: &mut String, field_limit: usize, remaining: &mut usize) {
        let mut end = value.len().min(field_limit).min(*remaining);
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        value.truncate(end);
        *remaining -= end;
    }

    fn trim_optional_text(value: &mut Option<String>, field_limit: usize, remaining: &mut usize) {
        if let Some(value) = value {
            trim_text(value, field_limit, remaining);
        }
    }

    let mut remaining = BOT_CHILD_MAX_INSPECT_BYTES;
    trim_optional_text(&mut snapshot.active_window, 128, &mut remaining);
    for node in &mut snapshot.nodes {
        trim_optional_text(&mut node.dom_id, 128, &mut remaining);
        trim_text(&mut node.role, 64, &mut remaining);
        trim_text(&mut node.name, 512, &mut remaining);
        trim_optional_text(&mut node.value, 512, &mut remaining);
        trim_optional_text(&mut node.slot, 128, &mut remaining);
    }

    // String contents are bounded before encoding, so these checks allocate
    // only a small temporary buffer while enforcing the actual wire size.
    loop {
        match serde_json::to_vec(snapshot) {
            Ok(encoded) if encoded.len() <= BOT_CHILD_MAX_INSPECT_BYTES => break,
            Ok(_) if !snapshot.nodes.is_empty() => {
                snapshot.nodes.pop();
            }
            _ => {
                snapshot.nodes.clear();
                snapshot.active_window = None;
                break;
            }
        }
    }
}

#[derive(Default)]
struct RequestBudget {
    requests: usize,
    actions: usize,
    inspections: usize,
}

impl RequestBudget {
    fn admit(
        &mut self,
        policy: &HeadlessPolicy,
        request: &ControlBridgeRequest,
    ) -> Result<(), DebugError> {
        if !policy.bot_child {
            return Ok(());
        }
        if self.requests >= BOT_CHILD_MAX_REQUESTS {
            return Err(policy_denial("the browser request budget is exhausted"));
        }
        self.requests += 1;
        match request {
            ControlBridgeRequest::Agent(AgentControlRequest::Inspect { .. }) => {
                if self.inspections >= BOT_CHILD_MAX_INSPECTIONS {
                    return Err(policy_denial("the inspection budget is exhausted"));
                }
                self.inspections += 1;
            }
            ControlBridgeRequest::Agent(AgentControlRequest::Act(_)) => {
                if self.actions >= BOT_CHILD_MAX_ACTIONS {
                    return Err(policy_denial("the action budget is exhausted"));
                }
                self.actions += 1;
            }
            _ => {}
        }
        Ok(())
    }
}

/// Drain synchronous script and reactive work without imposing a timer on every
/// control.
///
/// Delayed outcomes are polled by `ps-qa` against the exact declared verdict,
/// so sleeping here only makes fast controls slow and duplicates the caller's
/// timeout.
///
/// The deadline is a work budget, not a verdict. Past it the loop stops
/// draining script, resolves what the page has, and answers; the remaining
/// work runs on later host ticks. A slow layout (a large SVG map after a
/// route change) is a slow page, not a dead one: reporting it as
/// `pageUnresponsive` made the supervisor tear the session down. Hangs are
/// still caught by the control request and Poll deadlines.
fn settle_immediate(
    document: &mut ScriptDocument,
    clock: &std::time::Instant,
    deadline: std::time::Duration,
) -> bool {
    let before = document.inner().paint_damage().generation;
    let started = std::time::Instant::now();
    let mut iterations = 0_u32;
    loop {
        if started.elapsed() >= deadline {
            verbose(&format!(
                "settle budget of {}ms spent after {iterations} iterations; answering with pending work",
                deadline.as_millis()
            ));
            break;
        }
        let progressed = {
            let _turn = supervisor::operation(supervisor::Operation::Poll);
            // Settle the action's immediate checkpoint and DOM. Starting a
            // timer here can start an unrelated mesh build or animation while
            // the caller is waiting for its action acknowledgement. Timers run
            // on host ticks, between control requests.
            document.poll_for_settle(None, false)
        };
        iterations = iterations.saturating_add(1);
        // A yielded promise slice is progress, not a failure to settle:
        // resolve the partial DOM and answer this request before the next one.
        if document.has_pending_jobs() {
            break;
        }
        if started.elapsed() >= deadline {
            continue;
        }
        if !progressed {
            break;
        }
    }
    {
        let _resolve = supervisor::operation(supervisor::Operation::Resolve);
        document.inner_mut().resolve(clock.elapsed().as_secs_f64());
    }
    if started.elapsed() >= deadline {
        verbose(&format!(
            "settle took {}ms against a {}ms budget",
            started.elapsed().as_millis(),
            deadline.as_millis()
        ));
    }
    document.inner().paint_damage().generation != before
}

/// How long the loop waits for a request before letting the page run.
///
/// Short enough that a page which is waiting on a timer or a response is not
/// noticeably slowed by it, long enough that an idle host is not a spin.
const IDLE_TICK: std::time::Duration = std::time::Duration::from_millis(8);

/// How many missed turns one reply may hand back to the page.
///
/// A request that took a long time owes the page the turns it spent, but the
/// debt has to end somewhere: without a cap, a single slow snapshot would let
/// the page run unbounded before the next request is even read, which is the
/// starvation this fixes pointed the other way.
const MAX_CATCHUP_TURNS: u32 = 64;

/// How long `WaitForIdle` keeps running page jobs before it answers anyway.
const IDLE_WAIT_LIMIT: std::time::Duration = std::time::Duration::from_secs(5);

/// `document` and `paint` count committed renders; `style` and `layout`
/// count the Stylo traversals and layout passes blitz-dom actually ran.
fn headless_revisions(rendered: u64, page: &ScriptDocument) -> RevisionSet {
    let inner = page.inner();
    RevisionSet {
        document: rendered,
        style: inner.style_revision(),
        layout: inner.layout_revision(),
        paint: rendered,
    }
}

/// Let the page get on with what it started, while nothing is being asked of it.
///
/// Without this the document only advances inside a request. A page whose
/// bootstrap is a chain of asynchronous steps -- fetch a version, append a
/// script, wait for its `load`, mount -- gets exactly as far as the last step
/// that finished before the loader went quiet, and then stops. That is not a
/// slow page: it is a stopped one, and it reads as a site that renders nothing.
/// nofilter.io is the case that found it, and it stayed at 23 nodes for as long
/// as it was left running.
///
/// Returns whether the page painted.
fn tick_document(document: &mut ScriptDocument, clock: &std::time::Instant) -> bool {
    let before = document.inner().paint_damage().generation;
    // One due timer or one continuing checkpoint per poll. Return to the
    // request receiver before another timer starts. Boa's execution limit
    // interrupts runaway script; the supervisor remains the native hang guard.
    let poll_started = std::time::Instant::now();
    let progressed = {
        let _turn = supervisor::operation(supervisor::Operation::Poll);
        document.poll(Some(supervisor::task_context()))
    };
    let poll_elapsed = poll_started.elapsed();
    let resolve_started = std::time::Instant::now();
    {
        let _resolve = supervisor::operation(supervisor::Operation::Resolve);
        document.inner_mut().resolve(clock.elapsed().as_secs_f64());
    }
    let resolve_elapsed = resolve_started.elapsed();
    let painted = document.inner().paint_damage().generation != before;
    if progressed || painted {
        verbose(&format!(
            "tick progressed={progressed} painted={painted} poll={}ms resolve={}ms jobs_pending={}",
            poll_elapsed.as_millis(),
            resolve_elapsed.as_millis(),
            document.has_pending_jobs(),
        ));
    }
    painted
}

fn settle_response(
    document: &mut ScriptDocument,
    clock: &std::time::Instant,
    deadline: std::time::Duration,
    painted: &mut bool,
) -> DebugResponse {
    *painted = settle_immediate(document, clock, deadline);
    DebugResponse::Ack
}

/// Where a link click asks to go.
///
/// A plain `<a href>` is not the router's: `@solidjs/router` intercepts only its
/// own `<A>`, so every `Button href=` and `Link href=` in the fleet is an
/// ordinary anchor whose activation is the shell's to carry out. The windowed
/// browser has a shell. Without one the click is dispatched, acknowledged, and
/// nothing moves, which reads as a dead control rather than a missing
/// capability -- and it is most of the navigation on every site here.
///
/// A slot rather than a queue. Two navigations before the loop looks again
/// means the second won, exactly as it would in a browser, and a queue would
/// replay a page the user never waited for.
struct RequestedNavigation {
    destination: Mutex<Option<Url>>,
    refused: Mutex<bool>,
    policy: HeadlessPolicy,
}

impl RequestedNavigation {
    fn new(policy: HeadlessPolicy) -> Self {
        Self {
            destination: Mutex::new(None),
            refused: Mutex::new(false),
            policy,
        }
    }

    fn take(&self) -> Option<Url> {
        self.destination
            .lock()
            .ok()
            .and_then(|mut slot| slot.take())
    }

    fn take_refusal(&self) -> bool {
        self.refused
            .lock()
            .map(|mut refused| std::mem::take(&mut *refused))
            .unwrap_or(false)
    }
}

/// A clipboard, because a page with no window still has one.
///
/// The default `ShellProvider` refuses both directions, which is right for a
/// provider that has no shell to ask. It is wrong for a page: the usual copy
/// button is `await navigator.clipboard.writeText(t)` followed by the only
/// feedback a clipboard write ever has, and a rejected promise skips that line
/// and leaves an unhandled rejection behind. Under Solid 2 an error that
/// escapes every boundary halts the scheduler outright, so a headless run of a
/// page with a copy button is one press away from a frozen application that
/// still paints.
///
/// In memory and per host, which is the honest scope: nothing here reaches the
/// machine's clipboard, and a check that reads it back is reading what the page
/// wrote rather than what the operating system holds.
#[derive(Default)]
struct HostClipboard(std::sync::Mutex<String>);

impl blitz_traits::shell::ShellProvider for HostClipboard {
    fn get_clipboard_text(&self) -> Result<String, blitz_traits::shell::ClipboardError> {
        self.0
            .lock()
            .map(|held| held.clone())
            .map_err(|_| blitz_traits::shell::ClipboardError)
    }

    fn set_clipboard_text(&self, text: String) -> Result<(), blitz_traits::shell::ClipboardError> {
        let mut held = self
            .0
            .lock()
            .map_err(|_| blitz_traits::shell::ClipboardError)?;
        *held = text;
        Ok(())
    }
}

impl blitz_traits::navigation::NavigationProvider for RequestedNavigation {
    fn navigate_to(&self, options: blitz_traits::navigation::NavigationOptions) {
        if self.policy.validate_url(&options.url).is_err() {
            if let Ok(mut refused) = self.refused.lock() {
                *refused = true;
            }
            return;
        }
        if let Ok(mut slot) = self.destination.lock() {
            *slot = Some(options.url);
        }
    }
}

/// What the page put in storage, carried from one document to the next.
///
/// The shim's `localStorage` is in-memory and per document, which is right for
/// a capture and wrong the moment the host follows a link: an application
/// writes its settings on one route and reads them while booting the next, and
/// a fresh store makes that read a miss. Every check of the shape "save, go
/// somewhere, come back" is undecidable without this.
///
/// It is a copy, not real storage. Nothing here is on disk, no quota is
/// enforced, and no `storage` event is delivered; what it buys is that the same
/// origin sees what it wrote.
struct StoredState {
    origin: String,
    local: String,
    session: String,
}

/// Read both stores out of the document that is about to be replaced.
///
/// Through the public interface -- `length`, `key`, `getItem` -- rather than
/// through the shim's internals, so this keeps working the day storage stops
/// being a closure over an object.
fn snapshot_storage(document: &mut ScriptDocument, origin: &str) -> StoredState {
    let _storage = supervisor::operation(supervisor::Operation::Storage);
    fn dump(document: &mut ScriptDocument, store: &str) -> String {
        let script = format!(
            "(function () {{
               try {{
                 var out = {{}};
                 for (var i = 0; i < {store}.length; i++) {{
                   var key = {store}.key(i);
                   if (key !== null) {{ out[key] = {store}.getItem(key); }}
                 }}
                 return JSON.stringify(out);
               }} catch (error) {{ return '{{}}'; }}
             }})()"
        );
        document.eval(&format!("globalThis.__chuzz_dump = {script};"));
        document
            .eval_json("globalThis.__chuzz_dump")
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned))
            .unwrap_or_else(|| "{}".to_owned())
    }

    StoredState {
        origin: origin.to_owned(),
        local: dump(document, "localStorage"),
        session: dump(document, "sessionStorage"),
    }
}

/// The prelude that puts it back, or nothing when it does not belong here.
///
/// Storage is keyed by origin in a browser and is keyed by origin here. A link
/// that leaves the site under test lands on a page that must not see the site's
/// keys, and carrying them across would be a leak invented by the harness.
fn restore_script(stored: Option<&StoredState>, destination: &Url) -> String {
    let Some(stored) =
        stored.filter(|stored| stored.origin == destination.origin().ascii_serialization())
    else {
        return String::new();
    };
    format!(
        "(function () {{
           try {{
             var local = JSON.parse({local});
             for (var key in local) {{ localStorage.setItem(key, local[key]); }}
             var session = JSON.parse({session});
             for (var key in session) {{ sessionStorage.setItem(key, session[key]); }}
           }} catch (error) {{}}
         }})();",
        // Serialized twice on purpose: the inner JSON is the data, and the
        // outer encoding makes it a JavaScript string literal that cannot end
        // the statement early. A page's own key is untrusted input here.
        local = serde_json::to_string(&stored.local).unwrap_or_else(|_| "\"{}\"".to_owned()),
        session = serde_json::to_string(&stored.session).unwrap_or_else(|_| "\"{}\"".to_owned()),
    )
}

fn display_url(url: &Url, hide_path: bool) -> String {
    if hide_path {
        return url.origin().ascii_serialization();
    }
    let mut display = url.clone();
    let _ = display.set_username("");
    let _ = display.set_password(None);
    display.set_query(None);
    display.set_fragment(None);
    display.to_string()
}

fn commit_render(events: &Latest<DebugEvent>, revision: &mut u64) {
    *revision = revision.saturating_add(1);
    events.set_now(DebugEvent::PaintCommitted {
        revision: *revision,
    });

}

/// A data: page that names the URL that failed and why.
///
/// The text is escaped for HTML, and a percent sign is written as an entity so
/// the data: URL decoder cannot read it as an escape.
fn failure_page_url(url: &str, error: &str) -> Result<Url, String> {
    let escape = |text: &str| {
        text.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('%', "&#37;")
    };
    let html = format!(
        "<!doctype html><title>Page failed to load</title><h1>This page could not be loaded</h1><p>{}</p><p id=chuzz-load-error>{}</p>",
        escape(url),
        escape(error),
    );
    let mut page =
        Url::parse("data:text/html;charset=utf-8,").map_err(|error| error.to_string())?;
    page.set_path(&format!("text/html;charset=utf-8,{html}"));
    Ok(page)
}

fn follow_navigation(
    destination: Url,
    document: &mut ScriptDocument,
    origin: &mut String,
    policy: &HeadlessPolicy,
    load: &impl Fn(Url, Option<&StoredState>, blitz_traits::net::Initiator) -> Result<Box<ScriptDocument>, String>,
    render: (&Latest<DebugEvent>, &mut u64),
    browser_initiated: bool,
) -> Result<(), String> {
    let _navigation = supervisor::operation(supervisor::Operation::Load);
    policy.validate_url(&destination)?;
    document.eval(
        "globalThis.dispatchEvent(new Event('pagehide')); \
         globalThis.dispatchEvent(new Event('unload'));",
    );
    let carried = if policy.bot_child {
        None
    } else {
        Some(snapshot_storage(document, origin))
    };
    let initiator = if browser_initiated {
        Arc::new(blitz_traits::net::ResourcePolicy::default()).browser(destination.clone())
    } else {
        document.inner().net_provider.resource_initiator()
    };
    let next = load(destination, carried.as_ref(), initiator)?;
    let arriving = next.inner().url().origin().ascii_serialization();
    *document = *next;
    *origin = arriving;
    supervisor::ready(document.inner().url());
    commit_render(render.0, render.1);
    Ok(())
}

/// `localStorage` seed for the first document, or nothing when the variable is
/// unset.
///
/// Read once per process. The supervisor validates before it announces a
/// descriptor, and the page worker applies the map through the storage
/// prelude. A bad file exits 2 here so the check cannot run without its state.
fn read_qa_inspect_storage() -> Option<std::collections::BTreeMap<String, String>> {
    let path = match std::env::var("QA_INSPECT_STORAGE") {
        Ok(path) if !path.is_empty() => path,
        Ok(_) => fail_qa_inspect_storage("path is empty"),
        Err(std::env::VarError::NotPresent) => return None,
        Err(error) => fail_qa_inspect_storage(&error.to_string()),
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) => fail_qa_inspect_storage(&format!("{path}: {error}")),
    };
    match serde_json::from_str::<std::collections::BTreeMap<String, String>>(&text) {
        Ok(map) => Some(map),
        Err(error) => fail_qa_inspect_storage(&error.to_string()),
    }
}

fn fail_qa_inspect_storage(error: &str) -> ! {
    eprintln!("chuzz-headless: QA_INSPECT_STORAGE: {error}");
    std::process::exit(2);
}

/// Load `target` and serve it over the inspection socket until killed.
pub fn serve(target: &str) -> Result<(), String> {
    crate::sandbox::ensure()?;
    let storage = read_qa_inspect_storage();
    serve_page(target, storage.as_ref())
}

fn serve_page(
    target: &str,
    seeded: Option<&std::collections::BTreeMap<String, String>>,
) -> Result<(), String> {
    // Start the private reader before loading, so losing the supervisor also
    // stops a worker wedged during bootstrap.
    let policy = HeadlessPolicy::from_env()?;
    let capacity = if policy.bot_child { BOT_CHILD_MAX_PENDING_REQUESTS } else { 64 };
    let (request_tx, request_rx) = page_control::channel(capacity);
    let _egress_proxy = if policy.bot_child {
        Some(EgressProxy::start(policy.clone())?)
    } else {
        None
    };
    let _wall_timeout = if policy.bot_child {
        Some(WallTimeoutGuard::start(policy.wall_time)?)
    } else {
        None
    };
    supervisor::start_worker(request_tx)?;
    let target = classify(target).map_err(|error| {
        if policy.bot_child {
            let _ = error;
            "bot mode could not classify its launch target".to_owned()
        } else {
            error
        }
    })?;
    policy.validate_target(&target)?;
    let (width, height) = policy.view_dimensions()?;
    let settle_deadline =
        std::time::Duration::from_millis(u64::from(dimension("QA_HOST_SETTLE_MS", 100)?.min(1000)));

    // Keep the process worker pool and socket reactor alive for the whole run.
    let _runtime = nagoya::runtime::background();
    let _reactor = nagoya::reactor::Reactor::start()
        .map_err(|error| format!("could not start the network reactor: {error}"))?;

    // A directory becomes a site before it becomes a document: its markup names
    // `/static/...` absolutely and its router owns paths that were never built
    // as files, neither of which a `file://` base can answer.
    let url = match &target {
        Target::Directory(root) => {
            let origin = nagoya::block_on(crate::page_server::start(root))?;
            trace(&format!("serving {} at {origin}", root.display()));
            // A route can be opened directly rather than navigated to.
            //
            // A built single-page application has one file and many addresses:
            // `/spec` and `/designer` are the router's, not the filesystem's,
            // and the server already answers them with the document so the
            // router resolves them exactly as a production host does. Without
            // this the only way to reach such a page was to press whatever
            // links to it, which makes every check on a deep route depend on
            // the navigation above it and leaves a route with no link
            // unreachable altogether.
            let path = std::env::var("QA_HOST_PATH").unwrap_or_default();
            let target = format!("{origin}{}", path.trim_start_matches('/'));
            Url::parse(&target).map_err(|error| format!("{target} is not a URL: {error}"))?
        }
        Target::File { root, page } => {
            let origin = nagoya::block_on(crate::page_server::start(root))?;
            trace(&format!("serving {} at {origin}", root.display()));
            Url::parse(&format!("{origin}{page}"))
                .map_err(|error| format!("{origin}{page} is not a URL: {error}"))?
        }
        Target::Page(url) => url.clone(),
    };


    let cookies = if policy.bot_child {
        Arc::new(crate::cookie_store::BrowserCookieStore::default())
    } else {
        let _storage = supervisor::operation(supervisor::Operation::Storage);
        Arc::new(
            nagoya::block_on(crate::cookie_store::BrowserCookieStore::open(
                crate::cookie_store::profile_directory().join("cookies"),
            ))
            .map_err(|error| format!("could not open browser cookies: {error}"))?,
        )
    };
    let net_provider = Arc::new(
        crate::document_loader::NetProvider::with_user_agent_and_cookies(
            Some(supervisor::network_wake()),
            &crate::identity::user_agent_from_env(),
            cookies,
        ),
    );
    let navigation = Arc::new(RequestedNavigation::new(policy.clone()));
    // One clipboard for the host, not one per document, so a page that copies
    // on one route and reads it back on another sees what it wrote.
    let clipboard = Arc::new(HostClipboard::default());
    let animation_clock = std::time::Instant::now();

    let tls_status = std::cell::RefCell::new(None::<(Url, Option<blitz_net::TlsPolicyError>)>);
    let load = |url: Url, carried: Option<&StoredState>, initiator: blitz_traits::net::Initiator| -> Result<Box<ScriptDocument>, String> {
        // Includes HTML and external-script fetches, script execution, loader
        // pumping, shim evaluation and the initial viewport settle.
        let _load = supervisor::operation(supervisor::Operation::Load);
        policy.validate_url(&url)?;
        trace(&format!("loading {}", display_url(&url, policy.bot_child)));
        let mut prelude = if policy.bot_child {
            String::new()
        } else {
            restore_script(carried, &url)
        };
        // Diagnosis only: a script file run before the page's own scripts, to instrument a site
        // whose failure no standalone repro shows. Never read for a bot child.
        if !policy.bot_child
            && let Ok(path) = std::env::var("CHUZZ_DEBUG_PRELUDE")
        {
            let script = std::fs::read_to_string(&path)
                .map_err(|error| format!("CHUZZ_DEBUG_PRELUDE {path}: {error}"))?;
            prelude.push('\n');
            prelude.push_str(&script);
        }
        let mut policy_error = None;
        let mut document = if let Some(html) = crate::internal_pages::html_for(&url) {
            Box::new(ScriptDocument::from_html(
                &html,
                blitz_dom::DocumentConfig {
                    base_url: Some(url.to_string()),
                    ..Default::default()
                },
            ))
        } else {
            let loaded = nagoya::block_on(crate::document_loader::load_for_capture(
                blitz_traits::net::Request::get(url.clone()).initiator(initiator),
                Arc::clone(&net_provider),
                &prelude,
                Some(Viewport::new(width, height, 1.0, ColorScheme::Dark)),
            ));
            match loaded {
                Ok(loaded) => loaded.into_script().ok_or(
                    "this build cannot run scripts, so it cannot host a page worth inspecting",
                )?,
                Err(error) => {
                    if !policy.bot_child
                        && let Some(blitz_traits::platform::FetchError::TlsPolicy(reason)) =
                            error.downcast_ref::<blitz_traits::platform::FetchError>()
                    {
                        policy_error = Some(*reason);
                        trace(&format!("TLS refused for {url}: {reason}"));
                        Box::new(ScriptDocument::from_html(
                            &crate::internal_pages::tls_refused_html(&url, *reason),
                            blitz_dom::DocumentConfig {
                                base_url: Some(url.to_string()),
                                ..Default::default()
                            },
                        ))
                    } else {
                        return Err(if policy.bot_child {
                            format!(
                                "could not load {}: network request failed",
                                display_url(&url, policy.bot_child)
                            )
                        } else {
                            format!(
                                "could not load {}: {error}",
                                display_url(&url, policy.bot_child)
                            )
                        });
                    }
                }
            }
        };
        let landed_url = document.inner().url().clone();
        policy
            .validate_url(&landed_url)
            .map_err(|_| "the loaded page redirected outside the browser policy".to_owned())?;
        if policy.bot_child && document.inner().tree().iter().count() > BOT_CHILD_MAX_PAGE_NODES {
            return Err("the page exceeded the bot browser node limit".to_owned());
        }
        document
            .inner_mut()
            .set_viewport(Viewport::new(width, height, 1.0, ColorScheme::Dark));
        document.inner_mut().set_paint_damage_tracking(true);
        document
            .inner_mut()
            .set_navigation_provider(Arc::clone(&navigation) as _);
        document
            .inner_mut()
            .set_shell_provider(Arc::clone(&clipboard) as _);
        // The loader already ran and pumped the page's scripts. This settles
        // what the viewport change queued, rather than sleeping for a fixed
        // interval before announcing the socket. A slow first layout is not a
        // dead page: the load already finished inside its own budget, and a
        // large document (YouTube's) can take longer than one control
        // request's settle budget to lay out. Say so and serve it; requests
        // keep their own deadlines.
        settle_immediate(&mut document, &animation_clock, settle_deadline);
        *tls_status.borrow_mut() = Some((landed_url.clone(), policy_error));
        Ok(document)
    };

    // Which origin the standing document belongs to, so what it stored is
    // offered back only to itself. A check seed takes the same prelude as
    // storage carried across navigations, so the first document sees it
    // before its own scripts.
    let seed = match seeded {
        None => None,
        Some(entries) => {
            let local = match serde_json::to_string(entries) {
                Ok(local) => local,
                Err(error) => fail_qa_inspect_storage(&error.to_string()),
            };
            Some(StoredState {
                origin: url.origin().ascii_serialization(),
                local,
                session: "{}".to_owned(),
            })
        }
    };
    let failed_url = display_url(&url, policy.bot_child);
    let initial = net_provider.browser_initiator(url.clone());
    let mut document = match load(url, seed.as_ref(), initial) {
        Ok(document) => document,
        // A page that cannot load is still a page, as in any browser: show why
        // and keep the inspection socket up, so the caller can read the reason,
        // navigate elsewhere or close. A bot child keeps the exit its
        // supervisor expects.
        Err(error) if !policy.bot_child => {
            trace(&format!("top-level load failed: {error}"));
            let page = failure_page_url(&failed_url, &error)?;
            nagoya::block_on(crate::document_loader::load_for_capture(
                blitz_traits::net::Request::get(page.clone()).initiator(net_provider.browser_initiator(page)),
                Arc::clone(&net_provider),
                "",
                Some(Viewport::new(width, height, 1.0, ColorScheme::Dark)),
            ))
            .map_err(|page_error| {
                format!("{error}; the failure page did not load either: {page_error}")
            })?
            .into_script()
            .ok_or("this build cannot run scripts, so it cannot host a page worth inspecting")?
        }
        Err(error) => return Err(error),
    };
    let mut origin = document.inner().url().origin().ascii_serialization();
    trace("document ready");
    // Debugging aid for real sites: CHUZZ_DEBUG_EVAL names a script file that
    // runs once in the page after load. It can schedule its own timers and
    // report through console; nothing runs unless the variable is set.
    if let Some(path) = std::env::var_os("CHUZZ_DEBUG_EVAL") {
        match std::fs::read_to_string(&path) {
            Ok(script) => document.eval(&script),
            Err(error) => trace(&format!("CHUZZ_DEBUG_EVAL unreadable: {error}")),
        }
    }

    supervisor::ready(document.inner().url());
    let render_events = supervisor::events();
    if tick_document(&mut document, &animation_clock) {
        let _ = render_events.set_now(DebugEvent::PaintCommitted { revision: 0 });
    }

    let mut revision = 0_u64;
    let mut render_revision = 0_u64;
    let mut capture = DocumentCapture::new();
    let mut control = DocumentControl::new();
    let mut request_budget = RequestBudget::default();
    // When the page was last allowed to run. A request resets nothing on its
    // own, so this is what keeps the guarantee below true under load.
    let mut last_tick = std::time::Instant::now();
    let mut queued_request = None;
    loop {
        if policy.bot_child && document.inner().tree().iter().count() > BOT_CHILD_MAX_PAGE_NODES {
            trace("the page exceeded the bot browser node limit");
            break;
        }
        // Sleep only until the page has something to do. A fixed tick made
        // every `setTimeout(0)` continuation wait for the next frame (~20 ms
        // measured), so code that yields every few milliseconds sat idle most
        // of the time.
        let wait = if document.has_pending_jobs() {
            Some(std::time::Duration::ZERO)
        } else {
            document.next_timer_deadline().map(|due| due.saturating_duration_since(std::time::Instant::now()))
        };
        let received = match queued_request.take() {
            Some(pair) => Ok(pair),
            None => match wait {
                Some(timeout) => request_rx.recv_timeout(timeout),
                None => request_rx.receive(None),
            },
        };
        let (request, reply) = match received {
            Ok(pair) => pair,
            Err(page_control::RecvTimeoutError::Timeout) => {
                if tick_document(&mut document, &animation_clock) {
                    commit_render(&render_events, &mut render_revision);
                }
                last_tick = std::time::Instant::now();
                if let Some(destination) = navigation.take()
                    && let Err(error) = follow_navigation(
                        destination,
                        &mut document,
                        &mut origin,
                        &policy,
                        &load,
                        (&render_events, &mut render_revision),
                        false,
                    )
                {
                    trace(&format!("the navigation failed, staying put: {error}"));
                }
                continue;
            }
            Err(page_control::RecvTimeoutError::Disconnected) => break,
        };
        let (request, target_context) = match request {
            ControlBridgeRequest::Targeted(targeted) => {
                use blitz_control_protocol::{
                    TargetCapabilities, TargetCommand, TargetContext, TargetId, TargetInfo,
                };
                if let Err(error) = targeted.validate_options() {
                    reply.fill(DebugResponse::Error(error));
                    continue;
                }
                let context = TargetContext {
                    target_id: TargetId::Tab(0),
                    generation: (u64::from(std::process::id()) << 32)
                        | document.inner().id() as u64,
                    document_id: document.inner().id() as u64,
                };
                if matches!(
                    &targeted.command,
                    TargetCommand::Agent(AgentControlRequest::ListTargets)
                ) {
                    let title = document
                        .inner()
                        .query_selector("title")
                        .ok()
                        .flatten()
                        .and_then(|id| {
                            document
                                .inner()
                                .get_node(id)
                                .map(|node| node.text_content())
                        })
                        .unwrap_or_default();
                    reply.fill(DebugResponse::Targets(vec![TargetInfo {
                        id: context.target_id.clone(),
                        url: document.inner().url().to_string(),
                        title,
                        active: true,
                        generation: context.generation,
                        document_id: Some(context.document_id),
                        capabilities: TargetCapabilities {
                            agent: true,
                            diagnostics: !policy.bot_child,
                            capture: !policy.bot_child,
                            console: false,
                            runtime_errors: false,
                            web_gpu: !policy.bot_child,
                        },
                    }]));
                    continue;
                }
                if !targeted.instance_scoped()
                    && targeted
                        .options
                        .target
                        .as_ref()
                        .is_some_and(|target| target.id() != TargetId::Tab(0))
                {
                    reply.fill(DebugResponse::Error(blitz_control_protocol::target::error(
                        "unknownTarget",
                        "headless exposes only page 0",
                    )));
                    continue;
                }
                if !targeted.instance_scoped()
                    && let Err(error) = context.validate(&targeted)
                {
                    reply.fill(DebugResponse::Error(error));
                    continue;
                }
                if matches!(
                    &targeted.command,
                    TargetCommand::Diagnostics(DiagnosticsRequest::Observe { .. })
                ) {
                    reply.fill(DebugResponse::Ack.with_context(context));
                    continue;
                }
                let wrap_context = !targeted.instance_scoped();
                let request = match targeted.command {
                    TargetCommand::Agent(request) => ControlBridgeRequest::Agent(request),
                    TargetCommand::Diagnostics(request) => {
                        ControlBridgeRequest::Diagnostics(request)
                    }
                };
                (request, wrap_context.then_some(context))
            }
            request => (request, None),
        };
        if let Err(error) = request_budget.admit(&policy, &request) {
            reply.fill(DebugResponse::Error(error));
            trace("the bot browser request budget was exhausted");
            break;
        }
        let was_action = matches!(
            &request,
            ControlBridgeRequest::Agent(AgentControlRequest::Act(_))
        );
        verbose(match &request {
            ControlBridgeRequest::Agent(AgentControlRequest::Inspect { .. }) => "request inspect",
            ControlBridgeRequest::Agent(AgentControlRequest::Act(_)) => "request act",
            _ => "request other",
        });
        let request_turn = supervisor::operation(supervisor::Operation::Request);
        let mut painted = false;
        let deferred_buffer =
            if let ControlBridgeRequest::Diagnostics(DiagnosticsRequest::WebGpuBuffer {
                target,
                offset,
                length,
            }) = &request
                && !policy.bot_child
                && target_context.is_none()
            {
                let capture_reply = blitz_control_protocol::latest::Once::new();
                crate::webgpu::capture_gpu_buffer_reply(
                    &mut document,
                    target,
                    *offset,
                    *length,
                    Arc::clone(&capture_reply),
                );
                let page_reply = Arc::clone(&reply);
                drop(nagoya::spawn(async move {
                    if let Some(response) = capture_reply.recv().await {
                        page_reply.fill(response);
                    }
                    page_reply
                }));
                true
            } else {
                false
            };
        let mut response = if deferred_buffer {
            // The native worker owns this request's reply slot.
            DebugResponse::Ack
        } else {
            match request {
                ControlBridgeRequest::Agent(request) => match request {
                    AgentControlRequest::Inspect { root, max_depth } => {
                        revision += 1;
                        let depth = if policy.bot_child {
                            max_depth.min(BOT_CHILD_MAX_INSPECT_DEPTH)
                        } else {
                            max_depth
                        };
                        let response = inspect_document(&mut document, root, depth, revision);
                        let response = shadow_inspect::flatten(
                            &mut document,
                            response,
                            root,
                            depth as usize,
                            |document, id| inspect_document(document, Some(id), 1, revision),
                        );
                        sanitize_inspection(&policy, &document, response)
                    }
                    AgentControlRequest::Act(action) => {
                        let _action = supervisor::operation(supervisor::Operation::Action);
                        let _execution = document.enter_execution_time_limit();
                        match authorize_action(&policy, &document, &action) {
                            Ok(()) => match control.act(&mut document, action) {
                                Ok(()) => settle_response(
                                    &mut document,
                                    &animation_clock,
                                    settle_deadline,
                                    &mut painted,
                                ),
                                Err(error) => DebugResponse::Error(error),
                            },
                            Err(error) => DebugResponse::Error(error),
                        }
                    }
                    AgentControlRequest::Navigate { url } => {
                        // Resolved before the match, so the borrow of the standing
                        // document ends before it is replaced.
                        let resolved = document.inner().url().join(&url);
                        match resolved {
                            Ok(destination) => match follow_navigation(
                                destination,
                                &mut document,
                                &mut origin,
                                &policy,
                                &load,
                                (&render_events, &mut render_revision),
                                true,
                            ) {
                                Ok(()) => DebugResponse::Ack,
                                Err(message) => DebugResponse::Error(DebugError {
                                    code: "navigationFailed".into(),
                                    message,
                                }),
                            },
                            Err(error) => DebugResponse::Error(DebugError {
                                code: "invalidArgument".into(),
                                message: format!(
                                    "could not resolve {url:?} against the document URL: {error}"
                                ),
                            }),
                        }
                    }
                    AgentControlRequest::Condense { level, options } => {
                        condense_response(&policy, level, options, || {
                            revision += 1;
                            let response = inspect_document(&mut document, None, 0, revision);
                            shadow_inspect::flatten(
                                &mut document,
                                response,
                                None,
                                0,
                                |document, id| inspect_document(document, Some(id), 1, revision),
                            )
                        })
                    }
                    // The remaining request types are not implemented by this host.
                    _ => DebugResponse::Error(DebugError {
                        code: "unsupported".into(),
                        message: "the headless page does not handle this request".into(),
                    }),
                },
                ControlBridgeRequest::Diagnostics(DiagnosticsRequest::WebGpuInspect)
                    if !policy.bot_child =>
                {
                    match crate::webgpu::inspect_gpu(&mut document) {
                        Ok(report) => DebugResponse::WebGpu(report),
                        Err(error) => DebugResponse::Error(error),
                    }
                }
                ControlBridgeRequest::Diagnostics(DiagnosticsRequest::Capture(request))
                    if !policy.bot_child =>
                {
                    if !request.scale.is_finite() || !(0.25..=8.0).contains(&request.scale) {
                        DebugResponse::Error(DebugError {
                            code: "invalidArgument".into(),
                            message: "capture scale must be finite and between 0.25 and 8".into(),
                        })
                    } else {
                        match capture.capture(&mut document, request) {
                            Ok(captured) => DebugResponse::Captured(captured),
                            Err(error) => DebugResponse::Error(error),
                        }
                    }
                }
                ControlBridgeRequest::Diagnostics(DiagnosticsRequest::Snapshot(request))
                    if !policy.bot_child =>
                {
                    revision += 1;
                    let status_only = !request.include_dom;
                    match snapshot_document(&mut document, request, revision) {
                        Ok(mut snapshot) => {
                            if status_only {
                                // Read now, not at load: counters keep moving.
                                snapshot.dom =
                                    tls_status.borrow().as_ref().map(|(url, policy_error)| {
                                        crate::tls_status::inspection_state(url, *policy_error)
                                    });
                            }
                            DebugResponse::Snapshot(snapshot)
                        }
                        Err(error) => DebugResponse::Error(error),
                    }
                }
                ControlBridgeRequest::Diagnostics(DiagnosticsRequest::WindowComposition)
                    if !policy.bot_child =>
                {
                    // There is no window, so there is nothing composited over the
                    // page. Reporting the default is the true answer here, and it
                    // is what lets a spill check run headlessly at all.
                    DebugResponse::WindowComposition(WindowComposition::default())
                }
                ControlBridgeRequest::Diagnostics(DiagnosticsRequest::WebGpuCapture { target })
                    if !policy.bot_child =>
                {
                    match crate::webgpu::capture_gpu(&mut document, &target) {
                        Ok(captured) => DebugResponse::Captured(captured),
                        Err(error) => DebugResponse::Error(error),
                    }
                }
                ControlBridgeRequest::Diagnostics(DiagnosticsRequest::WebGpuBuffer {
                    target,
                    offset,
                    length,
                }) if !policy.bot_child => {
                    match crate::webgpu::capture_gpu_buffer(&mut document, &target, offset, length)
                    {
                        Ok(snapshot) => DebugResponse::WebGpuBuffer(snapshot),
                        Err(error) => DebugResponse::Error(error),
                    }
                }
                ControlBridgeRequest::Diagnostics(DiagnosticsRequest::Evaluate { script })
                    if !policy.bot_child =>
                {
                    painted = true;
                    match document.eval_json(&script) {
                        Ok(value) => DebugResponse::Evaluated(value),
                        Err(message) => DebugResponse::Error(DebugError {
                            code: "scriptError".into(),
                            message,
                        }),
                    }
                }
                ControlBridgeRequest::Diagnostics(DiagnosticsRequest::Metrics)
                    if !policy.bot_child =>
                {
                    DebugResponse::Metrics(RendererMetrics {
                        revisions: headless_revisions(render_revision, &document),
                        queue_depth: Some(document.pending_task_count()),
                        ..RendererMetrics::default()
                    })
                }
                ControlBridgeRequest::Diagnostics(DiagnosticsRequest::WaitForIdle)
                    if !policy.bot_child =>
                {
                    // Run queued jobs until the page has none left, bounded so a
                    // page that reschedules forever still gets an answer.
                    let deadline = std::time::Instant::now() + IDLE_WAIT_LIMIT;
                    while document.has_pending_jobs() && std::time::Instant::now() < deadline {
                        painted |= tick_document(&mut document, &animation_clock);
                    }
                    last_tick = std::time::Instant::now();
                    DebugResponse::Idle(headless_revisions(render_revision, &document))
                }
                ControlBridgeRequest::Diagnostics(_) => DebugResponse::Error(DebugError {
                    code: "unsupported".into(),
                    message: "the bot browser page serves no diagnostics".into(),
                }),
                ControlBridgeRequest::Targeted(_) => {
                    unreachable!("target envelopes are normalized before dispatch")
                }
            }
        };
        if policy.bot_child && navigation.take_refusal() {
            if was_action {
                response = DebugResponse::Error(policy_denial(
                    "the page requested navigation outside the browser policy",
                ));
            } else {
                trace("the page requested navigation outside the browser policy");
            }
        }
        if painted {
            commit_render(&render_events, &mut render_revision);
        }
        // A caller that stopped waiting leaves the `Once` behind unread; that
        // is not a reason to stop serving everyone else.
        drop(request_turn);
        if let Some(context) = target_context {
            response = response.with_context(context);
        }
        if !deferred_buffer {
            reply.fill(response);
        }
        // The page runs on a schedule, not only when the socket goes quiet.
        //
        // `recv_timeout` ticks the document when *no* request arrives, which
        // is exactly backwards for a harness: a check waiting for an outcome
        // inspects the tree continuously, the timeout never fires, and the
        // page it is waiting on never advances. The check then waits out its
        // whole deadline for something that completes moments after it gives
        // up. Measured on honey.id, whose sign-in completed three seconds
        // after a 150-second check declared it had not.
        //
        // Anything driven by a host callback rather than by a request lands
        // here: socket messages, timers, promise continuations.
        //
        // One turn per reply is not enough, and the arithmetic is the reason.
        // A semantic snapshot of a signed-in page costs upwards of 150ms, and
        // a driver polling for an outcome sends the next one the moment it has
        // the last. That bought the page six turns a second while the harness
        // watched, against roughly a hundred when it did not, so a login that
        // needed a few hundred turns of timers and promise continuations
        // completed only after the harness gave up and disconnected. Thirty
        // seconds of polling produced 179 turns; six seconds of quiet produced
        // about six hundred.
        //
        // So the page is owed the turns the request consumed. `last_tick`
        // advances by one interval per turn rather than being reset to now,
        // which is what makes this a catch-up rather than a single tick, and
        // the cap keeps one slow request from handing the page an unbounded
        // slice.
        //
        // After the reply, never before it. Ticking first resolves layout
        // underneath the request that is about to be answered, and an inspect
        // then reports a tree whose every box is 0x0.
        let catchup_started = std::time::Instant::now();
        let mut owed = 0_u32;
        while last_tick.elapsed() >= IDLE_TICK
            && owed < MAX_CATCHUP_TURNS
            && catchup_started.elapsed() < IDLE_TICK
        {
            match request_rx.try_recv() {
                Ok(pair) => {
                    queued_request = Some(pair);
                    break;
                }
                Err(page_control::TryRecvError::Empty) => {}
                Err(page_control::TryRecvError::Disconnected) => break,
            }
            if tick_document(&mut document, &animation_clock) {
                commit_render(&render_events, &mut render_revision);
            }
            last_tick += IDLE_TICK;
            owed += 1;
        }
        // Forgive debt at either cap. A single slow poll is also bounded by
        // the supervisor, independently of whether this loop returns.
        if owed >= MAX_CATCHUP_TURNS || catchup_started.elapsed() >= IDLE_TICK {
            last_tick = std::time::Instant::now();
        }

        /*
         * A link the last action followed.
         *
         * After the reply, not before it, so the action is acknowledged with
         * the timing it actually had rather than with a page load added to it.
         * A caller inspects again before it asserts anything, and by then the
         * new document is standing.
         *
         * A load that fails leaves the old page up and says so. Serving an
         * error document instead would make every check after it fail against
         * a page that is not the one under test, and the reason would be four
         * checks back in the log.
         */
        if queued_request.is_none()
            && let Some(destination) = navigation.take()
        {
            trace(&format!(
                "following a link to {}",
                display_url(&destination, policy.bot_child)
            ));
            match follow_navigation(
                destination,
                &mut document,
                &mut origin,
                &policy,
                &load,
                (&render_events, &mut render_revision),
                false,
            ) {
                Ok(()) => {}
                Err(error) => trace(&format!("the navigation failed, staying put: {error}")),
            }
        }
    }

    trace("inspection host finished");
    let _flush = supervisor::operation(supervisor::Operation::Storage);
    nagoya::block_on(net_provider.cookie_store().flush())
        .map_err(|error| format!("could not flush browser cookies: {error}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        ActionContext, BOT_CHILD_MAX_INSPECT_BYTES, BOT_CHILD_MAX_WALL_TIME, HeadlessPolicy,
        MAX_PROXY_BYTES, Target, Url, bound_inspection_snapshot, classify, condense_response,
        domain_is_allowed, parse_allowed_domains, parse_wall_time, proxy_connect_url,
        reserve_proxy_bytes, sensitive_surface, submit_needs_permission, target_from,
    };
    use blitz_control_protocol::{AgentSnapshot, DebugResponse, SemanticNode, condense};

    fn fixture_root(label: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "chuzz-serve-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock is after the epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).expect("create fixture root");
        root
    }

    /// A built page is a directory, and `ps-qa sweep-components` hands one over
    /// per component. It gets an origin rather than a `file://` path to its
    /// index, because a built site names its bundle absolutely and its router
    /// owns paths that were never built as files.
    #[test]
    fn a_directory_is_served_as_a_site() {
        let root = fixture_root("dir");
        std::fs::write(root.join("index.html"), "<html></html>").expect("write index");
        let target = classify(root.to_str().expect("utf-8 fixture path")).expect("classify");
        assert!(
            matches!(target, Target::Directory(_)),
            "a built page needs an origin, got {target:?}"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// A directory that is not a built page is refused rather than served
    /// empty, because an empty tree over a working socket reads as a component
    /// that renders nothing.
    #[test]
    fn a_directory_with_no_index_is_refused() {
        let root = fixture_root("empty");
        let error =
            classify(root.to_str().expect("utf-8 fixture path")).expect_err("no index to serve");
        assert!(
            error.contains("no index.html"),
            "unhelpful refusal: {error}"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// The ordering that matters: a relative path is checked against the
    /// filesystem before it is offered to the address-bar rule. `dist` is a
    /// directory here and a bare hostname to `request_from_input`, and guessing
    /// wrong fetches `https://dist/` and reports the result as the page under
    /// test.
    #[test]
    fn a_path_that_exists_is_never_read_as_a_hostname() {
        let root = fixture_root("host");
        let page = root.join("example.com");
        std::fs::create_dir(&page).expect("create fixture page");
        std::fs::write(page.join("index.html"), "<html></html>").expect("write index");
        let target = classify(page.to_str().expect("utf-8 fixture path")).expect("classify");
        assert!(
            matches!(target, Target::Directory(_)),
            "a path named like a host was fetched over the network: {target:?}"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// A built page is served from its own directory rather than opened as a
    /// file. Its markup names `/static/...` absolutely, which a `file://` base
    /// resolves to the filesystem root, so the bundle is never fetched and the
    /// page renders as an empty mount point.
    #[test]
    fn a_built_page_is_served_from_its_directory() {
        let root = fixture_root("file");
        let page = root.join("page.html");
        std::fs::write(&page, "<html></html>").expect("write page");
        let target = classify(page.to_str().expect("utf-8 fixture path")).expect("classify");
        let Target::File {
            root: served,
            page: name,
        } = target
        else {
            panic!("a built page should be served from its directory, got {target:?}");
        };
        assert_eq!(name, "page.html");
        assert!(served.ends_with(root.file_name().expect("fixture directory name")));
        let _ = std::fs::remove_dir_all(root);
    }

    /// Anything else named directly is still taken as given. The engine's own
    /// fixtures include files that are not documents.
    #[test]
    fn a_file_that_is_not_a_document_is_taken_as_a_page() {
        let root = fixture_root("plain-file");
        let page = root.join("data.json");
        std::fs::write(&page, "{}").expect("write file");
        let target = classify(page.to_str().expect("utf-8 fixture path")).expect("classify");
        let Target::Page(url) = target else {
            panic!("a plain file should be a page, got {target:?}");
        };
        assert_eq!(url.scheme(), "file");
        let _ = std::fs::remove_dir_all(root);
    }

    /// A remote page is still a page. The fleet is deployed, and pointing the
    /// harness at what is actually serving is the whole point of sharing the
    /// browser's loader.
    #[test]
    fn a_url_is_taken_as_written() {
        let target = classify("https://support.cafe/").expect("resolve url");
        assert_eq!(
            target,
            Target::Page(Url::parse("https://support.cafe/").unwrap())
        );
    }

    #[test]
    fn a_target_that_is_neither_is_refused() {
        let error = classify("not a page").expect_err("prose is not a page");
        assert!(
            error.contains("neither a path that exists nor a URL"),
            "unhelpful refusal: {error}"
        );
    }

    /// `ps-qa` runs the host binary with no arguments and `QA_INSPECT_PAGE`
    /// set, so a host that only reads argv is not a host it can launch.
    #[test]
    fn an_argument_wins_over_the_environment() {
        let args = vec!["chuzz-headless".to_owned(), "example.com".to_owned()];
        assert_eq!(target_from(&args).expect("argument"), "example.com");
    }

    /// A flag is not the page. The argument scan skips anything beginning with
    /// `--`, or `--serve` itself would be loaded as an address.
    #[test]
    fn a_flag_is_not_mistaken_for_the_page() {
        let args = vec![
            "chuzz-headless".to_owned(),
            "--serve".to_owned(),
            "example.com".to_owned(),
        ];
        assert_eq!(target_from(&args).expect("argument"), "example.com");
    }

    #[test]
    fn bot_mode_rejects_local_schemes_addresses_and_credentials() {
        let policy = HeadlessPolicy::bot_child_for_tests(&[], false);
        for target in [
            "http://example.com/",
            "file:///etc/passwd",
            "https://user:secret@example.com/",
            "https://127.0.0.1/",
            "https://10.0.0.8/",
            "https://169.254.169.254/",
            "https://100.64.0.1/",
            "https://[::1]/",
            "https://[fd00::1]/",
        ] {
            let url = Url::parse(target).expect("parse target URL");
            assert!(policy.validate_url(&url).is_err(), "accepted {target}");
        }
    }

    #[test]
    fn bot_mode_accepts_public_literal_addresses() {
        let policy = HeadlessPolicy::bot_child_for_tests(&[], false);
        let url = Url::parse("https://8.8.8.8/").expect("parse public URL");
        assert!(policy.validate_url(&url).is_ok());
    }

    #[test]
    fn bot_child_condense_is_denied_before_inspection() {
        let policy = HeadlessPolicy::bot_child_for_tests(&[], false);
        let response = condense_response(
            &policy,
            condense::Level::Content,
            condense::CondenseOptions::default(),
            || panic!("bot-child condensation must not inspect the page"),
        );
        assert!(matches!(
            response,
            DebugResponse::Error(error) if error.code == "policyDenied"
        ));
    }

    #[test]
    fn host_allowlist_matches_only_exact_domains_and_subdomains() {
        let allowed =
            parse_allowed_domains(Some("Example.com, example.org")).expect("parse allowed domains");
        assert!(domain_is_allowed("example.com", &allowed));
        assert!(domain_is_allowed("www.example.com", &allowed));
        assert!(!domain_is_allowed("notexample.com", &allowed));
        assert!(!domain_is_allowed("example.com.attacker", &allowed));
    }

    #[test]
    fn egress_filter_accepts_connect_and_refuses_plain_http_requests() {
        let tunnel = proxy_connect_url("CONNECT example.com:443 HTTP/1.1\r\n\r\n")
            .expect("HTTPS tunnel target");
        assert_eq!(tunnel.scheme(), "https");
        assert_eq!(tunnel.host_str(), Some("example.com"));
        assert!(proxy_connect_url("GET http://example.com/ HTTP/1.1\r\n\r\n").is_none());
    }

    #[test]
    fn form_submission_requires_explicit_caller_permission() {
        let context = ActionContext {
            may_submit: true,
            ..ActionContext::default()
        };
        let denied = HeadlessPolicy::bot_child_for_tests(&[], false);
        let allowed = HeadlessPolicy::bot_child_for_tests(&[], true);
        assert!(submit_needs_permission(&denied, &context));
        assert!(!submit_needs_permission(&allowed, &context));
    }

    #[test]
    fn sensitive_labels_are_blocked_without_rejecting_search_controls() {
        assert!(sensitive_surface("Sign in"));
        assert!(sensitive_surface("Pay now"));
        assert!(sensitive_surface("Book flight"));
        assert!(sensitive_surface("Passenger email"));
        assert!(!sensitive_surface("Search available flights"));
        assert!(!sensitive_surface("Facebook"));
    }

    #[test]
    fn proxy_byte_budget_stops_at_its_fixed_ceiling() {
        let counter = std::sync::atomic::AtomicUsize::new(MAX_PROXY_BYTES - 2);
        assert_eq!(reserve_proxy_bytes(&counter, 8), 2);
        assert_eq!(
            counter.load(std::sync::atomic::Ordering::Acquire),
            MAX_PROXY_BYTES
        );
        assert_eq!(reserve_proxy_bytes(&counter, 1), 0);
    }

    #[test]
    fn semantic_inspection_output_has_a_fixed_byte_ceiling() {
        let mut snapshot = AgentSnapshot {
            nodes: vec![SemanticNode {
                dom_id: Some("d".repeat(4000)),
                id: 1,
                parent: None,
                role: "r".repeat(4000),
                name: "n".repeat(40_000),
                value: Some("v".repeat(40_000)),
                enabled: true,
                focusable: true,
                viewport_fixed: false,
                visible: true,
                selected: false,
                bounds: None,
                slot: Some("s".repeat(4000)),
                attributes: Default::default(),
            }],
            ..AgentSnapshot::default()
        };
        bound_inspection_snapshot(&mut snapshot);
        let encoded = serde_json::to_vec(&snapshot).expect("encode bounded inspection");
        assert!(encoded.len() <= BOT_CHILD_MAX_INSPECT_BYTES);
    }

    #[test]
    fn wall_timeout_can_only_be_shortened() {
        assert_eq!(
            parse_wall_time(None).expect("default timeout"),
            BOT_CHILD_MAX_WALL_TIME
        );
        assert_eq!(
            parse_wall_time(Some("1500")).expect("short timeout"),
            std::time::Duration::from_millis(1500)
        );
        assert!(parse_wall_time(Some("20001")).is_err());
        assert!(parse_wall_time(Some("0")).is_err());
    }
}
