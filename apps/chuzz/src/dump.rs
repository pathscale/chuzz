//! A text dump of the laid-out tree, for comparing a capture against a real
//! browser without looking at pixels.
//!
//! `--capture` answers "what does it paint". This answers "why": every element
//! with its absolute box, its computed `display` and `position`, and the
//! attributes that decide those. The format is one line per node so it can be
//! grepped and diffed against `getBoundingClientRect` output from a reference
//! browser, which is the only way to tell a layout fault from a paint fault.

use std::fmt::Write as _;
use std::path::Path;

use blitz_dom::BaseDocument;
use blitz_dom::NodeId;

/// Write the tree rooted at the document root to `output`.
pub fn write_tree(document: &BaseDocument, output: &Path) -> std::io::Result<()> {
    let mut text = String::new();
    text.push_str("# depth tag#id.class  x,y w*h  display position [notes]\n");
    let root = document.root_node().id;
    walk(document, root, 0, 0.0, 0.0, &mut text);
    text.push('\n');
    text.push_str(&census(document));
    std::fs::write(output, text)
}

/// What the page costs to hold, counted rather than estimated.
///
/// Two ratios, both of which decide whether a storage optimisation is worth
/// writing before anyone writes it:
///
/// - **DOM nodes against nodes with a layout box.** Every DOM node carries a
///   `final_layout` whether or not anything lays it out. A large gap is the
///   argument for moving layout data off the node.
/// - **Distinct attribute values against total.** Pages repeat the same class
///   lists on hundreds of elements. A large gap is the argument for Blink's
///   `ShareableElementData`, where elements with identical attributes share one
///   allocation. A small gap says the sharing would buy nothing here and the
///   idea can be dropped rather than carried around.
fn census(document: &BaseDocument) -> String {
    use std::collections::HashSet;

    let mut nodes = 0usize;
    let mut elements = 0usize;
    let mut text_nodes = 0usize;
    let mut with_box = 0usize;
    let mut attributes = 0usize;
    let mut distinct_values: HashSet<String> = HashSet::new();
    let mut distinct_names: HashSet<String> = HashSet::new();
    // The whole class attribute, not each class: sharing works on the stored
    // string, and "flex items-center gap-2" repeated fifty times is one value
    // repeated fifty times.
    let mut distinct_class_lists: HashSet<String> = HashSet::new();
    let mut class_attributes = 0usize;

    let mut fanout: Vec<usize> = Vec::new();
    let mut stack = vec![document.root_node().id];
    while let Some(node_id) = stack.pop() {
        let Some(node) = document.get_node(node_id) else {
            continue;
        };
        fanout.push(node.children.len());
        nodes += 1;
        let layout = has_layout(node).then(|| *node.final_layout());
        let has_box = layout.is_some_and(|l| l.size.width > 0.0 || l.size.height > 0.0);
        match node.element_data() {
            Some(data) => {
                elements += 1;
                // Counted against elements, not against all nodes. Text nodes
                // are laid out by their inline formatting context and report
                // 0x0 by design, so counting them as "no box" turns a real
                // number into a flattering one.
                if has_box {
                    with_box += 1;
                }
                for attr in data.attrs.iter() {
                    attributes += 1;
                    distinct_names.insert(attr.name.local.to_string());
                    distinct_values.insert(attr.value.to_string());
                    if attr.name.local.as_ref() == "class" {
                        class_attributes += 1;
                        distinct_class_lists.insert(attr.value.to_string());
                    }
                }
            }
            None => text_nodes += 1,
        }
        stack.extend(node.children.iter().copied());
    }

    let share = |distinct: usize, total: usize| {
        if total == 0 {
            "n/a".to_owned()
        } else {
            format!("{:.0}%", 100.0 * distinct as f64 / total as f64)
        }
    };

    let leaves = fanout.iter().filter(|n| **n == 0).count();
    let ones = fanout.iter().filter(|n| **n == 1).count();
    let twos = fanout.iter().filter(|n| **n == 2).count();
    let max_fanout = fanout.iter().copied().max().unwrap_or(0);
    let total_children: usize = fanout.iter().sum();
    let mean = if nodes == 0 {
        0.0
    } else {
        total_children as f64 / nodes as f64
    };
    format!(
        "# fanout\n\
         #   leaves (0 kids) {leaves}  ({})\n\
         #   exactly 1       {ones}  ({})\n\
         #   exactly 2       {twos}  ({})\n\
         #   max             {max_fanout}\n\
         #   mean            {mean:.2}\n\
         # census\n\
         # nodes            {nodes}\n\
         #   elements       {elements}\n\
         #   text           {text_nodes}\n\
         #   with a box     {with_box}  ({} of elements)\n\
         # attributes       {attributes}\n\
         #   distinct names {}  ({})\n\
         #   distinct values {}  ({})\n\
         # class attributes {class_attributes}\n\
         #   distinct lists {}  ({})\n",
        share(leaves, nodes),
        share(ones, nodes),
        share(twos, nodes),
        share(with_box, elements),
        distinct_names.len(),
        share(distinct_names.len(), attributes),
        distinct_values.len(),
        share(distinct_values.len(), attributes),
        distinct_class_lists.len(),
        share(distinct_class_lists.len(), class_attributes),
    )
}

fn walk(
    document: &BaseDocument,
    node_id: NodeId,
    depth: usize,
    parent_x: f32,
    parent_y: f32,
    out: &mut String,
) {
    let Some(node) = document.get_node(node_id) else {
        return;
    };
    let layout = if has_layout(node) {
        *node.final_layout()
    } else {
        Default::default()
    };
    // Taffy stores each box relative to its parent's border box, so an absolute
    // position is only available by accumulating on the way down. Without that
    // the numbers cannot be compared against a browser's client rects.
    let x = parent_x + layout.location.x;
    let y = parent_y + layout.location.y;

    let element = node.element_data();
    let name = match element {
        Some(data) => {
            let mut label = data.name.local.to_string();
            let attr = |wanted: &str| {
                data.attrs
                    .iter()
                    .find(|attr| attr.name.local.as_ref() == wanted)
                    .map(|attr| attr.value.to_string())
            };
            if let Some(id) = attr("id") {
                let _ = write!(label, "#{id}");
            }
            if let Some(class) = attr("class") {
                let _ = write!(label, ".{}", class.replace(' ', "."));
            }
            label
        }
        None => {
            let text: String = node.text_content().chars().take(40).collect();
            format!("#text {:?}", text.trim())
        }
    };

    let (display, position) = match node.primary_styles() {
        Some(style) => (
            format!("{:?}", style.clone_display()),
            format!("{:?}", style.get_box().position),
        ),
        None => ("-".to_owned(), "-".to_owned()),
    };

    let indent = "  ".repeat(depth);
    let _ = writeln!(
        out,
        "{indent}[{node_id}] {name}  {x:.0},{y:.0} {w:.0}*{h:.0}  {display} {position}",
        w = layout.size.width,
        h = layout.size.height,
    );

    for child in &node.children {
        walk(document, *child, depth + 1, x, y, out);
    }
}

/// Whether this node kind carries a layout box.
///
/// `final_layout()` is a panicking accessor: only elements, anonymous blocks
/// and the document own one. It was a plain field before the node tree moved to
/// a SlotMap, so reading it unconditionally used to be safe and now is not.
fn has_layout(node: &blitz_dom::Node) -> bool {
    matches!(
        node.data.kind(),
        blitz_dom::node::NodeKind::Element
            | blitz_dom::node::NodeKind::AnonymousBlock
            | blitz_dom::node::NodeKind::Document
    )
}
