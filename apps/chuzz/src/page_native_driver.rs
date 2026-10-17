//! Concrete page runtime adapter for the native Ekojit document.
//!
//! The document owns the guest realm. The native host is shared with the
//! owner-local bridge so every intrinsic callback reaches this page's host.

use std::cell::{Ref, RefCell, RefMut};
use std::rc::Rc;
use std::sync::Arc;
use std::task::Context;

#[cfg(feature = "gui")]
use std::any::Any;
#[cfg(feature = "gui")]
use std::sync::OnceLock;

use blitz_dom::{DocGuard, DocGuardMut, Document, DocumentConfig, NodeId};
use blitz_script::{DiagnosticBatch, DiagnosticEntry, DiagnosticStreamBatch};
use blitz_script_ekojit::{EkojitDocument, NativePageDefect, PageSource, TypedNativeGuest};
use blitz_traits::events::{BlitzPointerEvent, DomEvent, UiEvent};
#[cfg(feature = "gui")]
use url::Url;

use crate::page_document::{
    PageDiagnosticBatch, PageNativeMetrics, PageNativeReceipt, PageRuntimeDriver,
};
use crate::page_script_host::ChuzzPageRuntimeHost;

/// The engine-neutral page driver backed by one persistent native realm.
pub(super) struct PageNativeDriver {
    document: EkojitDocument,
    host: Rc<RefCell<ChuzzPageRuntimeHost>>,
    #[cfg(feature = "gui")]
    poll_hooks: Vec<izumo::DocumentPollHook>,
    #[cfg(feature = "gui")]
    deferred_scripts: Vec<(Arc<str>, Arc<str>)>,
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
    mut config: DocumentConfig,
    host: ChuzzPageRuntimeHost,
) -> Result<Box<dyn PageRuntimeDriver>, String> {
    if config.net_provider.is_none() {
        config.net_provider =
            Some(Arc::clone(host.net_provider().map_err(|error| error.to_string())?) as _);
    }
    let fetcher = PageScriptFetcher(host.script_network().map_err(|error| error.to_string())?);
    let host = Rc::new(RefCell::new(host));
    let mut document = EkojitDocument::from_html(html, config, NativePageDefect::None)
        .map_err(|error| error.to_string())?;
    crate::page_ekojit_bridge::install(&mut document, Rc::clone(&host))?;
    crate::page_ekojit_bootstrap::install(&mut document)?;
    crate::page_ekojit_compat::install(&mut document)?;
    document.set_script_fetcher(fetcher);
    Ok(Box::new(PageNativeDriver {
        document,
        host,
        #[cfg(feature = "gui")]
        poll_hooks: Vec::new(),
        #[cfg(feature = "gui")]
        deferred_scripts: Vec::new(),
    }))
}

impl PageNativeDriver {
    fn source(source: &str, name: &str) -> Arc<PageSource> {
        Arc::new(PageSource {
            text: Arc::from(source),
            url: Arc::from(name),
        })
    }

    fn evaluate(&mut self, source: &str, name: &str) -> Result<(), String> {
        TypedNativeGuest::evaluate(&mut self.document, Self::source(source, name))
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    fn diagnostics(
        entries: Vec<DiagnosticEntry>,
        after: u64,
        limit: usize,
    ) -> DiagnosticStreamBatch {
        let first = entries
            .first()
            .map_or(after.saturating_add(1), |entry| entry.sequence);
        let lost = first.saturating_sub(after.saturating_add(1));
        let entries: Vec<_> = entries
            .into_iter()
            .filter(|entry| entry.sequence > after)
            .take(limit.clamp(1, 1000))
            .collect();
        let next_sequence = entries.last().map_or(after, |entry| entry.sequence);
        DiagnosticStreamBatch {
            entries,
            lost,
            next_sequence,
        }
    }

    fn page_host(&self) -> Ref<'_, ChuzzPageRuntimeHost> {
        self.host.borrow()
    }

    fn page_host_mut(&self) -> RefMut<'_, ChuzzPageRuntimeHost> {
        self.host.borrow_mut()
    }

    #[cfg(feature = "gui")]
    fn run_poll_hooks(&mut self, context: Option<&Context<'_>>) -> bool {
        let mut hooks = std::mem::take(&mut self.poll_hooks);
        let mut work = false;
        for hook in &mut hooks {
            work |= hook(self, context);
        }
        hooks.append(&mut self.poll_hooks);
        self.poll_hooks = hooks;
        work
    }
}

impl PageRuntimeDriver for PageNativeDriver {
    #[cfg(feature = "gui")]
    fn defer_named_script(&mut self, source: Arc<str>, name: Arc<str>) {
        self.deferred_scripts.push((source, name));
    }
    fn host(&self) -> Ref<'_, ChuzzPageRuntimeHost> {
        self.page_host()
    }

    fn host_mut(&mut self) -> RefMut<'_, ChuzzPageRuntimeHost> {
        self.page_host_mut()
    }

    #[cfg(feature = "gui")]
    fn application_host(&self) -> Arc<dyn izumo::DocumentHost> {
        static HOST: OnceLock<Arc<PageDocumentHost>> = OnceLock::new();
        HOST.get_or_init(|| Arc::new(PageDocumentHost)).clone()
    }

    fn set_was_discarded(&mut self, discarded: bool) {
        self.document.set_was_discarded(discarded);
    }

    fn begin_restore(&mut self, budget: std::time::Duration) {
        self.document.begin_restore(budget);
    }

    fn execute_scripts(&mut self) -> Result<(), String> {
        TypedNativeGuest::execute_scripts(&mut self.document).map_err(|error| error.to_string())
    }

    fn eval(&mut self, source: &str) -> Result<(), String> {
        self.evaluate(source, "<eval>")
    }

    fn eval_named(&mut self, source: &str, name: &str) -> Result<(), String> {
        self.evaluate(source, name)
    }

    fn eval_json(&mut self, source: &str) -> Result<serde_json::Value, String> {
        crate::page_ekojit_bridge::refresh_diagnostics(&mut self.document)?;
        self.document
            .with_embedder_host(|host| host.eval_page_json(source))
    }

    fn has_pending_jobs(&self) -> bool {
        self.document.has_pending_jobs()
            || self
                .page_host()
                .next_timer_deadline_ns()
                .is_some_and(|deadline| deadline <= nagoya::now_ns())
    }

    fn pending_task_count(&self) -> u64 {
        self.document.pending_task_count() + self.page_host().timer_count() as u64
    }

    fn report_runtime_error(&mut self, error: String) {
        self.document.report_runtime_error(error);
    }

    fn native_metrics(&self) -> Option<PageNativeMetrics> {
        let (compilations, native_entries) = self.document.native_counts();
        let receipts = self
            .document
            .native_receipts()
            .iter()
            .map(|receipt| PageNativeReceipt {
                source_url: receipt.source_url.to_string(),
                backend: receipt.backend.to_owned(),
                entry_abi: receipt.entry_abi.to_owned(),
                native_entries: receipt.native_entries,
                host_calls: receipt.host_calls,
                code_bytes: receipt.code_bytes,
            })
            .collect();
        Some(PageNativeMetrics {
            compilations,
            native_entries,
            receipts,
        })
    }

    fn dispatch_node_event(&mut self, node: NodeId, name: &str) -> Result<bool, String> {
        Ok(TypedNativeGuest::dispatch_node_event(
            &mut self.document,
            node,
            name,
        ))
    }

    fn diagnostic_batch(
        &mut self,
        console_after: u64,
        errors_after: u64,
        limit: usize,
    ) -> Result<PageDiagnosticBatch, String> {
        Ok(DiagnosticBatch {
            console: Self::diagnostics(self.document.console_entries(), console_after, limit),
            errors: Self::diagnostics(self.document.runtime_errors(), errors_after, limit),
        })
    }

    fn poll_runtime(&mut self, context: Option<Context>) -> Result<bool, String> {
        #[cfg(feature = "gui")]
        for (source, name) in std::mem::take(&mut self.deferred_scripts) {
            self.evaluate(&source, &name)?;
        }
        #[cfg(feature = "gui")]
        let mut work = {
            let poll_context = context
                .as_ref()
                .map(|context| Context::from_waker(context.waker()));
            TypedNativeGuest::poll_runtime(&mut self.document, poll_context)
        };
        #[cfg(not(feature = "gui"))]
        let mut work = TypedNativeGuest::poll_runtime(&mut self.document, context);
        #[cfg(feature = "gui")]
        {
            work |= self.run_poll_hooks(context.as_ref());
        }
        Ok(work)
    }

    fn poll_runtime_for_settle(
        &mut self,
        context: Option<Context>,
        run_timers: bool,
    ) -> Result<bool, String> {
        #[cfg(feature = "gui")]
        for (source, name) in std::mem::take(&mut self.deferred_scripts) {
            self.evaluate(&source, &name)?;
        }
        #[cfg(feature = "gui")]
        let mut work = {
            let poll_context = context
                .as_ref()
                .map(|context| Context::from_waker(context.waker()));
            TypedNativeGuest::poll_runtime_for_settle(&mut self.document, poll_context, run_timers)
        };
        #[cfg(not(feature = "gui"))]
        let mut work =
            TypedNativeGuest::poll_runtime_for_settle(&mut self.document, context, run_timers);
        #[cfg(feature = "gui")]
        {
            work |= self.run_poll_hooks(context.as_ref());
        }
        Ok(work)
    }

    fn next_timer_deadline_ns(&self) -> Option<u64> {
        let host_deadline = self.page_host().next_timer_deadline_ns();
        let native_deadline = self.document.next_timer_delay().map(|delay| {
            let delay_ns = u64::try_from(delay.as_nanos()).unwrap_or(u64::MAX);
            nagoya::now_ns().saturating_add(delay_ns)
        });
        match (host_deadline, native_deadline) {
            (Some(left), Some(right)) => Some(left.min(right)),
            (Some(deadline), None) | (None, Some(deadline)) => Some(deadline),
            (None, None) => None,
        }
    }
}

impl Document for PageNativeDriver {
    fn inner(&self) -> DocGuard<'_> {
        self.document.inner()
    }

    fn inner_mut(&mut self) -> DocGuardMut<'_> {
        self.document.inner_mut()
    }

    fn handle_ui_event(&mut self, event: UiEvent) {
        self.document.handle_ui_event(event);
    }

    fn poll(&mut self, context: Option<Context>) -> bool {
        PageRuntimeDriver::poll_runtime(self, context).unwrap_or(false)
    }

    fn poll_for_settle(&mut self, context: Option<Context>, run_timers: bool) -> bool {
        PageRuntimeDriver::poll_runtime_for_settle(self, context, run_timers).unwrap_or(false)
    }

    fn prepare_teardown(&mut self) {
        self.document.prepare_teardown();
    }

    fn dispose(self: Box<Self>) -> Arc<blitz_dom::release::Teardown> {
        let Self { document, .. } = *self;
        Box::new(document).dispose()
    }

    fn set_page_visible(&mut self, visible: bool) {
        self.document.set_page_visible(visible);
    }

    fn freeze(&mut self) {
        self.document.freeze();
    }

    fn finish_restore(&mut self) {
        self.document.finish_restore();
    }

    fn resume(&mut self) {
        self.document.resume();
    }
}

impl blitz_control_protocol::ControlScriptBackend for PageNativeDriver {
    fn dispatch_dom_event(&mut self, event: DomEvent) {
        self.document.handle_dom_event(event);
    }

    fn handle_pointer_move_to_node(&mut self, event: BlitzPointerEvent, node: NodeId) {
        self.document.handle_pointer_move_to_node(event, node);
    }

    fn handle_ui_event_to_node(&mut self, event: UiEvent, node: NodeId) {
        self.document.handle_ui_event_to_node(event, node);
    }

    fn script_metrics(&self) -> Option<blitz_control_protocol::ScriptMetrics> {
        // Native receipts have entry and host-call counts but no elapsed-time
        // samples, so they cannot truthfully populate ScriptMetrics' timing fields.
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
            && let Err(error) = crate::page_ekojit_bridge::install_ipc(&mut page.document, handler)
        {
            page.report_runtime_error(error);
        }
    }

    fn add_poll_hook(&self, document: &mut dyn Document, hook: izumo::DocumentPollHook) {
        if let Some(page) = page_mut(document) {
            page.poll_hooks.push(hook);
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
            "the native page backend does not implement this diagnostics operation",
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
            .map(|page| blitz_script::DiagnosticBatch {
                console: PageNativeDriver::diagnostics(
                    page.document.console_entries(),
                    console_after,
                    limit,
                ),
                errors: PageNativeDriver::diagnostics(
                    page.document.runtime_errors(),
                    errors_after,
                    limit,
                ),
            })
            .unwrap_or_else(|| blitz_script::DiagnosticBatch {
                console: empty(console_after),
                errors: empty(errors_after),
            })
    }
}
