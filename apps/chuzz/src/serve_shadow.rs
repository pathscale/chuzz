//! Project inspector records onto the native flattened tree.

use std::collections::HashMap;

use blitz_control_protocol::{DebugResponse, SemanticNode};
use blitz_dom::{Document, Node, NodeId};

/// The protocol owns roles, labels and geometry. Reuse its records, asking
/// for missing shadow nodes by id, then emit only the current composition.
/// The caller supplies the same revision to every read.
pub(super) fn flatten<D: Document + ?Sized>(
    document: &mut D,
    response: DebugResponse,
    root: Option<u64>,
    max_depth: usize,
    mut inspect_node: impl FnMut(&mut D, u64) -> DebugResponse,
) -> DebugResponse {
    let DebugResponse::AgentSnapshot(mut snapshot) = response else {
        return response;
    };
    let traversal = {
        let inner = document.inner();
        if !inner.has_shadow_roots() {
            return DebugResponse::AgentSnapshot(snapshot);
        }
        let root_id = root.map(NodeId::from_u64).unwrap_or(inner.root_node().id);
        let mut traversal = Vec::new();
        let mut stack = vec![(root_id, None, 0usize, false)];
        while let Some((id, parent, depth, hidden)) = stack.pop() {
            let Some(node) = inner.get_node(id) else {
                continue;
            };
            let hidden = hidden
                || node.attr(blitz_dom::LocalName::from("aria-hidden")) == Some("true")
                || node.computed_style_properties().is_some_and(|properties| {
                    properties
                        .iter()
                        .any(|(name, value)| *name == "display" && value == "none")
                });
            let name = node.element_data().and_then(|element| {
                let named = element.attrs().iter().any(|attribute| {
                    matches!(
                        attribute.name.local.as_ref(),
                        "aria-label" | "aria-labelledby" | "title"
                    )
                });
                (!named && (node.shadow_root_id().is_some() || node.assigned_nodes.is_some()))
                    .then(|| flattened_text(node))
            });
            traversal.push((id.as_u64(), parent, hidden, name, node.is_element()));
            if max_depth == 0 || depth < max_depth {
                stack.extend(
                    node.layout_dom_children()
                        .iter()
                        .rev()
                        .map(|&child| (child, Some(id.as_u64()), depth + 1, hidden)),
                );
            }
        }
        traversal
    };

    let mut records: HashMap<u64, SemanticNode> = std::mem::take(&mut snapshot.nodes)
        .into_iter()
        .map(|node| (node.id, node))
        .collect();
    let mut parents = HashMap::new();
    for (id, parent, hidden, name, inspectable) in traversal {
        if inspectable
            && !records.contains_key(&id)
            && let DebugResponse::AgentSnapshot(extra) = inspect_node(document, id)
        {
            for node in extra.nodes {
                records.entry(node.id).or_insert(node);
            }
        }
        // Non-semantic containers remain transparent. Their children attach
        // to the closest ancestor that actually has a protocol record.
        let parent = parent.and_then(|parent| parents.get(&parent).copied().flatten());
        if let Some(mut node) = records.remove(&id) {
            node.parent = parent;
            node.visible &= !hidden;
            if let Some(name) = name {
                node.name = name;
            }
            parents.insert(id, Some(id));
            snapshot.nodes.push(node);
        } else {
            parents.insert(id, parent);
        }
    }
    DebugResponse::AgentSnapshot(snapshot)
}

fn flattened_text(root: &Node) -> String {
    let mut text = String::new();
    let mut stack: Vec<_> = root.layout_dom_children().iter().rev().copied().collect();
    while let Some(id) = stack.pop() {
        let node = root.with(id);
        if node.is_text_node() {
            text.push_str(&node.text_content());
        } else if !node.element_data().is_some_and(|element| {
            matches!(element.name.local.as_ref(), "style" | "script" | "template")
        }) {
            stack.extend(node.layout_dom_children().iter().rev().copied());
        }
    }
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
