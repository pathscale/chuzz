//! Concrete page runtime adapter for the bounded native guest.
//!
//! The adapter owns one RuntimeDriver and obtains the page host through that
//! driver's live guest. The application host is stateless: every operation
//! resolves the driver from the live document passed by izumo, on its owner
//! thread, so script calls and IPC stay in the same realm.

#[cfg(feature = "gui")]
use std::any::Any;
use std::sync::Arc;
#[cfg(feature = "gui")]
use std::sync::OnceLock;
use std::task::Context;

use blitz_dom::{DocGuard, DocGuardMut, Document, DocumentConfig, NodeId};
use blitz_traits::events::{BlitzPointerEvent, DomEvent, UiEvent};
#[cfg(feature = "gui")]
use url::Url;

use crate::page_document::{PageDiagnosticBatch, PageRuntimeDriver};
use crate::page_script_host::ChuzzPageRuntimeHost;
use blitz_script::RuntimeDriver;
use blitz_script::native::{Error as GuestError, Guest, GuestHost};

use crate::page_native_bridge::ChuzzBridge;

/// The engine-neutral page driver backed by the Rust AST guest.
pub(super) struct PageNativeDriver {
    runtime: RuntimeDriver<Guest<ChuzzBridge>, GuestHost<ChuzzBridge>>,
}

struct PageScriptFetcher(crate::net_bridge::NetworkHost);

impl blitz_script::ScriptFetcher for PageScriptFetcher {
    fn fetch(&self, url: &url::Url) -> Result<String, blitz_script::FetchError> {
        use blitz_script::FetchError;
        let reply = nagoya::block_on(self.0.perform(crate::net_bridge::NetRequest {
            id: 0,
            url: url.to_string(),
            method: String::from("GET"),
            headers: Vec::new(),
            body: None,
            credentials: String::from("same-origin"),
        }))
        .map_err(|error| FetchError::InvalidData(error.to_string()))?;
        if !(200..300).contains(&reply.status) {
            return Err(FetchError::InvalidData(format!(
                "script request returned status {}",
                reply.status
            )));
        }
        if reply.bytes.len() > 16 * 1024 * 1024 {
            return Err(FetchError::InvalidData(String::from(
                "script exceeds source byte limit",
            )));
        }
        String::from_utf8(reply.bytes).map_err(|error| FetchError::InvalidData(error.to_string()))
    }

    fn fetch_for_document(
        &self,
        url: &url::Url,
        document: Option<&url::Url>,
    ) -> Result<String, blitz_script::FetchError> {
        if matches!(url.scheme(), "http" | "https") {
            self.fetch(url)
        } else {
            blitz_script::DefaultScriptFetcher.fetch_for_document(url, document)
        }
    }
}

/// Construct the built-in driver. A registered factory remains an explicit
/// override and is selected by PageDocument before calling this function.
pub(super) fn default_page_runtime_factory(
    html: Arc<str>,
    config: DocumentConfig,
    host: ChuzzPageRuntimeHost,
) -> Result<Box<dyn PageRuntimeDriver>, String> {
    let fetcher = PageScriptFetcher(host.script_network().map_err(|error| error.to_string())?);
    let bridge = ChuzzBridge::new(host)?;
    let guest = Guest::new(bridge);
    let document = blitz_script::BoundScriptDocument::new_bound_with_fetcher(
        html,
        config,
        GuestHost(guest),
        fetcher,
    )?;
    let runtime = RuntimeDriver::from_document(document)?;
    Ok(Box::new(PageNativeDriver { runtime }))
}

impl PageNativeDriver {
    fn page_host(&self) -> &ChuzzPageRuntimeHost {
        &self
            .runtime
            .guest()
            .expect("a live RuntimeDriver retains its bound guest")
            .bridge
            .host
    }

    fn page_host_mut(&mut self) -> &mut ChuzzPageRuntimeHost {
        &mut self
            .runtime
            .guest_mut()
            .expect("a live RuntimeDriver retains its bound guest")
            .bridge
            .host
    }

    fn runtime_error(&mut self, message: String) {
        let error = GuestError::new("Error", message);
        let _ = self
            .runtime
            .with_guest_mut(|guest, _document| guest.report_error(error));
    }
}

impl PageRuntimeDriver for PageNativeDriver {
    fn host(&self) -> &ChuzzPageRuntimeHost {
        self.page_host()
    }

    fn host_mut(&mut self) -> &mut ChuzzPageRuntimeHost {
        self.page_host_mut()
    }

    #[cfg(feature = "gui")]
    fn application_host(&self) -> Arc<dyn izumo::DocumentHost> {
        static HOST: OnceLock<Arc<PageDocumentHost>> = OnceLock::new();
        HOST.get_or_init(|| Arc::new(PageDocumentHost)).clone()
    }

    fn execute_scripts(&mut self) -> Result<(), String> {
        self.runtime.execute_scripts()
    }

    fn eval(&mut self, source: &str) -> Result<(), String> {
        self.runtime.eval(source)
    }

    fn eval_named(&mut self, source: &str, name: &str) -> Result<(), String> {
        self.runtime.eval_named(source, name)
    }

    fn eval_json(&mut self, source: &str) -> Result<serde_json::Value, String> {
        self.runtime.eval_json(source)
    }

    fn has_pending_jobs(&self) -> bool {
        self.runtime.has_pending_jobs()
    }

    fn pending_task_count(&self) -> u64 {
        self.runtime.pending_task_count()
    }

    fn report_runtime_error(&mut self, error: String) {
        self.runtime_error(error);
    }

    fn dispatch_node_event(&mut self, node: NodeId, name: &str) -> Result<bool, String> {
        self.runtime.dispatch_node_event(node, name)
    }

    fn diagnostic_batch(
        &mut self,
        console_after: u64,
        errors_after: u64,
        limit: usize,
    ) -> Result<PageDiagnosticBatch, String> {
        self.runtime
            .diagnostic_batch(console_after, errors_after, limit)
    }

    fn poll_runtime(&mut self, context: Option<Context>) -> Result<bool, String> {
        let work = Document::poll(&mut self.runtime, context);
        match self.runtime.document_mut().take_last_error() {
            Some(error) => Err(error),
            None => Ok(work),
        }
    }

    fn poll_runtime_for_settle(
        &mut self,
        context: Option<Context>,
        run_timers: bool,
    ) -> Result<bool, String> {
        let work = Document::poll_for_settle(&mut self.runtime, context, run_timers);
        match self.runtime.document_mut().take_last_error() {
            Some(error) => Err(error),
            None => Ok(work),
        }
    }
}

impl Document for PageNativeDriver {
    fn inner(&self) -> DocGuard<'_> {
        self.runtime.inner()
    }

    fn inner_mut(&mut self) -> DocGuardMut<'_> {
        self.runtime.inner_mut()
    }

    fn handle_ui_event(&mut self, event: UiEvent) {
        self.runtime.handle_ui_event(event);
    }

    fn poll(&mut self, context: Option<Context>) -> bool {
        Document::poll(&mut self.runtime, context)
    }

    fn poll_for_settle(&mut self, context: Option<Context>, run_timers: bool) -> bool {
        Document::poll_for_settle(&mut self.runtime, context, run_timers)
    }

    fn prepare_teardown(&mut self) {
        self.runtime.prepare_teardown();
    }

    fn dispose(self: Box<Self>) -> Arc<blitz_dom::release::Teardown> {
        let Self { runtime } = *self;
        Box::new(runtime).dispose()
    }

    fn set_page_visible(&mut self, visible: bool) {
        self.runtime.set_page_visible(visible);
    }

    fn freeze(&mut self) {
        self.runtime.freeze();
    }

    fn finish_restore(&mut self) {
        self.runtime.finish_restore();
    }

    fn resume(&mut self) {
        self.runtime.resume();
    }
}

impl blitz_control_protocol::ControlScriptBackend for PageNativeDriver {
    fn dispatch_dom_event(&mut self, event: DomEvent) {
        if let Err(error) = self.runtime.dispatch_dom_event(event) {
            self.runtime_error(error);
        }
    }

    fn handle_pointer_move_to_node(&mut self, event: BlitzPointerEvent, node: NodeId) {
        if let Err(error) = self.runtime.handle_pointer_move_to_node(event, node) {
            self.runtime_error(error);
        }
    }

    fn handle_ui_event_to_node(&mut self, event: UiEvent, node: NodeId) {
        if let Err(error) = self.runtime.handle_ui_event_to_node(event, node) {
            self.runtime_error(error);
        }
    }

    fn script_metrics(&self) -> Option<blitz_control_protocol::ScriptMetrics> {
        None
    }
}

/// Stateless izumo adapter. It never retains a document or guest reference.
#[cfg(feature = "gui")]
struct PageDocumentHost;

#[cfg(feature = "gui")]
fn page_ref(document: &dyn Document) -> Option<&PageNativeDriver> {
    let any = document as &dyn Any;
    any.downcast_ref::<PageNativeDriver>().or_else(|| {
        any.downcast_ref::<crate::page_document::PageDocument>()?
            .driver_any()
            .downcast_ref()
    })
}

#[cfg(feature = "gui")]
fn page_mut(document: &mut dyn Document) -> Option<&mut PageNativeDriver> {
    let any = document as &mut dyn Any;
    if any.is::<PageNativeDriver>() {
        return any.downcast_mut();
    }
    any.downcast_mut::<crate::page_document::PageDocument>()?
        .driver_any_mut()
        .downcast_mut()
}

#[cfg(feature = "gui")]
fn host_error(code: &str, message: &str) -> blitz_control_protocol::DebugResponse {
    blitz_control_protocol::DebugResponse::Error(blitz_control_protocol::DebugError {
        code: code.to_owned(),
        message: message.to_owned(),
    })
}

#[cfg(feature = "gui")]
impl izumo::DocumentHost for PageDocumentHost {
    fn current_url(&self, document: &dyn Document) -> Option<Url> {
        page_ref(document).map(|_| document.inner().url().clone())
    }

    fn eval(&self, document: &mut dyn Document, source: &str) {
        if let Some(page) = page_mut(document)
            && let Err(error) = page.eval(source)
        {
            page.report_runtime_error(error);
        }
    }

    fn eval_json(
        &self,
        document: &mut dyn Document,
        source: &str,
    ) -> Result<serde_json::Value, String> {
        page_mut(document)
            .ok_or_else(|| String::from("document is not owned by the page runtime"))?
            .eval_json(source)
    }

    fn set_ipc_handler(&self, document: &mut dyn Document, handler: Box<dyn Fn(String)>) {
        if let Some(page) = page_mut(document)
            && let Err(error) = page.runtime.set_ipc_handler(handler)
        {
            page.report_runtime_error(error);
        }
    }

    fn add_poll_hook(&self, document: &mut dyn Document, hook: izumo::DocumentPollHook) {
        if let Some(page) = page_mut(document)
            && let Err(error) = page.runtime.add_poll_hook(hook)
        {
            page.report_runtime_error(error);
        }
    }

    #[cfg(feature = "gui")]
    fn service_debug_controller(
        &self,
        _document: &mut dyn Document,
        _controller: &mut blitz_script::DebugController,
        _animation_time: f64,
    ) -> bool {
        false
    }

    #[cfg(all(feature = "gui", unix))]
    fn agent(
        &self,
        document: &mut dyn Document,
        control: &mut blitz_control_protocol::in_process::DocumentControl,
        request: blitz_control_protocol::AgentControlRequest,
    ) -> blitz_control_protocol::DebugResponse {
        match page_mut(document) {
            Some(page) => control.agent(page, request),
            None => host_error(
                "unsupportedDocument",
                "document is not owned by the page runtime",
            ),
        }
    }

    #[cfg(all(feature = "gui", unix))]
    fn diagnostics(
        &self,
        _document: &mut dyn Document,
        _control: &mut blitz_control_protocol::in_process::DocumentControl,
        _request: blitz_control_protocol::DiagnosticsRequest,
    ) -> blitz_control_protocol::DebugResponse {
        host_error(
            "unsupportedRequest",
            "the bounded page runtime does not implement this diagnostics operation",
        )
    }

    #[cfg(all(feature = "gui", unix))]
    fn diagnostic_batch(
        &self,
        document: &dyn Document,
        console_after: u64,
        errors_after: u64,
        limit: usize,
    ) -> blitz_script::DiagnosticBatch {
        let empty = |next_sequence| blitz_script::DiagnosticStreamBatch {
            entries: Vec::new(),
            lost: 0,
            next_sequence,
        };
        page_ref(document)
            .and_then(|page| {
                page.runtime
                    .diagnostic_batch(console_after, errors_after, limit)
                    .ok()
            })
            .unwrap_or_else(|| blitz_script::DiagnosticBatch {
                console: empty(console_after),
                errors: empty(errors_after),
            })
    }
}
