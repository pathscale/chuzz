//! A worker's module map is local to its own Boa context.
//! It is the module identity table required for cycles and diamond imports.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;


use blitz_script::boa_engine::module::{ModuleLoader, ModuleRequest, Referrer};
use blitz_script::boa_engine::{Context, JsNativeError, JsResult, JsString, Module, Source};
use blitz_traits::net::{Request, Url};

use crate::decode::decode_body;
use crate::document_loader::NetProvider;

use super::{DEADLINE, Endpoint, cancellable};

pub(super) struct Loader {
    net: Arc<NetProvider>,
    endpoint: Arc<Endpoint>,
    base: Url,
    modules: RefCell<HashMap<String, Module>>,
}

impl Loader {
    pub(super) fn new(net: Arc<NetProvider>, endpoint: Arc<Endpoint>, base: Url) -> Self {
        Self {
            net,
            endpoint,
            base,
            modules: RefCell::new(HashMap::new()),
        }
    }

    pub(super) fn source(&self, url: &Url) -> JsResult<(Url, String)> {
        if self.endpoint.stopped() {
            return Err(JsNativeError::error()
                .with_message("worker terminated")
                .into());
        }
        if !matches!(url.scheme(), "http" | "https" | "data" | "file") {
            return Err(JsNativeError::typ().with_message("unsupported script URL").into());
        }
        let response = nagoya::block_on(cancellable(
            &self.endpoint,
            nagoya::timeout(
                DEADLINE,
                self.net.fetch_response_async(Request::get(url.clone())),
            ),
        ))
        .map_err(|error| JsNativeError::error().with_message(error))?
        .map_err(|_| JsNativeError::error().with_message("worker script fetch timed out"))?
        .map_err(|error| JsNativeError::typ().with_message(format!("{error:?}")))?;
        if !response.status.is_success() {
            return Err(JsNativeError::typ()
                .with_message(format!("worker script {url} returned {}", response.status))
                .into());
        }
        Ok((response.url, decode_body(&response.body)))
    }

    pub(super) fn register(&self, url: &Url, module: Module) {
        self.modules.borrow_mut().insert(url.to_string(), module);
    }

    fn load(
        &self,
        referrer: &Referrer,
        request: &ModuleRequest,
        context: &mut Context,
    ) -> JsResult<Module> {
        if !request.attributes().is_empty() {
            return Err(JsNativeError::typ()
                .with_message("worker import attributes are unsupported")
                .into());
        }
        let specifier = request.specifier().to_std_string_escaped();
        let base = referrer
            .path()
            .and_then(Path::to_str)
            .and_then(|path| Url::parse(path).ok())
            .unwrap_or_else(|| self.base.clone());
        let url = if let Ok(url) = Url::parse(&specifier) {
            url
        } else {
            if !specifier.starts_with('/') && !specifier.starts_with('.') {
                return Err(JsNativeError::typ()
                    .with_message("bare specifiers have no worker import map")
                    .into());
            }
            base.join(&specifier)
                .map_err(|error| JsNativeError::typ().with_message(error.to_string()))?
        };
        if let Some(module) = self.modules.borrow().get(url.as_str()) {
            return Ok(module.clone());
        }
        let (resolved, source) = self.source(&url)?;
        if let Some(module) = self.modules.borrow().get(resolved.as_str()).cloned() {
            self.register(&url, module.clone());
            return Ok(module);
        }
        let path = resolved.to_string();
        let module = Module::parse(
            Source::from_bytes(source.as_bytes()).with_path(Path::new(&path)),
            None,
            context,
        )?;
        self.register(&url, module.clone());
        self.register(&resolved, module.clone());
        Ok(module)
    }
}

impl ModuleLoader for Loader {
    fn load_imported_module(
        self: Rc<Self>,
        referrer: Referrer,
        request: ModuleRequest,
        context: &RefCell<&mut Context>,
    ) -> impl Future<Output = JsResult<Module>> {
        let result = self.load(&referrer, &request, &mut context.borrow_mut());
        async { result }
    }

    fn init_import_meta(
        self: Rc<Self>,
        import_meta: &blitz_script::boa_engine::JsObject,
        module: &Module,
        context: &mut Context,
    ) {
        let url = module
            .path()
            .and_then(Path::to_str)
            .unwrap_or(self.base.as_str());
        let _ = import_meta.set(JsString::from("url"), JsString::from(url), false, context);
    }
}
