//! Fetching a URL and turning the bytes into a renderable page document.
//!
//! Each tab owns one loader. A loader holds at most one in-flight request: a
//! new navigation aborts the previous one, so a slow page cannot land after
//! the user has already moved on.

use std::sync::{Arc, Mutex};

use blitz_dom::{DocumentConfig, FontContext};
use blitz_html::{HtmlDocument, HtmlProvider};
use blitz_traits::net::{AbortController, AbortSignal, Request};
use blitz_traits::shell::ShellProvider;
use dioxus_native::{SubDocumentAttr, prelude::*};

use crate::history::{History, SyncStore, TabNavProvider};

pub type NetProvider = blitz_net::Provider;

/// Shown when a page cannot be fetched at all (DNS failure, refused
/// connection, TLS error). The message node is filled in from the real error.
const ERROR_HTML: &str = r#"<!doctype html>
<html>
  <head>
    <meta charset="utf-8">
    <title>Page not available</title>
    <style>
      body { font: 16px/1.5 system-ui, sans-serif; margin: 0; padding: 12vh 10vw; color: #202124; background: #fff; }
      h1 { font-size: 22px; font-weight: 600; margin: 0 0 12px; }
      p { margin: 0 0 8px; color: #5f6368; }
      #error { font-family: ui-monospace, monospace; font-size: 13px; color: #b3261e; word-break: break-word; }
    </style>
  </head>
  <body>
    <h1>This page could not be loaded</h1>
    <p>Chuzz could not reach the site.</p>
    <p id="error"></p>
  </body>
</html>
"#;

/// Shown when the server answered but sent nothing back.
const EMPTY_HTML: &str = r#"<!doctype html>
<html>
  <head><meta charset="utf-8"><title>Empty response</title></head>
  <body style="font: 16px/1.5 system-ui, sans-serif; margin: 0; padding: 12vh 10vw; color: #5f6368;">
    <h1 style="font-size: 22px; color: #202124;">Empty response</h1>
    <p>The server returned no content for this address.</p>
  </body>
</html>
"#;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LoadStatus {
    Loading,
    Idle,
}

/// A page that finished loading and is ready to be swapped into its tab.
#[derive(Clone)]
pub struct LoadedDocument {
    pub document: SubDocumentAttr,
    pub title: String,
}

pub struct DocumentLoader {
    pub font_ctx: FontContext,
    pub net_provider: Arc<NetProvider>,
    pub status: Signal<LoadStatus>,
    pub history: SyncStore<History>,
    /// Bumped by `reload`. The tab's load resource reads it, so reloading the
    /// same URL still re-runs the fetch.
    pub reload_generation: Signal<u64>,
    current_abort: Mutex<Option<AbortController>>,
}

impl DocumentLoader {
    pub fn new(net_provider: Arc<NetProvider>, history: SyncStore<History>) -> Self {
        Self {
            font_ctx: FontContext::default(),
            net_provider,
            status: Signal::new(LoadStatus::Idle),
            history,
            reload_generation: Signal::new(0),
            current_abort: Mutex::new(None),
        }
    }

    pub fn reload(&self) {
        self.abort_current();
        let mut generation = self.reload_generation;
        *generation.write() += 1;
    }

    pub fn reload_generation(&self) -> u64 {
        *self.reload_generation.read()
    }

    pub fn abort_current(&self) {
        // A poisoned lock here would mean a panic while swapping abort
        // handles; dropping the stale handle is still the right recovery.
        if let Ok(mut slot) = self.current_abort.lock()
            && let Some(controller) = slot.take()
        {
            controller.abort();
        }
    }

    fn install_abort(&self) -> AbortSignal {
        let controller = AbortController::default();
        let signal = controller.signal.clone();
        if let Ok(mut slot) = self.current_abort.lock() {
            if let Some(previous) = slot.take() {
                previous.abort();
            }
            *slot = Some(controller);
        }
        signal
    }

    fn doc_config(&self, base_url: Option<String>, signal: AbortSignal) -> DocumentConfig {
        DocumentConfig {
            base_url,
            net_provider: Some(Arc::clone(&self.net_provider) as _),
            navigation_provider: Some(Arc::new(TabNavProvider {
                history: self.history,
            })),
            shell_provider: Some(consume_context::<Arc<dyn ShellProvider>>()),
            html_parser_provider: Some(Arc::new(HtmlProvider)),
            font_ctx: Some(self.font_ctx.clone()),
            abort_signal: Some(signal),
            ..Default::default()
        }
    }

    pub async fn load(&self, req: Request) -> LoadedDocument {
        let signal = self.install_abort();
        let response = self
            .net_provider
            .fetch_async(req.signal(signal.clone()))
            .await;

        match response {
            Ok((resolved_url, bytes)) => {
                let html = if bytes.is_empty() {
                    EMPTY_HTML.to_string()
                } else {
                    String::from_utf8_lossy(&bytes).into_owned()
                };
                let config = self.doc_config(Some(resolved_url), signal);
                let document = HtmlDocument::from_html(&html, config).into_inner();
                let title = document
                    .find_title_node()
                    .map(|node| node.text_content())
                    .unwrap_or_default();
                LoadedDocument {
                    document: SubDocumentAttr::new(document),
                    title,
                }
            }
            Err(error) => {
                let config = self.doc_config(None, signal);
                let mut document = HtmlDocument::from_html(ERROR_HTML, config).into_inner();
                if let Some(text_node) = document
                    .get_element_by_id("error")
                    .and_then(|id| document.get_node(id))
                    .and_then(|node| node.children.first().copied())
                {
                    document
                        .mutate()
                        .set_node_text(text_node, &format!("{error:?}"));
                }
                LoadedDocument {
                    document: SubDocumentAttr::new(document),
                    title: String::from("Page not available"),
                }
            }
        }
    }
}

impl Drop for DocumentLoader {
    fn drop(&mut self) {
        self.abort_current();
    }
}
