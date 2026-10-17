//! Engine-neutral owner for one parsed page and its native runtime host.
//!
//! The native Ekojit document is the default. An embedder may register a
//! replacement before the first page is constructed; that explicit factory
//! takes precedence over the default.

use std::cell::{Ref, RefMut};
use std::sync::{Arc, OnceLock};
use std::task::Context;

use blitz_dom::{DocGuard, DocGuardMut, Document, DocumentConfig, NodeId};
use blitz_traits::events::{BlitzPointerEvent, DomEvent, UiEvent};

use crate::page_native_driver::default_page_runtime_factory;
use crate::page_script_host::ChuzzPageRuntimeHost;
use crate::script_host::{HostError, HostInstallContext};

pub use blitz_script::{DiagnosticBatch as PageDiagnosticBatch, DiagnosticEntry as PageDiagnostic};

/// Native Ekojit counters and the receipts emitted by this page realm.
#[derive(Clone, Debug)]
pub struct PageNativeMetrics {
    pub compilations: u64,
    pub native_entries: u64,
    pub receipts: Vec<PageNativeReceipt>,
}

/// A guest-independent summary of one native source activation.
#[derive(Clone, Debug)]
pub struct PageNativeReceipt {
    pub source_url: String,
    pub backend: String,
    pub entry_abi: String,
    pub native_entries: u64,
    pub host_calls: u64,
    pub code_bytes: usize,
}

/// Engine-neutral operations required from a real page runtime.
///
/// The implementation owns the parsed document and the exact host supplied
/// during construction. Its poll methods must poll both guest work and
/// `ChuzzPageRuntimeHost` work. Event methods must preserve targeted delivery,
/// bubbling, cancellation, native defaults, and pointer hover ordering.
pub trait PageRuntimeDriver:
    blitz_dom::Document + blitz_control_protocol::ControlScriptBackend
{
    fn host(&self) -> Ref<'_, ChuzzPageRuntimeHost>;
    fn host_mut(&mut self) -> RefMut<'_, ChuzzPageRuntimeHost>;

    #[cfg(feature = "gui")]
    fn application_host(&self) -> Arc<dyn izumo::DocumentHost>;

    #[cfg(feature = "gui")]
    fn defer_named_script(&mut self, source: Arc<str>, name: Arc<str>);

    fn set_was_discarded(&mut self, discarded: bool);
    fn begin_restore(&mut self, budget: std::time::Duration);
    fn execute_scripts(&mut self) -> Result<(), String>;
    fn eval(&mut self, source: &str) -> Result<(), String>;
    fn eval_named(&mut self, source: &str, name: &str) -> Result<(), String>;
    fn eval_json(&mut self, source: &str) -> Result<serde_json::Value, String>;
    fn has_pending_jobs(&self) -> bool;
    fn pending_task_count(&self) -> u64;
    fn report_runtime_error(&mut self, error: String);

    fn native_metrics(&self) -> Option<PageNativeMetrics> {
        None
    }

    /// Dispatch a named event through the backend's event implementation.
    fn dispatch_node_event(&mut self, node: NodeId, name: &str) -> Result<bool, String>;

    /// Return records newer than each independent cursor.
    fn diagnostic_batch(
        &mut self,
        console_after: u64,
        errors_after: u64,
        limit: usize,
    ) -> Result<PageDiagnosticBatch, String>;

    /// Seed storage before document scripts execute.
    fn seed_session(&mut self, entries: &[(String, String)]) -> Result<(), String> {
        self.host_mut()
            .seed_session(entries)
            .map_err(format_host_error)
    }

    /// Capture the host's session state using the caller's storage budget.
    fn capture_session(
        &self,
        budget: &Arc<crate::tab_stash::SleepingBudget>,
    ) -> Result<Option<Arc<crate::tab_stash::SessionBucket>>, String> {
        self.host()
            .capture_session(budget)
            .map_err(format_host_error)
    }

    /// Allow the backend to perform its normal poll, including host callbacks.
    fn poll_runtime(&mut self, context: Option<Context>) -> Result<bool, String>;

    /// Drain immediately runnable work according to the backend's settle policy.
    fn poll_runtime_for_settle(
        &mut self,
        context: Option<Context>,
        run_timers: bool,
    ) -> Result<bool, String>;

    fn next_timer_deadline_ns(&self) -> Option<u64> {
        self.host().next_timer_deadline_ns()
    }
}

/// Factory for a real engine-bound page runtime.
///
/// The supplied host must be the host used by the guest's bindings and
/// callbacks. Returning an error drops the host and any partially created
/// runtime.
pub type PageRuntimeFactory = fn(
    Arc<str>,
    DocumentConfig,
    ChuzzPageRuntimeHost,
) -> Result<Box<dyn PageRuntimeDriver>, String>;

static PAGE_RUNTIME_FACTORY: OnceLock<PageRuntimeFactory> = OnceLock::new();

/// Register the one real page backend for this process.
pub fn register_page_runtime_factory(factory: PageRuntimeFactory) -> Result<(), String> {
    PAGE_RUNTIME_FACTORY
        .set(factory)
        .map_err(|_| String::from("a page runtime factory is already registered"))
}

/// A parsed page and the driver and native host that share its realm.
pub struct PageDocument {
    driver: Box<dyn PageRuntimeDriver>,
}

impl PageDocument {
    #[cfg(feature = "gui")]
    pub(crate) fn driver_any(&self) -> &dyn std::any::Any {
        self.driver.as_ref()
    }

    #[cfg(feature = "gui")]
    pub(crate) fn driver_any_mut(&mut self) -> &mut dyn std::any::Any {
        self.driver.as_mut()
    }
    /// Construct a page using an explicitly registered backend, or the
    /// native Ekojit runtime when no override has been registered.
    pub fn from_html(
        html: Arc<str>,
        mut document_config: DocumentConfig,
        mut host_config: HostInstallContext,
    ) -> Result<Self, String> {
        if matches!(
            host_config.net.initiator(),
            blitz_traits::net::Initiator::Unknown
        ) && let Some(base) = host_config.base.clone()
        {
            host_config.net = Arc::new(
                host_config
                    .net
                    .for_document(base, &blitz_traits::net::Initiator::Unknown),
            );
        }
        document_config.teardown = Some(Arc::clone(&host_config.lifetime));
        document_config.abort_signal = Some(host_config.signal.clone());
        if document_config.net_provider.is_none() {
            document_config.net_provider = Some(Arc::clone(&host_config.net) as _);
        }
        if document_config.html_parser_provider.is_none() {
            document_config.html_parser_provider = Some(Arc::new(blitz_html::HtmlProvider));
        }
        let factory = PAGE_RUNTIME_FACTORY
            .get()
            .copied()
            .unwrap_or(default_page_runtime_factory);
        let host =
            ChuzzPageRuntimeHost::new_page_script_host(host_config).map_err(format_host_error)?;
        Ok(Self {
            driver: factory(html, document_config, host)?,
        })
    }

    /// Construct a page and seed session storage before its scripts execute.
    pub fn from_html_with_session(
        html: Arc<str>,
        document_config: DocumentConfig,
        host_config: HostInstallContext,
        session: &[(String, String)],
    ) -> Result<Self, String> {
        let mut page = Self::from_html(html, document_config, host_config)?;
        page.driver.seed_session(session)?;
        Ok(page)
    }

    pub fn host(&self) -> Ref<'_, ChuzzPageRuntimeHost> {
        self.driver.host()
    }

    pub fn host_mut(&mut self) -> RefMut<'_, ChuzzPageRuntimeHost> {
        self.driver.host_mut()
    }

    #[cfg(feature = "gui")]
    pub fn application_host(&self) -> Arc<dyn izumo::DocumentHost> {
        self.driver.application_host()
    }

    #[cfg(feature = "gui")]
    pub(crate) fn defer_named_script(&mut self, source: Arc<str>, name: Arc<str>) {
        self.driver.defer_named_script(source, name);
    }

    pub fn set_was_discarded(&mut self, discarded: bool) {
        self.driver.set_was_discarded(discarded);
    }

    pub fn begin_restore(&mut self, budget: std::time::Duration) {
        self.driver.begin_restore(budget);
    }

    pub fn execute_scripts(&mut self) -> Result<(), String> {
        self.driver.execute_scripts()
    }

    pub fn eval(&mut self, source: &str) -> Result<(), String> {
        self.driver.eval(source)
    }

    pub fn eval_named(&mut self, source: &str, name: &str) -> Result<(), String> {
        self.driver.eval_named(source, name)
    }

    pub fn eval_json(&mut self, source: &str) -> Result<serde_json::Value, String> {
        self.driver.eval_json(source)
    }

    pub fn has_pending_jobs(&self) -> bool {
        self.driver.has_pending_jobs()
    }

    pub fn pending_task_count(&self) -> u64 {
        self.driver.pending_task_count()
    }

    pub fn native_metrics(&self) -> Option<PageNativeMetrics> {
        self.driver.native_metrics()
    }

    pub fn dispatch_node_event(&mut self, node: NodeId, name: &str) -> Result<bool, String> {
        self.driver.dispatch_node_event(node, name)
    }

    pub fn diagnostic_batch(
        &mut self,
        console_after: u64,
        errors_after: u64,
        limit: usize,
    ) -> Result<PageDiagnosticBatch, String> {
        self.driver
            .diagnostic_batch(console_after, errors_after, limit)
    }

    pub fn capture_session(
        &self,
        budget: &Arc<crate::tab_stash::SleepingBudget>,
    ) -> Result<Option<Arc<crate::tab_stash::SessionBucket>>, String> {
        self.driver.capture_session(budget)
    }

    pub fn next_timer_deadline_ns(&self) -> Option<u64> {
        self.driver.next_timer_deadline_ns()
    }
}

fn format_host_error(error: HostError) -> String {
    format!(
        "could not initialize page native host: {}: {}",
        error.name, error.message
    )
}

impl Document for PageDocument {
    fn inner(&self) -> DocGuard<'_> {
        self.driver.inner()
    }

    fn inner_mut(&mut self) -> DocGuardMut<'_> {
        self.driver.inner_mut()
    }

    fn handle_ui_event(&mut self, event: UiEvent) {
        self.driver.handle_ui_event(event);
    }

    fn poll(&mut self, context: Option<Context>) -> bool {
        match self.driver.poll_runtime(context) {
            Ok(work) => work,
            Err(error) => {
                self.driver.report_runtime_error(error);
                false
            }
        }
    }

    fn poll_for_settle(&mut self, context: Option<Context>, run_timers: bool) -> bool {
        match self.driver.poll_runtime_for_settle(context, run_timers) {
            Ok(work) => work,
            Err(error) => {
                self.driver.report_runtime_error(error);
                false
            }
        }
    }

    fn prepare_teardown(&mut self) {
        self.driver.prepare_teardown();
        self.driver.host_mut().teardown_page_script_host();
    }

    fn dispose(self: Box<Self>) -> Arc<blitz_dom::release::Teardown> {
        let Self { driver } = *self;
        driver.dispose()
    }

    fn set_page_visible(&mut self, visible: bool) {
        self.driver.set_page_visible(visible);
    }

    fn freeze(&mut self) {
        self.driver.freeze();
    }

    fn finish_restore(&mut self) {
        self.driver.finish_restore();
    }

    fn resume(&mut self) {
        self.driver.resume();
    }
}

impl blitz_control_protocol::ControlScriptBackend for PageDocument {
    fn dispatch_dom_event(&mut self, event: DomEvent) {
        self.driver.dispatch_dom_event(event);
    }

    fn handle_pointer_move_to_node(&mut self, event: BlitzPointerEvent, node: NodeId) {
        self.driver.handle_pointer_move_to_node(event, node);
    }

    fn handle_ui_event_to_node(&mut self, event: UiEvent, node: NodeId) {
        self.driver.handle_ui_event_to_node(event, node);
    }

    fn script_metrics(&self) -> Option<blitz_control_protocol::ScriptMetrics> {
        self.driver.script_metrics()
    }
}
