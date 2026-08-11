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

/// Write the tree rooted at the document root to `output`.
pub fn write_tree(document: &BaseDocument, output: &Path) -> std::io::Result<()> {
    let mut text = String::new();
    text.push_str("# depth tag#id.class  x,y w*h  display position [notes]\n");
    let root = document.root_node().id;
    walk(document, root, 0, 0.0, 0.0, &mut text);
    std::fs::write(output, text)
}

fn walk(
    document: &BaseDocument,
    node_id: usize,
    depth: usize,
    parent_x: f32,
    parent_y: f32,
    out: &mut String,
) {
    let Some(node) = document.get_node(node_id) else {
        return;
    };
    let layout = &node.final_layout;
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
