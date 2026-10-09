//! Fetching the scripts a page asks for after it has started running.
//!
//! Both loading paths prefetch: they walk the parsed HTML, load every script it
//! names, and serve those from a map. That map cannot contain a script the page
//! builds for itself, because nobody knew the URL until the page was already
//! running. Such a request used to reach `DefaultScriptFetcher`, which serves
//! `file:` and `data:` only, so it was refused with `unsupported URL scheme for
//! script: https` and the script was dropped.
//!
//! Measured over a hundred-site corpus that was the most common single defect,
//! reaching a quarter of the sites. It hides well, because the scripts the
//! parser found load perfectly: a page fails only in the parts it assembles
//! itself.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use blitz_traits::net::{Request, Url};

use crate::decode::decode_body;
use crate::document_loader::NetProvider;

/// Serves prefetched sources, and fetches anything else from the network.
pub struct PageScripts {
    scripts: HashMap<Url, String>,
    net: Arc<NetProvider>,
    /// How long a script discovered by a running document may take before the
    /// page moves on.
    ///
    /// Bounded because this blocks script execution. A server that accepts the
    /// connection and never answers would otherwise hang the caller, and a
    /// missing script is a far better outcome than a page that never finishes.
    deadline: Duration,
}

impl PageScripts {
    pub fn new(scripts: HashMap<Url, String>, net: Arc<NetProvider>, deadline: Duration) -> Self {
        Self {
            scripts,
            net,
            deadline,
        }
    }

    fn fetch_over_network(&self, url: &Url) -> Result<String, blitz_script::FetchError> {
        let net = Arc::clone(&self.net);
        let target = url.clone();
        let deadline = self.deadline;
        let fetch =
            async move { nagoya::timeout(deadline, net.fetch_async(Request::get(target))).await };

        // Script fetching is synchronous, so drive the bounded networking
        // future on this caller while its socket operations use Nagoya.
        let joined = nagoya::block_on(fetch);

        match joined {
            Ok(Ok((_, bytes))) => Ok(decode_body(&bytes)),
            Ok(Err(error)) => Err(blitz_script::FetchError::InvalidData(format!("{error:?}"))),
            Err(_) => Err(blitz_script::FetchError::InvalidData(format!(
                "script fetch timed out after {}s",
                deadline.as_secs()
            ))),
        }
    }
}

impl blitz_script::ScriptFetcher for PageScripts {
    fn fetch(&self, url: &Url) -> Result<String, blitz_script::FetchError> {
        if let Some(source) = self.scripts.get(url) {
            return Ok(source.clone());
        }
        match url.scheme() {
            "http" | "https" => self.fetch_over_network(url),
            // `file:` and `data:` still belong to the default fetcher: it
            // decodes data URLs correctly and needs no network.
            _ => blitz_script::DefaultScriptFetcher.fetch(url),
        }
    }
}
