//! The shared parts of turning a URL into a renderable document.
//!
//! Tab loading itself lives in `browser.rs`, next to the state it mutates.
//! What stays here is the shared network provider and the headless capture
//! path, which loads a page without a window to attach it to.

// The capture path is off by default: it pulls in the CPU rasteriser the
// windowed browser never needs.
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

#[cfg(feature = "capture")]
use blitz_dom::DocumentConfig;
#[cfg(feature = "capture")]
use blitz_html::HtmlProvider;
#[cfg(feature = "capture")]
use blitz_traits::net::Request;

#[cfg(feature = "capture")]
use crate::decode::decode_body;

use blitz_traits::net::{Credentials, Initiator, ResourcePolicy, Url};

/// Network provider used by every document and script in Chuzz.
///
/// The wrapped provider owns the HTTP client, cache and browser identity. This
/// layer attaches the shared profile cookie jar and the browser's default
/// content language to every request, including subresources created inside
/// the DOM engine. Explicit caller headers still win.
#[derive(Clone)]
pub struct NetProvider {
    inner: Arc<blitz_net::Provider>,
    policy: Arc<ResourcePolicy>,
    initiator: Initiator,
    cookies: Arc<crate::cookie_store::BrowserCookieStore>,
    #[cfg(feature = "gui")]
    cache: Option<Arc<blitz_net::PrivateCache>>,
    waker: Arc<dyn blitz_traits::net::NetWaker>,
    blocked_requests: Arc<Mutex<VecDeque<String>>>,
}

fn document_fetch_error(error: blitz_net::ProviderError) -> blitz_traits::platform::FetchError {
    match error {
        blitz_net::ProviderError::TlsPolicy(reason) => {
            blitz_traits::platform::FetchError::TlsPolicy(reason)
        }
        error => blitz_traits::platform::FetchError::Network(format!("{error:?}")),
    }
}

/// Debugging aid for real sites: with `CHUZZ_DEBUG_DUMP=<dir>`, every script
/// response is written there as `<n>-<host><path>.js`, the exact bytes the page
/// ran, so an error's `line:column` on an obfuscated or per-request script (a
/// challenge page's) can be read back. Nothing is written unless the variable
/// is set.
fn dump_script_for_debugging(response: &blitz_traits::platform::FetchResponse) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let Some(dir) = std::env::var_os("CHUZZ_DEBUG_DUMP") else {
        return;
    };
    let script = response
        .headers
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.contains("javascript") || value.contains("ecmascript"))
        || response.url.path().ends_with(".js")
        || response.url.path().ends_with(".mjs");
    if !script {
        return;
    }
    let name: String = format!(
        "{}{}",
        response.url.host_str().unwrap_or_default(),
        response.url.path()
    )
    .chars()
    .map(|c| {
        if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
            c
        } else {
            '_'
        }
    })
    .take(120)
    .collect();
    let path = std::path::Path::new(&dir).join(format!(
        "{}-{name}.js",
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let written =
        std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&path, &response.body));
    match written {
        Ok(()) => eprintln!("chuzz: dumped {} to {}", response.url, path.display()),
        Err(error) => eprintln!("chuzz: CHUZZ_DEBUG_DUMP {}: {error}", path.display()),
    }
}

impl NetProvider {
    pub fn new(waker: Option<Arc<dyn blitz_traits::net::NetWaker>>) -> Self {
        Self::with_user_agent(waker, &crate::identity::user_agent_from_env())
    }

    pub fn with_user_agent(
        waker: Option<Arc<dyn blitz_traits::net::NetWaker>>,
        user_agent: &str,
    ) -> Self {
        Self::with_user_agent_and_cookies(
            waker,
            user_agent,
            Arc::new(crate::cookie_store::BrowserCookieStore::default()),
        )
    }

    pub fn with_user_agent_and_cookies(
        waker: Option<Arc<dyn blitz_traits::net::NetWaker>>,
        user_agent: &str,
        cookies: Arc<crate::cookie_store::BrowserCookieStore>,
    ) -> Self {
        Self::with_user_agent_cookies_and_cache(waker, user_agent, cookies, None)
    }

    pub(crate) fn with_user_agent_cookies_and_cache(
        waker: Option<Arc<dyn blitz_traits::net::NetWaker>>,
        user_agent: &str,
        cookies: Arc<crate::cookie_store::BrowserCookieStore>,
        cache: Option<Arc<blitz_net::PrivateCache>>,
    ) -> Self {
        let waker = waker.unwrap_or_else(|| Arc::new(|_| {}));
        let application_policy = crate::resource_parameters::from_environment();
        let mut inner = blitz_net::Provider::with_user_agent_and_cookie_provider(
            None,
            user_agent,
            Arc::clone(&cookies),
        );
        if let Some(cache) = &cache {
            inner = inner.with_cache(Arc::clone(cache));
        }
        Self {
            inner: Arc::new(inner),
            policy: Arc::new(ResourcePolicy::default()),
            initiator: Initiator::Unknown,
            cookies,
            #[cfg(feature = "gui")]
            cache,
            waker,
            blocked_requests: Arc::new(Mutex::new(VecDeque::new())),
        }
        .with_resource_policy(application_policy)
    }

    /// Application-owned policy, supplied before sharing the root provider.
    pub fn with_resource_policy(mut self, policy: Arc<ResourcePolicy>) -> Self {
        self.policy = policy;
        self
    }

    pub fn browser_initiator(&self, url: Url) -> Initiator {
        self.policy.browser(url)
    }

    pub fn initiator(&self) -> Initiator {
        self.initiator.clone()
    }

    pub fn for_document(&self, url: Url, navigation: &Initiator) -> Self {
        let mut provider = self.clone();
        provider.initiator = self.policy.document(url, navigation);
        provider
    }

    fn bind(&self, mut request: blitz_traits::net::Request) -> blitz_traits::net::Request {
        if !matches!(self.initiator, Initiator::Unknown) {
            request.initiator = self.initiator.clone();
        }
        Self::with_default_language(request)
    }

    pub async fn fetch_platform_async(
        &self,
        mut request: blitz_traits::platform::FetchRequest,
    ) -> Result<blitz_traits::platform::FetchResponse, blitz_traits::platform::FetchError> {
        if !matches!(self.initiator, Initiator::Unknown) {
            request.initiator = self.initiator.clone();
        }
        crate::languages::insert_accept_language(&mut request.headers);
        self.inner.fetch_platform_async(request).await
    }

    pub fn user_agent(&self) -> &str {
        self.inner.user_agent()
    }

    pub fn cookie_store(&self) -> Arc<crate::cookie_store::BrowserCookieStore> {
        Arc::clone(&self.cookies)
    }

    #[cfg(feature = "gui")]
    pub(crate) fn http_cache(&self) -> Option<Arc<blitz_net::PrivateCache>> {
        self.cache.clone()
    }

    pub(crate) fn note_blocked_request(&self, url: &Url) {
        let message = crate::telemetry::diagnostic(url);
        self.blocked_requests
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .push_back(message.clone());
        eprintln!("chuzz: net: {message}");
    }

    /// Drain blocked-request diagnostics accumulated by this provider.
    ///
    /// Diagnostics consumers can call this after page loading to retrieve each
    /// blocked request once; subsequent calls return only newer entries.
    pub fn take_blocked_requests(&self) -> Vec<String> {
        self.blocked_requests
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .drain(..)
            .collect()
    }

    fn with_default_language(
        mut request: blitz_traits::net::Request,
    ) -> blitz_traits::net::Request {
        if request.credentials == Credentials::Omit {
            for name in ["cookie", "cookie2", "authorization", "proxy-authorization"] {
                request.headers.remove(name);
            }
        }
        crate::languages::insert_accept_language(&mut request.headers);
        request
    }

    pub async fn fetch_response_async(
        &self,
        request: blitz_traits::net::Request,
    ) -> Result<blitz_traits::platform::FetchResponse, blitz_net::ProviderError> {
        let response = self.inner.fetch_response_async(self.bind(request)).await?;
        dump_script_for_debugging(&response);
        Ok(response)
    }

    /// A top-level document. Every HTTP status is a page, as in any browser: the body of a 403 or 503
    /// is often an error or challenge page the reader has to see and its scripts have to run. Only a
    /// network failure is an error. A request with a body (form POST) keeps the older path.
    pub async fn fetch_document_async(
        &self,
        mut request: blitz_traits::net::Request,
    ) -> Result<(String, blitz_traits::net::Bytes), blitz_traits::platform::FetchError> {
        request = self.bind(request);
        let initiator = request.initiator.document_url().cloned();
        crate::identity::stamp_request_headers(
            &mut request.headers,
            self.user_agent(),
            &request.url.clone(),
            crate::identity::RequestKind::Navigate {
                initiator: initiator.as_ref(),
            },
        );
        let requested_url = request.url.clone();
        let response = if !matches!(request.body, blitz_traits::net::Body::Empty)
            || !matches!(request.url.scheme(), "http" | "https")
        {
            // This includes file URLs and form submissions. Keep their
            // response headers until document-type detection has run.
            self.fetch_response_async(request)
                .await
                .map_err(document_fetch_error)?
        } else {
            let request = Self::with_default_language(request);
            let mut wire =
                blitz_traits::platform::FetchRequest::get(request.url).method(request.method);
            wire.headers = request.headers;
            wire.signal = request.signal;
            wire.initiator = request.initiator;
            wire.credentials = request.credentials;
            self.fetch_platform_async(wire).await?
        };
        Ok(Self::document_response(&requested_url, response))
    }

    fn document_response(
        requested_url: &url::Url,
        response: blitz_traits::platform::FetchResponse,
    ) -> (String, blitz_traits::net::Bytes) {
        let resolved_url = response.url.to_string();
        let body = response.body;
        #[cfg(feature = "pdf")]
        let body = {
            let content_type = response
                .headers
                .get("content-type")
                .and_then(|value| value.to_str().ok());
            if crate::pdf::is_pdf(&resolved_url, content_type, &body)
                || crate::pdf::is_pdf(requested_url.as_str(), None, &body)
            {
                blitz_traits::net::Bytes::from(
                    crate::pdf::viewer_html(&resolved_url, &body).into_bytes(),
                )
            } else {
                body
            }
        };
        #[cfg(not(feature = "pdf"))]
        let _ = requested_url;
        let body = if crate::image_document::is_image(
            response
                .headers
                .get("content-type")
                .and_then(|value| value.to_str().ok()),
            &body,
        ) {
            blitz_traits::net::Bytes::from(
                crate::image_document::viewer_html(&resolved_url).into_bytes(),
            )
        } else {
            body
        };
        // The generated HTML is parsed at the navigation's resolved URL,
        // preserving history, location and the document's base URL.
        (resolved_url, body)
    }

    pub async fn fetch_async(
        &self,
        request: blitz_traits::net::Request,
    ) -> Result<(String, blitz_traits::net::Bytes), blitz_net::ProviderError> {
        let response = self.fetch_response_async(request).await?;
        Ok((response.url.to_string(), response.body))
    }
}

impl blitz_traits::net::NetProvider for NetProvider {
    fn fetch(
        &self,
        doc_id: usize,
        mut request: blitz_traits::net::Request,
        handler: Box<dyn blitz_traits::net::NetHandler>,
    ) {
        request = self.bind(request);
        let initiator = request.initiator.document_url().cloned();
        let destination = request.destination.as_str();
        crate::identity::stamp_request_headers(
            &mut request.headers,
            self.user_agent(),
            &request.url.clone(),
            crate::identity::RequestKind::Subresource {
                destination,
                initiator: initiator.as_ref(),
            },
        );
        let signal = request.signal.clone();
        let provider = self.clone();
        if crate::telemetry::blocked_url(&request.url) {
            provider.note_blocked_request(&request.url);
            handler.bytes(request.url.to_string(), blitz_traits::net::Bytes::new());
            provider.waker.wake(doc_id);
            return;
        }
        let request_url = request.url.to_string();
        let _detached = nagoya::spawn(async move {
            let result = provider.fetch_response_async(request).await;
            if signal.as_ref().is_some_and(|signal| signal.aborted()) {
                return;
            }
            match result {
                Ok(response) => handler.bytes(response.url.to_string(), response.body),
                Err(blitz_net::ProviderError::Abort) => return,
                Err(error) => {
                    eprintln!("chuzz: resource {request_url}: {error}");
                    // Settle the engine's pending resource even when denied.
                    handler.bytes(request_url, blitz_traits::net::Bytes::new());
                }
            }
            provider.waker.wake(doc_id);
        });
    }

    fn resource_initiator(&self) -> Initiator {
        self.initiator()
    }

    fn for_child_document(&self, url: Url) -> Option<Arc<dyn blitz_traits::net::NetProvider>> {
        let mut provider = self.clone();
        provider.initiator = self.initiator.child(url);
        Some(Arc::new(provider))
    }

    fn is_noop(&self) -> bool {
        false
    }
}

#[cfg(all(feature = "capture", feature = "javascript"))]
const CAPTURE_NETWORK_DEADLINE: std::time::Duration = std::time::Duration::from_secs(15);

/// Load a page outside the browser, for headless capture.
///
/// Shares the fetch, decompression, script execution and native page hosts with the
/// browser's own loading, so a capture shows what a tab would show. It is a
/// separate path rather than the same one because `browser.rs` loads into a
/// live window: it attaches the result to a `<web-view>` and emits events, and
/// there is no window here to attach to. **A capture therefore proves the
/// engine renders and does not prove the shell's mount rendezvous.**
///
/// `prelude` is evaluated after native host attachment and before the page's own scripts, for
/// state the caller is carrying into this document. That ordering is the whole
/// of its usefulness: an application reads its stored settings while it boots,
/// so seeding storage a moment later is the same as not seeding it. Empty for a
/// capture, which loads one page and has nothing to carry.
#[cfg(feature = "capture")]
pub async fn load_for_capture(
    mut request: Request,
    net_provider: Arc<NetProvider>,
    prelude: &str,
    viewport: Option<blitz_traits::shell::Viewport>,
) -> Result<CapturedDocument, Box<dyn std::error::Error>> {
    use blitz_dom::Document as _;

    if matches!(request.initiator, Initiator::Browser(_)) {
        request.initiator = net_provider.browser_initiator(request.url.clone());
    }
    let navigation = request.initiator.clone();

    // `view-source:` is the browser's, and a capture that could not take it was
    // the one address a tab could show and a PNG could not. The scheme is not a
    // fetchable one, so this has to come before the net provider sees it: the
    // inner URL is what gets fetched, and the bytes are escaped rather than
    // parsed.
    //
    // `browser::source_html` rather than a second copy of the escaping. What
    // the capture writes has to be byte-for-byte what the tab shows, or the PNG
    // stops being evidence about the browser and becomes evidence about this
    // function.
    //
    // Nothing below applies to a source document: it has no scripts to run and
    // no images to wait for, so it returns here rather than falling through to
    // the script pump.
    if request.url.scheme() == "view-source" {
        let inner = request.url.path().to_owned();
        let url = Url::parse(&inner).map_err(|error| format!("{inner} is not a URL: {error}"))?;
        let navigation = if matches!(navigation, Initiator::Browser(_)) {
            net_provider.browser_initiator(url.clone())
        } else {
            navigation.clone()
        };
        let (_, bytes) = net_provider
            .fetch_async(Request::get(url).initiator(navigation.clone()))
            .await
            .map_err(document_fetch_error)?;
        let html = crate::internal_pages::source_html(&decode_body(&bytes));
        return Ok(CapturedDocument::Html(Box::new(
            blitz_html::HtmlDocument::from_html(
                &html,
                DocumentConfig {
                    html_parser_provider: Some(Arc::new(HtmlProvider)),
                    ..Default::default()
                },
            )
            .into_inner(),
        )));
    }

    // The navigation signal also owns subresource work started by the page.
    // In particular, the page runtime's ScriptFetcher must stop when the
    // capture request is cancelled, rather than receiving a fresh signal that
    // can outlive the navigation which created it.
    let page_signal = request.signal.clone().unwrap_or_default();
    let (resolved_url, bytes) = net_provider.fetch_document_async(request).await?;
    let html = decode_body(&bytes);

    let resolved_url = Url::parse(&resolved_url)?;
    let net_provider = Arc::new(net_provider.for_document(resolved_url.clone(), &navigation));
    let mut config = DocumentConfig {
        base_url: Some(resolved_url.to_string()),
        net_provider: Some(Arc::clone(&net_provider) as _),
        html_parser_provider: Some(Arc::new(HtmlProvider)),
        abort_signal: Some(page_signal.clone()),
        // The host's viewport, before any script runs: a page that reads
        // innerWidth or matchMedia during load must see the size it renders at.
        viewport,
        ..Default::default()
    };
    crate::install_headless_fonts(&mut config);

    #[cfg(feature = "javascript")]
    {
        let host_config = crate::script_host::HostInstallContext {
            net: Arc::clone(&net_provider),
            base: Some(resolved_url.clone()),
            deadline: CAPTURE_NETWORK_DEADLINE,
            wake: crate::net_bridge::PageWake::default(),
            lifetime: blitz_dom::release::Teardown::new(),
            signal: page_signal,
            session_backing: None,
        };
        let mut document = crate::page_document::PageDocument::from_html(
            Arc::<str>::from(html),
            config,
            host_config,
        )
        .map_err(std::io::Error::other)?;
        // A headless or captured page is the one foreground page, as in
        // headless Chrome: document.hasFocus() is true before its scripts run,
        // so input handlers that ignore keys while unfocused still take them.
        document.inner_mut().set_window_focused(true);
        if !prelude.is_empty() {
            document.eval(prelude).map_err(std::io::Error::other)?;
        }
        document.execute_scripts().map_err(std::io::Error::other)?;
        // Pump the script runtime until the page has built its DOM, then keep
        // pumping for a few more passes.
        //
        // Breaking out the moment `body > * > *` matches is wrong: parsing the
        // HTML already issued the <img> fetches, and those responses are
        // delivered on the document's channel. Returning early drops the
        // document that owns the receiver while requests are still in flight,
        // so `respond` sends into a closed channel and the decoded image is
        // discarded, which looks exactly like an image that failed to load.
        //
        // Note the selector also matches static markup that simply has a
        // wrapper element, so for most pages this loop used to exit on the
        // first pass regardless of whether anything was pending.
        let mut passes_since_built = 0;
        for _ in 0..24 {
            nagoya::sleep(std::time::Duration::from_millis(25)).await;
            document.poll(None);
            if document.has_pending_jobs() {
                // Publish a progressing document to its host. The control
                // loop resumes this checkpoint instead of waiting for the
                // entire async initialization chain during loading.
                break;
            }
            let built = document
                .inner()
                .query_selector("body > * > *")
                .ok()
                .flatten()
                .is_some();
            if built {
                passes_since_built += 1;
                if passes_since_built >= 8 {
                    break;
                }
            }
        }
        Ok(CapturedDocument::Script(Box::new(document)))
    }

    #[cfg(not(feature = "javascript"))]
    Ok(CapturedDocument::Html(
        HtmlDocument::from_html(&html, config).into_inner(),
    ))
}

/// A loaded page held for capture. Which variant it is depends on whether the
/// build runs scripts; both expose the same document underneath.
#[cfg(feature = "capture")]
pub enum CapturedDocument {
    // Boxed: a PageDocument is far larger than a bare one, and the enum
    // would otherwise cost the bigger variant on every use.
    #[cfg(feature = "javascript")]
    Script(Box<crate::page_document::PageDocument>),
    #[allow(dead_code)]
    Html(Box<blitz_dom::BaseDocument>),
}

#[cfg(feature = "capture")]
impl CapturedDocument {
    pub fn with_document<R>(
        &mut self,
        callback: impl FnOnce(&mut blitz_dom::BaseDocument) -> R,
    ) -> R {
        #[cfg(feature = "javascript")]
        use blitz_dom::Document as _;
        match self {
            #[cfg(feature = "javascript")]
            Self::Script(document) => callback(&mut document.inner_mut()),
            Self::Html(document) => callback(document),
        }
    }
}

#[cfg(all(feature = "capture", feature = "javascript"))]
impl CapturedDocument {
    /// The script document underneath, for a caller that has to drive the
    /// script runtime rather than only paint what it produced.
    ///
    /// `with_document` hands out the `BaseDocument`, which is enough to lay out
    /// and paint and not enough to serve inspection: answering a click means
    /// dispatching the event and then pumping the microtask queue the handler
    /// filled, and only the `PageDocument` owns that queue. `None` for a page
    /// that was parsed rather than scripted, which has no queue to pump.
    pub fn into_page_document(self) -> Option<Box<crate::page_document::PageDocument>> {
        match self {
            Self::Script(document) => Some(document),
            Self::Html(_) => None,
        }
    }
}

#[cfg(all(test, feature = "javascript"))]
mod tests {
    use super::NetProvider;
    use crate::page_document::PageDocument;
    use blitz_dom::DocumentConfig;
    use blitz_traits::net::Url;
    use std::sync::Arc;

    fn native_page_document(
        html: &str,
        config: DocumentConfig,
        provider: NetProvider,
    ) -> PageDocument {
        let base = config
            .base_url
            .as_deref()
            .and_then(|url| Url::parse(url).ok());
        let host_config = crate::script_host::HostInstallContext {
            net: Arc::new(provider),
            base,
            deadline: std::time::Duration::from_secs(5),
            wake: crate::net_bridge::PageWake::default(),
            lifetime: blitz_dom::release::Teardown::new(),
            signal: blitz_traits::net::AbortSignal::default(),
            session_backing: None,
        };
        PageDocument::from_html(Arc::from(html), config, host_config)
            .expect("a registered page runtime must construct the native guest")
    }

    fn page() -> PageDocument {
        native_page_document(
            "<html><body></body></html>",
            DocumentConfig {
                base_url: Some("https://example.test/".to_owned()),
                ..Default::default()
            },
            NetProvider::with_user_agent(None, "Browser/123"),
        )
    }

    fn value(document: &mut PageDocument, script: &str) -> serde_json::Value {
        document
            .eval_json(script)
            .unwrap_or_else(|error| panic!("evaluating `{script}` failed: {error:?}"))
    }

    /// Run timers until a probe answers, or give up.
    ///
    /// `setTimeout` callbacks run when the native page runtime is polled.
    fn pump_for(document: &mut PageDocument, probe: &str) -> serde_json::Value {
        use blitz_dom::Document as _;
        for _ in 0..100 {
            document.poll(None);
            let seen = value(document, probe);
            if !seen.is_null() {
                return seen;
            }
            nagoya::block_on(nagoya::sleep(std::time::Duration::from_millis(5)));
        }
        serde_json::Value::Null
    }

    /// The whole string parses and every global it promises is installed.
    ///
    /// One bad token anywhere silently costs all of them, so this asserts the
    /// list rather than each name where it is tested.
    #[test]
    fn native_runtime_provides_the_required_globals() {
        let mut document = page();
        for name in [
            "localStorage",
            "sessionStorage",
            "Blob",
            "URLSearchParams",
            "MutationObserver",
            "IntersectionObserver",
            "requestIdleCallback",
            "matchMedia",
            "unescape",
            "escape",
            "TextEncoder",
            "TextDecoder",
            "AbortController",
            "AbortSignal",
            "ResizeObserver",
            "NodeFilter",
            "Image",
            "Path2D",
            "ShadowRoot",
            "DOMException",
            "atob",
            "btoa",
            "top",
            "scrollX",
        ] {
            assert_ne!(
                value(&mut document, &format!("typeof globalThis.{name}")),
                serde_json::json!("undefined"),
                "the native runtime should define {name}"
            );
        }
    }

    /// Tree traversal callers receive the standard filter results and node
    /// visibility masks, including the unsigned all-nodes mask.
    #[test]
    fn node_filter_exposes_standard_constants() {
        let mut document = page();
        assert_eq!(
            value(
                &mut document,
                "[NodeFilter.FILTER_ACCEPT === 1, NodeFilter.FILTER_REJECT === 2, \
                  NodeFilter.FILTER_SKIP === 3, NodeFilter.SHOW_ALL === 0xFFFFFFFF, \
                  NodeFilter.SHOW_ELEMENT === 0x1, NodeFilter.SHOW_ATTRIBUTE === 0x2, \
                  NodeFilter.SHOW_TEXT === 0x4, NodeFilter.SHOW_CDATA_SECTION === 0x8, \
                  NodeFilter.SHOW_ENTITY_REFERENCE === 0x10, NodeFilter.SHOW_ENTITY === 0x20, \
                  NodeFilter.SHOW_PROCESSING_INSTRUCTION === 0x40, \
                  NodeFilter.SHOW_COMMENT === 0x80, NodeFilter.SHOW_DOCUMENT === 0x100, \
                  NodeFilter.SHOW_DOCUMENT_TYPE === 0x200, \
                  NodeFilter.SHOW_DOCUMENT_FRAGMENT === 0x400, \
                  NodeFilter.SHOW_NOTATION === 0x800, \
                  Object.isFrozen(NodeFilter)]"
            ),
            serde_json::Value::Array(vec![serde_json::Value::Bool(true); 17])
        );
    }

    /// Compatibility loaders use prototype inspection, the pre-selector DOM
    /// collection APIs and the older event factory before mounting an app.
    #[test]
    fn legacy_dom_collections_and_event_factory_are_callable() {
        let mut document = native_page_document(
            "<html><body><section class='card chosen'><i></i></section><section class='card'></section></body></html>",
            blitz_dom::DocumentConfig::default(),
            NetProvider::with_user_agent(None, "Browser/123"),
        );
        assert_eq!(
            value(
                &mut document,
                "[document.getElementsByClassName('card').length, \
                  document.getElementsByClassName('card chosen').length, \
                  document.getElementsByClassName('   ').length, \
                  document.getElementsByTagName('section').length, \
                  document.querySelector('body').getElementsByTagName('i').length]"
            ),
            serde_json::json!([2, 1, 0, 2, 1])
        );
        assert_eq!(
            value(
                &mut document,
                "(function () { var event = document.createEvent('CustomEvent'); \
                  event.initCustomEvent('ready', true, true, 7); \
                  return [event.type, event.bubbles, event.cancelable, event.detail]; })()"
            ),
            serde_json::json!(["ready", true, true, 7])
        );
        assert_eq!(
            value(
                &mut document,
                "[HTMLElement.prototype.isPrototypeOf(document.documentElement), \
                  Element.prototype.isPrototypeOf(document.documentElement), \
                  Document.prototype.isPrototypeOf(document), \
                  typeof customElements.whenDefined]"
            ),
            serde_json::json!([true, true, true, "function"])
        );
    }

    /// Media feature detection can read the standard readiness constants and
    /// classify audio/video nodes without implying decoder support.
    #[test]
    fn media_interfaces_expose_the_standard_readiness_surface() {
        let mut document = page();
        assert_eq!(
            value(
                &mut document,
                "[HTMLMediaElement.HAVE_NOTHING, HTMLMediaElement.HAVE_METADATA, \
                  HTMLMediaElement.HAVE_CURRENT_DATA, HTMLMediaElement.HAVE_FUTURE_DATA, \
                  HTMLMediaElement.HAVE_ENOUGH_DATA, \
                  document.createElement('video') instanceof HTMLMediaElement, \
                  document.createElement('video') instanceof HTMLVideoElement, \
                  document.createElement('audio') instanceof HTMLAudioElement]"
            ),
            serde_json::json!([0, 1, 2, 3, 4, true, true, true])
        );
    }

    /// Form controls reflect their identifying name as a DOM property so an
    /// event handler can map a target back to its validation rule.
    #[test]
    fn element_name_reflects_the_name_attribute() {
        let mut document = native_page_document(
            "<html><body><input id='field' name='first'></body></html>",
            blitz_dom::DocumentConfig::default(),
            NetProvider::with_user_agent(None, "Browser/123"),
        );

        assert_eq!(
            value(
                &mut document,
                "(function () { var field = document.getElementById('field'); \
                  var before = field.name; field.name = 'second'; \
                  return [before, field.name, field.getAttribute('name')]; })()"
            ),
            serde_json::json!(["first", "second", "second"])
        );
    }

    /// Parsed adjacent markup lands on the requested side in document order.
    #[test]
    fn adjacent_html_inserts_parsed_siblings() {
        let mut document = native_page_document(
            "<html><body><main><input id='field'><span id='tail'></span></main></body></html>",
            blitz_dom::DocumentConfig::default(),
            NetProvider::with_user_agent(None, "Browser/123"),
        );

        assert_eq!(
            value(
                &mut document,
                "(function () { var field = document.getElementById('field'); \
                  field.insertAdjacentHTML('afterend', \
                    '<div role=alert id=reason>invalid</div><b id=marker>next</b>'); \
                  return [field.nextSibling.id, field.nextSibling.nextSibling.id, \
                    document.getElementById('reason').textContent, \
                    document.querySelector('main').children.length]; })()"
            ),
            serde_json::json!(["reason", "marker", "invalid", 4])
        );
    }

    /// A message channel delivers data asynchronously between its paired
    /// ports and a closed port stops further delivery.
    #[test]
    fn message_channel_delivers_between_its_ports() {
        let mut document = page();
        document
            .eval(
                "globalThis.channelMessages = []; \
             var channel = new MessageChannel(); \
             channel.port1.onmessage = function (event) { \
               channelMessages.push(event.data); channel.port1.close(); \
             }; \
             channel.port2.postMessage('first');",
            )
            .expect("guest script executes");
        assert_eq!(
            pump_for(
                &mut document,
                "channelMessages.length === 1 ? channelMessages : null"
            ),
            serde_json::json!(["first"])
        );
    }

    /// The registry promise resolves to the same constructor returned by the
    /// registry, both for an existing definition and one added afterward.
    #[test]
    fn custom_element_definition_promises_follow_the_registry() {
        let mut document = page();
        document
            .eval(
                "globalThis.seenDefinitions = []; \
             class FirstElement extends HTMLElement {} \
             customElements.define('first-element', FirstElement); \
             customElements.whenDefined('first-element').then(function (value) { \
               seenDefinitions.push(value === FirstElement ? 'existing' : 'wrong'); \
             }); \
             customElements.whenDefined('later-element').then(function (value) { \
               seenDefinitions.push(value === LaterElement ? 'later' : 'wrong'); \
             }); \
             class LaterElement extends HTMLElement {} \
             setTimeout(function () { customElements.define('later-element', LaterElement); }, 0);",
            )
            .expect("guest script executes");
        assert_eq!(
            pump_for(
                &mut document,
                "seenDefinitions.length === 2 ? seenDefinitions : null"
            ),
            serde_json::json!(["existing", "later"])
        );
    }

    /// The script-visible identity, language and URL origin are complete and
    /// agree with what the network provider sends.
    #[test]
    fn browser_environment_is_consistent_inside_the_page() {
        let provider = NetProvider::with_user_agent(None, "Browser/123");
        let mut document = native_page_document(
            "<html><body></body></html>",
            blitz_dom::DocumentConfig {
                base_url: Some("https://example.test:8443/start".to_owned()),
                ..Default::default()
            },
            provider,
        );
        assert_eq!(
            value(
                &mut document,
                "[navigator.userAgent, navigator.language, navigator.languages.join(','), \
                  location.origin, new URL('/next', location.origin).href, \
                  typeof PerformanceObserver, typeof navigator.sendBeacon, \
                  PerformanceObserver.supportedEntryTypes]"
            ),
            serde_json::json!([
                "Browser/123",
                "en-US",
                "en-US,en",
                "https://example.test:8443",
                "https://example.test:8443/next",
                "function",
                "function",
                []
            ])
        );
        assert_eq!(
            value(
                &mut document,
                "(function () { var delivered = false; \
                  var observer = new PerformanceObserver(function () { delivered = true; }); \
                  observer.observe({entryTypes: []}); performance.mark('probe'); \
                  observer.disconnect(); return [observer.takeRecords(), delivered]; })()"
            ),
            serde_json::json!([[], false])
        );
    }

    /// Script-visible cookie operations use the profile cookie store.
    #[test]
    fn document_cookie_reads_the_profile_bridge_cache() {
        let provider = NetProvider::with_user_agent(None, "Browser/123");
        let store = provider.cookie_store();
        let url = Url::parse("https://example.test/").expect("fixture URL is valid");
        let mut document = native_page_document(
            "<html><body></body></html>",
            DocumentConfig {
                base_url: Some(url.to_string()),
                ..Default::default()
            },
            provider,
        );
        assert_eq!(
            value(&mut document, "document.cookie"),
            serde_json::json!("")
        );
        store
            .add_response_cookies(&url, ["first=one", "second=two"])
            .expect("fixture cookies are accepted");
        assert_eq!(
            value(&mut document, "document.cookie"),
            serde_json::json!("first=one; second=two")
        );
        document
            .eval("document.cookie = 'first=updated'")
            .expect("set profile cookie");
        assert_eq!(store.script_cookies(&url), "first=updated; second=two");
        assert_eq!(
            value(&mut document, "document.cookie"),
            serde_json::json!("first=updated; second=two")
        );
    }

    /// A caller-selected content language wins; otherwise English is attached
    /// before the request reaches the underlying network provider.
    #[test]
    fn requests_default_to_english_without_overwriting_a_choice() {
        use blitz_traits::net::http::header::{ACCEPT_LANGUAGE, HeaderValue};
        use blitz_traits::net::{Request, Url};

        let ordinary = NetProvider::with_default_language(Request::get(
            Url::parse("https://example.test/").unwrap(),
        ));
        assert_eq!(
            ordinary.headers.get(ACCEPT_LANGUAGE).unwrap(),
            HeaderValue::from_static("en-US,en;q=0.9")
        );

        let mut chosen = Request::get(Url::parse("https://example.test/").unwrap());
        chosen
            .headers
            .insert(ACCEPT_LANGUAGE, HeaderValue::from_static("fr-FR"));
        let chosen = NetProvider::with_default_language(chosen);
        assert_eq!(
            chosen.headers.get(ACCEPT_LANGUAGE).unwrap(),
            HeaderValue::from_static("fr-FR")
        );
    }

    /// ASCII, two-byte, three-byte and a surrogate pair, both ways.
    ///
    /// The callers that reach for `TextEncoder` are hashing or framing bytes, so
    /// a wrong length is worse than a missing constructor: it fails somewhere
    /// else, later, as a bad digest.
    #[test]
    fn text_encoding_is_real_utf8() {
        let mut document = page();
        assert_eq!(
            value(
                &mut document,
                "Array.from(new TextEncoder().encode('A\u{00e9}\u{20ac}\u{1f600}'))"
            ),
            serde_json::json!([0x41, 0xc3, 0xa9, 0xe2, 0x82, 0xac, 0xf0, 0x9f, 0x98, 0x80]),
            "one ASCII, one two-byte, one three-byte and one four-byte code point"
        );
        assert_eq!(
            value(
                &mut document,
                "new TextDecoder().decode(new TextEncoder().encode('A\u{00e9}\u{20ac}\u{1f600}'))"
            ),
            serde_json::json!("A\u{00e9}\u{20ac}\u{1f600}"),
            "decoding what the encoder produced returns the original string"
        );
        assert_eq!(
            value(
                &mut document,
                "new TextDecoder().decode(new Uint8Array([0xc0, 0x80, 0x41]))"
            ),
            serde_json::json!("\u{fffd}\u{fffd}A"),
            "an overlong sequence is replaced rather than decoded"
        );
    }

    /// An object URL is something the engine can actually load.
    ///
    /// A loader that fetches its own bundle, wraps it in a `Blob` and injects
    /// it as `script.src = URL.createObjectURL(blob)` is a common shape, and it
    /// is the shape honey.id ships. With no `Blob` the loader threw before it
    /// reached the URL; with a handle that is only a token the injected script
    /// pointed at something nothing could fetch. Either way the bundle never
    /// ran and the page stayed as its loading placeholder, with nothing in the
    /// log to say why.
    ///
    /// The assertion is that the handle is a `data:` URL carrying the blob's
    /// own bytes, because that is what makes it loadable rather than merely
    /// present.
    #[test]
    fn an_object_url_carries_what_the_blob_holds() {
        let mut document = page();
        assert_eq!(
            value(
                &mut document,
                "URL.createObjectURL(new Blob(['globalThis.x = 1;'], \
                 { type: 'text/javascript' }))"
            ),
            serde_json::json!("data:text/javascript;base64,Z2xvYmFsVGhpcy54ID0gMTs="),
        );
        // Revoking one is not allowed to throw: a page that tidies up on
        // teardown would otherwise take the teardown with it.
        assert_eq!(
            value(
                &mut document,
                "(function () { var u = URL.createObjectURL(new Blob(['a'])); \
                 URL.revokeObjectURL(u); return 'ok'; })()"
            ),
            serde_json::json!("ok"),
        );
    }

    /// Annex B escaping, which is a pure string transform with a specification.
    #[test]
    fn escape_and_unescape_round_trip() {
        let mut document = page();
        assert_eq!(
            value(&mut document, "escape('a b/\u{00e9}\u{20ac}')"),
            serde_json::json!("a%20b/%E9%u20AC"),
            "space and Latin-1 as %XX, above 255 as %uXXXX, and `/` left alone"
        );
        assert_eq!(
            value(&mut document, "unescape(escape('a b/\u{00e9}\u{20ac}'))"),
            serde_json::json!("a b/\u{00e9}\u{20ac}")
        );
    }

    /// `substr` is Annex B, the engine lacks it, and old code still calls it.
    ///
    /// A missing prototype method reads as `TypeError: not a callable function`
    /// rather than a missing global, which is why no corpus count found it.
    #[test]
    fn substr_handles_the_cases_old_code_uses() {
        let mut document = page();
        assert_eq!(
            value(
                &mut document,
                "['abcdef'.substr(2), 'abcdef'.substr(1, 3), 'abcdef'.substr(-2), 'abcdef'.substr(1, 0)]"
            ),
            serde_json::json!(["cdef", "bcd", "ef", ""])
        );
        assert_eq!(
            value(
                &mut document,
                "(function () { var keys = []; for (var key in 'ab') { keys.push(key); } return keys; })()"
            ),
            serde_json::json!(["0", "1"]),
            "a prototype addition must not become enumerable on every string"
        );
    }

    /// A controller aborts its signal, and the abort is observable three ways.
    #[test]
    fn aborting_a_controller_notifies_its_signal() {
        let mut document = page();
        assert_eq!(
            value(
                &mut document,
                "(function () {
                   var controller = new AbortController();
                   var seen = 0;
                   controller.signal.addEventListener('abort', function () { seen++; });
                   controller.signal.onabort = function () { seen++; };
                   var before = controller.signal.aborted;
                   controller.abort();
                   var threw = false;
                   try { controller.signal.throwIfAborted(); } catch (error) { threw = error.name; }
                   return [before, controller.signal.aborted, seen, threw];
                 })()"
            ),
            serde_json::json!([false, true, 2, "AbortError"])
        );
        assert_eq!(
            value(&mut document, "AbortSignal.abort('gone').reason"),
            serde_json::json!("gone"),
            "an explicit reason is kept rather than replaced with an AbortError"
        );
    }

    /// Setting `src` reports a load, asynchronously, to both handler styles.
    ///
    /// Asynchronously matters: a preloader attaches `onload` on the line after
    /// the assignment, and a callback fired during the setter would miss it.
    #[test]
    fn an_image_reports_a_load_after_its_src_is_set() {
        let mut document = page();
        document
            .eval(
                "globalThis.__loaded = null;
             var image = new Image();
             var seen = [];
             image.addEventListener('load', function () { seen.push('listener'); });
             image.onload = function () { seen.push('onload'); globalThis.__loaded = seen; };
             globalThis.__during = image.complete;
             image.src = 'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAYAAABytg0kAAAAAXNSR0IArs4c6QAAADhlWElmTU0AKgAAAAgAAYdpAAQAAAABAAAAGgAAAAAAAqACAAQAAAABAAAAAqADAAQAAAABAAAAAgAAAADO0J6QAAAAG0lEQVQIHWN4LO/JIBx8+T+D6n5rhrtTef8DADtNByG5rydBAAAAAElFTkSuQmCC';",
            )
            .expect("guest script executes");
        assert_eq!(
            value(&mut document, "globalThis.__during"),
            serde_json::json!(false),
            "the load must not be reported from inside the setter"
        );
        assert_eq!(
            pump_for(&mut document, "globalThis.__loaded"),
            serde_json::json!(["onload", "listener"]),
            "both handler styles run once the timer fires"
        );
    }

    /// Base64 both ways, including the unpadded and whitespaced inputs pages send.
    ///
    /// This is the one addition here the corpus did not ask for and measurement
    /// did: `substr` let four of the twelve re-captured pages run past their
    /// first TypeError, and `atob` was the wall they hit next.
    #[test]
    fn base64_round_trips() {
        let mut document = page();
        assert_eq!(
            value(&mut document, "btoa('any carnal pleasure.')"),
            serde_json::json!("YW55IGNhcm5hbCBwbGVhc3VyZS4=")
        );
        assert_eq!(
            value(&mut document, "atob('YW55IGNhcm5hbCBwbGVhc3VyZS4=')"),
            serde_json::json!("any carnal pleasure.")
        );
        assert_eq!(
            value(
                &mut document,
                "[btoa('a'), btoa('ab'), btoa('abc'), atob('YQ'), atob('YWJj')]"
            ),
            serde_json::json!(["YQ==", "YWI=", "YWJj", "a", "abc"]),
            "every padding length, and an unpadded input decoding anyway"
        );
        assert_eq!(
            value(&mut document, "atob('  YW Jj\\n')"),
            serde_json::json!("abc"),
            "whitespace anywhere is stripped rather than rejected"
        );
        assert_eq!(
            value(
                &mut document,
                "(function () { try { atob('!'); } catch (error) { return error.name; } return 'no throw'; })()"
            ),
            serde_json::json!("InvalidCharacterError")
        );
    }

    /// A DOMException carries the name a page branches on, and a legacy code.
    #[test]
    fn dom_exception_is_the_type_a_browser_throws() {
        let mut document = page();
        assert_eq!(
            value(
                &mut document,
                "(function () {
                   var error = new DOMException('nope', 'AbortError');
                   return [error.name, error.message, error.code, String(error)];
                 })()"
            ),
            serde_json::json!(["AbortError", "nope", 20, "AbortError: nope"])
        );
        assert_eq!(
            value(
                &mut document,
                "(function () {
                   var controller = new AbortController();
                   controller.abort();
                   return controller.signal.reason instanceof DOMException;
                 })()"
            ),
            serde_json::json!(true),
            "an abort with no reason throws what a browser throws"
        );
    }

    /// A document with no frames is its own top, which is the true answer.
    #[test]
    fn the_window_is_its_own_top() {
        let mut document = page();
        assert_eq!(
            value(
                &mut document,
                "[globalThis.top === globalThis.self, globalThis.parent === globalThis, globalThis.frameElement]"
            ),
            serde_json::json!([true, true, serde_json::Value::Null]),
            "frame-busting code must not conclude it is framed"
        );
    }

    /// The native runtime leaves missing APIs absent and preserves real APIs.
    /// Native DOM instances and locale formatting must work, rather than
    /// merely exposing constructor names that lie about their capabilities.
    #[test]
    fn the_lying_stubs_are_left_out() {
        let mut document = native_page_document(
            "<html><body></body></html>",
            blitz_dom::DocumentConfig::default(),
            NetProvider::with_user_agent(None, "Browser/123"),
        );
        document
            .eval(
                "globalThis.__beforeNative = { ReadableStream: globalThis.ReadableStream, \
             NodeList: globalThis.NodeList, DocumentFragment: globalThis.DocumentFragment, \
             CharacterData: globalThis.CharacterData, Intl: globalThis.Intl, \
             ActiveXObject: globalThis.ActiveXObject };",
            )
            .expect("guest script executes");

        for name in [
            "ReadableStream",
            "NodeList",
            "DocumentFragment",
            "CharacterData",
            "Intl",
            "ActiveXObject",
        ] {
            assert_eq!(
                value(
                    &mut document,
                    &format!("globalThis.{name} === __beforeNative.{name}")
                ),
                serde_json::json!(true),
                "the native runtime must preserve the engine's {name}"
            );
        }
        assert_eq!(
            value(
                &mut document,
                "[typeof ReadableStream, typeof ActiveXObject]"
            ),
            serde_json::json!(["undefined", "undefined"])
        );
        assert_eq!(
            value(
                &mut document,
                "(function () { var text = document.createTextNode('first'); \
                  text.data = 'second'; return [ \
                  document.querySelectorAll('body') instanceof NodeList, \
                  document.createDocumentFragment() instanceof DocumentFragment, \
                  text instanceof CharacterData, text.nodeValue, \
                  new Intl.NumberFormat('de-DE').format(1234.5)]; })()"
            ),
            serde_json::json!([true, true, true, "second", "1.234,5"])
        );
    }
}
