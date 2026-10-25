//! Concrete page runtime adapter for the native Ekojit document.
//!
//! The document owns the guest realm. The native host is shared with the
//! owner-local bridge so every intrinsic callback reaches this page's host.

use std::cell::{Ref, RefCell, RefMut};
use std::collections::BTreeMap;
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
use crate::script_host::{HostFrame, HostFrameSource};

/// The engine-neutral page driver backed by one persistent native realm.
pub(super) struct PageNativeDriver {
    document: EkojitDocument,
    host: Rc<RefCell<ChuzzPageRuntimeHost>>,
    pending_frames: Rc<RefCell<Vec<HostFrame>>>,
    frame_revision: u64,
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
    let pending_frames = Rc::new(RefCell::new(Vec::new()));
    let mut host = host;
    let frame_queue = Rc::clone(&pending_frames);
    host.set_frame_sink(move |frame| {
        frame_queue.borrow_mut().push(frame);
        Ok(())
    })
    .map_err(|error| format!("could not install native page frame sink: {error}"))?;
    let host = Rc::new(RefCell::new(host));
    let mut document = EkojitDocument::from_html(html, config, NativePageDefect::None)
        .map_err(|error| error.to_string())?;
    crate::page_ekojit_bridge::install(&mut document, Rc::clone(&host))?;
    crate::page_ekojit_bootstrap::install(&mut document)?;
    crate::page_ekojit_compat::install(&mut document)?;
    install_native_canvas_handles(&mut document)?;
    document.set_script_fetcher(fetcher);
    Ok(Box::new(PageNativeDriver {
        document,
        host,
        pending_frames,
        frame_revision: 0,
        #[cfg(feature = "gui")]
        poll_hooks: Vec::new(),
        #[cfg(feature = "gui")]
        deferred_scripts: Vec::new(),
    }))
}

/// Install the native canvas surface before author scripts run.
///
/// The bootstrap factory contract is
/// `globalThis.__chuzzCreateCanvasContext(canvas, kind, options)`. `canvas` is
/// the actual DOM element, `kind` is the requested context name, and `options`
/// is the original options argument. The numeric handle below is a
/// page-local canvas transport handle. The private DOM marker below resolves
/// it back to the actual connected canvas node before a raster is installed.
fn install_native_canvas_handles(document: &mut EkojitDocument) -> Result<(), String> {
    const SOURCE: &str = r#"(function (g) {
  'use strict';
  var doc = g.document;
  if (!doc || doc.__chuzzCanvasHandlesInstalled) { return; }
  var next = 1;
  function installDimension(canvas, name, defaultValue) {
    if (typeof canvas[name] === 'number') { return; }
    var assigned;
    var hasAssigned = false;
    function current() {
      if (typeof canvas.getAttribute === 'function') {
        var raw = canvas.getAttribute(name);
        if (raw != null && String(raw).trim() !== '') {
          var value = Number(raw);
          if (Number.isFinite(value) && value >= 0) { return Math.min(4294967295, Math.floor(value)); }
          return defaultValue;
        }
        if (raw != null) { return defaultValue; }
      }
      return hasAssigned ? assigned : defaultValue;
    }
    function set(value) {
      value = Number(value);
      var numeric = Number.isFinite(value) ? (Math.trunc(value) >>> 0) : 0;
      if (typeof canvas.setAttribute === 'function') {
        try {
          canvas.setAttribute(name, String(numeric));
          hasAssigned = false;
          return;
        } catch (_) {}
      }
      assigned = numeric;
      hasAssigned = true;
    }
    try {
      Object.defineProperty(canvas, name, {
        configurable: true,
        enumerable: true,
        get: current,
        set: set
      });
    } catch (_) {
      try { canvas[name] = current(); } catch (_) {}
    }
  }
  function attach(canvas) {
    if (!canvas || String(canvas.tagName || canvas.nodeName || '').toLowerCase() !== 'canvas') { return canvas; }
    if (canvas.__chuzzCanvasHandlesInstalled) { return canvas; }
    canvas.__chuzzCanvasHandlesInstalled = true;
    canvas.__chuzzCanvasHandle = next++;
    if (typeof canvas.setAttribute === 'function') {
      canvas.setAttribute('data-chuzz-canvas-handle', String(canvas.__chuzzCanvasHandle));
    }
    installDimension(canvas, 'width', 300);
    installDimension(canvas, 'height', 150);
    var nativeCloneNode = typeof canvas.cloneNode === 'function' ? canvas.cloneNode : null;
    if (nativeCloneNode) {
      canvas.cloneNode = function () {
        return attach(nativeCloneNode.apply(this, arguments));
      };
    }
    var canvasPrototype = g.HTMLCanvasElement && g.HTMLCanvasElement.prototype;
    var nativeGetContext = canvas.__chuzzCanvasNativeGetContext ||
      (canvasPrototype && canvasPrototype.__chuzzNativeGetContext) ||
      (typeof canvas.getContext === 'function' ? canvas.getContext : null);
    function trackWebGpuContext(context) {
      if (!context || context.__chuzzPageFrameTracking) { return context; }
      context.__chuzzPageFrameTracking = true;
      var configure = typeof context.configure === 'function' ? context.configure : null;
      var unconfigure = typeof context.unconfigure === 'function' ? context.unconfigure : null;
      if (configure) {
        context.configure = function () {
          var result = configure.apply(this, arguments);
          if (typeof canvas.setAttribute === 'function') {
            canvas.setAttribute('data-chuzz-gpu-generation', String(this.__chuzzGeneration));
            canvas.setAttribute('data-chuzz-gpu-active', this.__chuzzDevice ? '1' : '0');
          }
          return result;
        };
      }
      if (unconfigure) {
        context.unconfigure = function () {
          var result = unconfigure.apply(this, arguments);
          if (typeof canvas.setAttribute === 'function') {
            canvas.setAttribute('data-chuzz-gpu-generation', String(Number(this.__chuzzGeneration) + 1));
            canvas.setAttribute('data-chuzz-gpu-active', '0');
          }
          return result;
        };
      }
      return context;
    }
    var getContext = function (kind, options) {
      var context = nativeGetContext ? nativeGetContext.apply(this, arguments) : null;
      if (context == null) {
        context = typeof g.__chuzzCreateCanvasContext === 'function'
          ? g.__chuzzCreateCanvasContext(this, kind, options)
          : null;
      }
      return String(kind).toLowerCase() === 'webgpu' ? trackWebGpuContext(context) : context;
    };
    try {
      Object.defineProperty(canvas, 'getContext', {
        configurable: true,
        writable: true,
        enumerable: true,
        value: getContext
      });
    } catch (_) {
      try { canvas.getContext = getContext; } catch (_) {}
    }
    return canvas;
  }
  function installLazyPrototypeAccessors() {
    var prototype = g.HTMLCanvasElement && g.HTMLCanvasElement.prototype;
    // Native host interfaces can expose methods on instances instead of the
    // prototype. Those instances are attached below and by createElement.
    if (!prototype || typeof prototype.getContext !== 'function' || prototype.__chuzzCanvasLazyAccessorsInstalled) { return; }
    var nativeGetContext = prototype.getContext;
    if (typeof nativeGetContext === 'function') {
      prototype.__chuzzNativeGetContext = nativeGetContext;
      var lazyGetContext = function () {
        attach(this);
        var own = Object.getOwnPropertyDescriptor(this, 'getContext');
        var target = own && typeof own.value === 'function' && own.value !== lazyGetContext
          ? own.value : nativeGetContext;
        return target.apply(this, arguments);
      };
      prototype.getContext = lazyGetContext;
    }
    var names = ['width', 'height'];
    for (var i = 0; i < names.length; i++) {
      var name = names[i], descriptor = Object.getOwnPropertyDescriptor(prototype, name);
      if (!descriptor || typeof descriptor.get !== 'function' || typeof descriptor.set !== 'function') { continue; }
      (function (property, nativeDescriptor) {
        Object.defineProperty(prototype, property, {
          configurable: nativeDescriptor.configurable,
          enumerable: nativeDescriptor.enumerable,
          get: function () {
            var value = nativeDescriptor.get.call(this);
            attach(this);
            return value;
          },
          set: function (value) {
            nativeDescriptor.set.call(this, value);
            attach(this);
          }
        });
      })(name, descriptor);
    }
    prototype.__chuzzCanvasLazyAccessorsInstalled = true;
  }
  installLazyPrototypeAccessors();
  var canvases = typeof doc.getElementsByTagName === 'function' ? doc.getElementsByTagName('canvas') :
    (typeof doc.querySelectorAll === 'function' ? doc.querySelectorAll('canvas') : []);
  for (var index = 0; index < canvases.length; index++) { attach(canvases[index]); }
  if (typeof doc.createElement === 'function') {
    var createElement = doc.createElement;
    doc.createElement = function (name) {
      var element = createElement.apply(this, arguments);
      return String(name).toLowerCase() === 'canvas' ? attach(element) : element;
    };
  }
  doc.__chuzzCanvasHandlesInstalled = true;
})(globalThis);"#;
    document
        .eval_source(Arc::new(PageSource {
            text: Arc::from(SOURCE),
            url: Arc::from("chuzz:page-bootstrap/webgpu-canvas-handles"),
        }))
        .map(|_| ())
        .map_err(|error| format!("could not install native canvas handles: {error}"))
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

    /// Move frames produced by host polling into Blitz's replaced-element
    /// image path. Handles are resolved through the page-private marker and
    /// checked against the current connected node and WebGPU generation.
    fn drain_native_frames(&mut self) -> bool {
        let mut frames = std::mem::take(&mut *self.pending_frames.borrow_mut());
        let (host_frames, torn_down) = {
            let mut host = self.page_host_mut();
            let torn_down = host.is_torn_down();
            (host.drain_frames(), torn_down)
        };
        frames.extend(host_frames);
        if torn_down {
            return false;
        }

        // A canvas can be updated several times during one poll. Keep only
        // the latest frame for each native source/target pair.
        let mut latest = BTreeMap::new();
        for frame in frames {
            latest.insert((frame.source, frame.target), frame);
        }

        let mut changed = false;
        for frame in latest.into_values() {
            changed |= self.install_native_frame(frame);
        }
        changed
    }

    fn install_native_frame(&mut self, frame: HostFrame) -> bool {
        if !matches!(
            frame.source,
            HostFrameSource::Canvas | HostFrameSource::WebGpu
        ) {
            return false;
        }
        let selector = format!("canvas[data-chuzz-canvas-handle=\"{}\"]", frame.target);
        let mut inner = self.document.inner_mut();
        let Ok(nodes) = inner.query_selector_all(&selector) else {
            return false;
        };
        if nodes.len() != 1 {
            return false;
        }
        let node_id = nodes[0];
        let Some(node) = inner.get_node(node_id) else {
            return false;
        };
        if !node.flags.is_in_document() {
            return false;
        }
        let Some(element) = node.element_data() else {
            return false;
        };
        if element.name.local.as_ref() != "canvas"
            || element
                .attr(blitz_dom::LocalName::from("data-chuzz-canvas-handle"))
                .and_then(|value| value.parse::<u64>().ok())
                != Some(frame.target)
        {
            return false;
        }

        if frame.source == HostFrameSource::WebGpu {
            let current_generation = element
                .attr(blitz_dom::LocalName::from("data-chuzz-gpu-generation"))
                .and_then(|value| value.parse::<u64>().ok());
            let active =
                element.attr(blitz_dom::LocalName::from("data-chuzz-gpu-active")) == Some("1");
            if current_generation != Some(frame.generation) || (frame.image.is_some() && !active) {
                return false;
            }
        } else if frame.generation != 0 {
            return false;
        }

        let Some(node) = inner.get_node_mut(node_id) else {
            return false;
        };
        let Some(element) = node.element_data_mut() else {
            return false;
        };
        element.special_data = match frame.image {
            Some(image) => blitz_dom::node::SpecialElementData::Image(Box::new(
                blitz_dom::node::ImageData::Raster(image.as_ref().clone()),
            )),
            None => blitz_dom::node::SpecialElementData::None,
        };

        self.frame_revision = self.frame_revision.wrapping_add(1);
        inner.mutate().set_attribute(
            node_id,
            blitz_dom::QualName::new(
                None,
                blitz_dom::Namespace::from(""),
                blitz_dom::LocalName::from("data-chuzz-raster-revision"),
            ),
            &self.frame_revision.to_string(),
        );
        true
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
        work |= self.drain_native_frames();
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
        work |= self.drain_native_frames();
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
