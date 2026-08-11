//! Wiring the control socket to the live document.
//!
//! The socket thread cannot touch the DOM: a document is not `Send`, and the
//! renderer owns it. Requests therefore arrive on a channel, are drained on the
//! UI thread where the document lives, and the answer is sent back down a
//! oneshot the caller is already awaiting.

use std::cell::RefCell;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use blitz_dom::{BaseDocument, NodeId};
use chuzz_control::server::{ControlBridge, ControlServer};
use chuzz_control::{
    AgentControlRequest, AgentSnapshot, ControlError, ControlResponse, ImageStatus, SemanticNode,
};
use tokio::sync::oneshot;

/// A request paired with the channel its answer belongs on.
pub struct PendingRequest {
    pub request: AgentControlRequest,
    pub reply: oneshot::Sender<ControlResponse>,
}

/// Owns the socket and the queue feeding the UI thread.
pub struct ControlHandle {
    // Kept alive: dropping it unbinds the socket and removes the descriptor.
    _server: ControlServer,
    pending: Receiver<PendingRequest>,
    /// Requests taken off the channel but not yet answered. Held so a caller
    /// can ask whether work is waiting without consuming it.
    queued: RefCell<Vec<PendingRequest>>,
}

impl ControlHandle {
    /// Start the socket. Returns `None` when it cannot bind, which is not fatal:
    /// a browser without a control socket is still a browser.
    pub fn start() -> Option<Self> {
        let (sender, pending) = channel::<PendingRequest>();
        let sender: Arc<Sender<PendingRequest>> = Arc::new(sender);

        let bridge: ControlBridge = Arc::new(move |request| {
            let (reply, receiver) = oneshot::channel();
            // A send failure means the UI thread is gone; dropping `reply`
            // resolves the caller's receiver with an error rather than hanging.
            let _ = sender.send(PendingRequest { request, reply });
            receiver
        });

        match ControlServer::start(bridge) {
            Ok(server) => Some(Self {
                _server: server,
                pending,
                queued: RefCell::new(Vec::new()),
            }),
            Err(error) => {
                eprintln!("chuzz: control socket unavailable: {error}");
                None
            }
        }
    }

    /// Whether anything is waiting, so the poll loop can skip the borrow
    /// entirely on an idle browser.
    pub fn has_pending(&self) -> bool {
        !self.queued.borrow().is_empty() || self.refill()
    }

    /// Answer every queued request with an explicit error, for when the page
    /// document cannot be reached. Silence would look like a hang to a client
    /// that is waiting on a reply.
    pub fn drain_unavailable(&self) {
        self.refill();
        for PendingRequest { request, reply } in self.queued.borrow_mut().drain(..) {
            let response = match request {
                AgentControlRequest::Quit => ControlResponse::Ok,
                _ => ControlResponse::Error(ControlError::new(
                    "no_document",
                    "the active tab has no mounted document to inspect",
                )),
            };
            let _ = reply.send(response);
        }
    }

    /// Move anything the socket thread has sent into the local queue.
    fn refill(&self) -> bool {
        let mut queued = self.queued.borrow_mut();
        while let Ok(pending) = self.pending.try_recv() {
            queued.push(pending);
        }
        !queued.is_empty()
    }

    /// Answer everything queued since the last call. Runs on the UI thread.
    pub fn service(&self, document: &BaseDocument, url: Option<String>, title: Option<String>) {
        self.refill();
        for PendingRequest { request, reply } in self.queued.borrow_mut().drain(..) {
            let response = match request {
                AgentControlRequest::Inspect { root, max_depth } => ControlResponse::Snapshot(
                    snapshot(document, root, max_depth, url.clone(), title.clone()),
                ),
                AgentControlRequest::Act(_) => ControlResponse::Error(ControlError::new(
                    "unimplemented",
                    "act is not wired to the document yet",
                )),
                AgentControlRequest::Relaunch => ControlResponse::Error(ControlError::new(
                    "unimplemented",
                    "relaunch is not supported",
                )),
                AgentControlRequest::Quit => ControlResponse::Ok,
            };
            // The client may have disconnected while we were working.
            let _ = reply.send(response);
        }
    }
}

/// Walk the document into a flat list of nodes with their layout boxes.
///
/// Flat rather than nested because every node carries its parent: a client can
/// rebuild the tree, and a flat list survives truncation at `max_depth` without
/// producing a malformed document.
fn snapshot(
    document: &BaseDocument,
    root: Option<u64>,
    max_depth: u32,
    url: Option<String>,
    title: Option<String>,
) -> AgentSnapshot {
    let root_id = root
        .map(NodeId::from_u64)
        .unwrap_or(document.root_node().id);
    let mut nodes = Vec::new();
    collect(document, root_id, None, 0, max_depth, &mut nodes);

    AgentSnapshot {
        revision: 0,
        url,
        title,
        focused_node: document.get_focussed_node_id().map(|id| id.as_u64()),
        nodes,
    }
}

fn collect(
    document: &BaseDocument,
    node_id: NodeId,
    parent: Option<u64>,
    depth: u32,
    max_depth: u32,
    out: &mut Vec<SemanticNode>,
) {
    if depth > max_depth {
        return;
    }
    let Some(node) = document.get_node(node_id) else {
        return;
    };

    let element = node.element_data();
    let role = element
        .map(|data| data.name.local.to_string())
        .unwrap_or_else(|| "#text".to_owned());

    // Both elements and text nodes report their text content; the difference
    // between them is carried by `role`.
    let name = node.text_content();

    let layout = node.final_layout();
    let bounds = Some([
        layout.location.x as f64,
        layout.location.y as f64,
        layout.size.width as f64,
        layout.size.height as f64,
    ]);

    // Report what an image slot holds: a laid-out element with no decoded data
    // looks identical on screen to one whose file never arrived.
    let image = element.and_then(|data| {
        data.image_data().map(|image| match image {
            blitz_dom::node::ImageData::Raster(raster) => ImageStatus {
                kind: "raster".to_owned(),
                intrinsic: Some([raster.width, raster.height]),
            },
            // No `cfg` here: `svg` is a feature of blitz-dom, not of chuzz, so
            // gating on it silently compiled this arm out and reported every
            // parsed SVG as an empty slot.
            blitz_dom::node::ImageData::Svg(svg) => ImageStatus {
                kind: "svg".to_owned(),
                intrinsic: Some([
                    svg.tree.size().width().round() as u32,
                    svg.tree.size().height().round() as u32,
                ]),
            },
            blitz_dom::node::ImageData::None => ImageStatus {
                kind: "none".to_owned(),
                intrinsic: None,
            },
        })
    });

    let namespace = element.map(|data| data.name.ns.to_string());

    // The resolved `src`, so a client can tell a missing asset from a decode
    // failure without guessing at the URL.
    let value = element.and_then(|data| {
        data.attrs
            .iter()
            .find(|attr| attr.name.local.as_ref() == "src")
            .map(|attr| attr.value.to_string())
    });

    out.push(SemanticNode {
        id: node_id.as_u64(),
        parent,
        role,
        namespace,
        image,
        name: name.chars().take(200).collect(),
        value,
        enabled: true,
        // A zero-area box paints nothing, which is exactly what "invisible"
        // means to a client trying to click it.
        visible: layout.size.width > 0.0 && layout.size.height > 0.0,
        bounds,
    });

    for child in &node.children {
        collect(
            document,
            *child,
            Some(node_id.as_u64()),
            depth + 1,
            max_depth,
            out,
        );
    }
}
