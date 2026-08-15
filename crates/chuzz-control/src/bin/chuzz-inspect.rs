//! Read and drive a running chuzz window from the command line.
//!
//! ```text
//! cargo run -p chuzz-control --bin chuzz-inspect -- tree
//! cargo run -p chuzz-control --bin chuzz-inspect -- find settings
//! cargo run -p chuzz-control --bin chuzz-inspect -- click 4294967331
//! cargo run -p chuzz-control --bin chuzz-inspect -- overlap 4294967774 12884902369
//! cargo run -p chuzz-control --bin chuzz-inspect -- press 1362 28
//! ```
//!
//! This machine grants no screen recording and no assistive access, so nothing
//! in the window can be confirmed by looking at it. That is not an
//! inconvenience to route around with screenshots: it is the reason a stderr
//! line saying a document had been built was once taken as proof that it
//! painted, and two bugs reached the user on the strength of it. Reading the
//! boxes back out of the live document is a stronger claim than a screenshot
//! anyway — a control the wrong size, or a title overlapping the button beside
//! it, is a number here rather than a judgement call.
//!
//! Start the browser with the plane on:
//!
//! ```text
//! CHUZZ_CONTROL=1 target/release/chuzz-gui
//! ```

use std::collections::BTreeMap;

use chuzz_control::client::{Client, bounds, newest_descriptor, overlaps};
use serde_json::Value;

const USAGE: &str = "\
chuzz-inspect — read and drive a running chuzz window

    tree                       the semantic tree, indented, with boxes
    nodes                      the same nodes, one per line, unindented
    find <needle>              nodes whose role or name contains <needle>
    click <node-id>            a synthesised pointer press on a node's centre
    press <x> <y>              a pointer press at window coordinates
    set-value <node-id> <text> replace a text input's contents
    type <text>                type into the focused field, one key at a time
    key <key>                  one key, down then up (Enter, Escape, Backspace)
    overlap <node-a> <node-b>  whether two boxes intersect
    raw <json>                 one request, verbatim, answer printed as JSON

Options:
    --descriptor <path>        a specific control descriptor (default: newest)
    --depth <n>                tree depth to request (default: 40)
    --root <node-id>           subtree to report (default: the document)
    --all                      keep zero-sized and hidden nodes (default: drop)
";

struct Options {
    descriptor: Option<String>,
    depth: u32,
    root: Option<u64>,
    all: bool,
    command: Vec<String>,
}

fn parse_options() -> Result<Options, String> {
    let mut options = Options {
        descriptor: None,
        depth: 40,
        root: None,
        all: false,
        command: Vec::new(),
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = |name: &str| args.next().ok_or_else(|| format!("{name} needs a value"));
        match arg.as_str() {
            "--descriptor" => options.descriptor = Some(value("--descriptor")?),
            "--depth" => {
                options.depth = value("--depth")?
                    .parse()
                    .map_err(|_| "--depth takes a number".to_owned())?
            }
            "--root" => {
                options.root = Some(
                    value("--root")?
                        .parse()
                        .map_err(|_| "--root takes a node id".to_owned())?,
                )
            }
            "--all" => options.all = true,
            "-h" | "--help" => return Err(USAGE.to_owned()),
            _ => options.command.push(arg),
        }
    }
    if options.command.is_empty() {
        return Err(USAGE.to_owned());
    }
    Ok(options)
}

/// Two decimals, trailing zeros dropped.
///
/// The runtime measures in `f32` and widens, so a box that is exactly 38.88
/// arrives as 38.880001. Printing the full expansion buries the number that
/// matters in noise that means nothing.
fn round(value: f64) -> String {
    let text = format!("{value:.2}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text == "-0" { "0" } else { text }.to_owned()
}

/// A node's box, or that it has none.
fn box_of(node: &Value) -> String {
    match bounds(node) {
        Some([x, y, width, height]) => format!(
            "{}x{} at {},{}",
            round(width),
            round(height),
            round(x),
            round(y)
        ),
        None => "no box".to_owned(),
    }
}

fn describe(node: &Value) -> String {
    let id = node["id"].as_u64().unwrap_or_default();
    let role = node["role"].as_str().unwrap_or("?");
    let name = node["name"].as_str().unwrap_or_default();
    let name = if name.chars().count() > 60 {
        format!("{}...", name.chars().take(57).collect::<String>())
    } else {
        name.to_owned()
    };
    let named = if name.is_empty() {
        String::new()
    } else {
        format!(" {name:?}")
    };
    let mut flags = String::new();
    if node["visible"] == Value::Bool(false) {
        flags.push_str(" hidden");
    }
    if node["enabled"] == Value::Bool(false) {
        flags.push_str(" disabled");
    }
    if node["selected"] == Value::Bool(true) {
        flags.push_str(" selected");
    }
    format!("#{id} {role}{named} [{}]{flags}", box_of(node))
}

/// Whether a node is worth printing.
///
/// A snapshot is mostly `0x0 at 0,0` nodes: `@pathscale/ui` mounts portals,
/// dialogs and menus up front and hides them, so the interesting twenty lines
/// sit inside three hundred. `--all` keeps them for the case where the question
/// is why something that should be on screen is not.
fn is_interesting(node: &Value) -> bool {
    bounds(node).is_some_and(|[_, _, width, height]| width > 0.0 && height > 0.0)
}

fn print_tree(nodes: &[Value], all: bool) {
    let mut children: BTreeMap<u64, Vec<&Value>> = BTreeMap::new();
    let mut roots: Vec<&Value> = Vec::new();
    let known: std::collections::HashSet<u64> = nodes
        .iter()
        .filter_map(|node| node["id"].as_u64())
        .collect();
    for node in nodes {
        match node["parent"].as_u64() {
            Some(parent) if known.contains(&parent) => {
                children.entry(parent).or_default().push(node)
            }
            _ => roots.push(node),
        }
    }

    fn walk(node: &Value, indent: usize, children: &BTreeMap<u64, Vec<&Value>>, all: bool) {
        let show = all || is_interesting(node);
        if show {
            println!("{}{}", "  ".repeat(indent), describe(node));
        }
        let id = node["id"].as_u64().unwrap_or_default();
        for child in children.get(&id).into_iter().flatten() {
            // A hidden container is skipped but its children are not
            // re-indented away: an element that is on screen inside an
            // ancestor reported as hidden is exactly the kind of thing worth
            // seeing.
            walk(child, if show { indent + 1 } else { indent }, children, all);
        }
    }

    for root in roots {
        walk(root, 0, &children, all);
    }
}

async fn run(options: Options) -> Result<(), Box<dyn std::error::Error>> {
    let descriptor = match &options.descriptor {
        Some(path) => std::path::PathBuf::from(path),
        None => newest_descriptor()?,
    };
    let mut client = Client::connect(&descriptor).await?;

    let command = options.command[0].as_str();
    let argument = |index: usize| -> Result<&str, String> {
        options
            .command
            .get(index)
            .map(String::as_str)
            .ok_or_else(|| format!("{command} needs {index} argument(s)"))
    };

    match command {
        "raw" => {
            let request: Value = serde_json::from_str(argument(1)?)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&client.call(request).await?)?
            );
        }
        "click" => {
            let node_id: u64 = argument(1)?.parse()?;
            println!(
                "{}",
                serde_json::to_string_pretty(&client.click(node_id).await?)?
            );
        }
        "set-value" => {
            let node_id: u64 = argument(1)?.parse()?;
            client.set_value(node_id, argument(2)?).await?;
            println!("set #{node_id}");
        }
        "type" => {
            client.type_text(argument(1)?).await?;
            println!("typed {:?}", argument(1)?);
        }
        "key" => {
            let key = argument(1)?;
            // The code is the physical key; for the ones worth typing from a
            // command line it is the name with `Key`/nothing in front, and
            // guessing beats making every caller pass both.
            let code = match key {
                "Enter" => "Enter".to_owned(),
                "Escape" => "Escape".to_owned(),
                "Tab" => "Tab".to_owned(),
                other if other.len() == 1 => format!("Key{}", other.to_uppercase()),
                other => other.to_owned(),
            };
            client.key(key, &code).await?;
            println!("pressed {key}");
        }
        "press" => {
            let x: f64 = argument(1)?.parse()?;
            let y: f64 = argument(2)?.parse()?;
            client.press(x, y).await?;
            println!("pressed at {x},{y}");
        }
        "tree" | "nodes" | "find" | "overlap" => {
            let page = client.inspect(options.root, options.depth).await?;
            let nodes = page["nodes"].as_array().cloned().unwrap_or_default();
            match command {
                "tree" => {
                    println!("nodes={} focused={}", nodes.len(), page["focusedNode"]);
                    print_tree(&nodes, options.all);
                }
                "nodes" => {
                    for node in nodes
                        .iter()
                        .filter(|node| options.all || is_interesting(node))
                    {
                        println!("{}", describe(node));
                    }
                }
                "find" => {
                    let needle = argument(1)?.to_lowercase();
                    for node in &nodes {
                        let haystack = format!(
                            "{} {}",
                            node["role"].as_str().unwrap_or_default(),
                            node["name"].as_str().unwrap_or_default()
                        )
                        .to_lowercase();
                        if haystack.contains(&needle) {
                            println!("{}", describe(node));
                        }
                    }
                }
                "overlap" => {
                    let first: u64 = argument(1)?.parse()?;
                    let second: u64 = argument(2)?.parse()?;
                    let find = |wanted: u64| {
                        nodes
                            .iter()
                            .find(|node| node["id"].as_u64() == Some(wanted))
                            .ok_or_else(|| format!("node {wanted} is not in the snapshot"))
                    };
                    let (first, second) = (find(first)?, find(second)?);
                    let (Some(a), Some(b)) = (bounds(first), bounds(second)) else {
                        return Err(
                            "one of the nodes has no box, so they cannot be compared".into()
                        );
                    };
                    let verdict = if overlaps(a, b) { "OVERLAP" } else { "clear" };
                    println!("{verdict}\n  {}\n  {}", describe(first), describe(second));
                }
                _ => unreachable!(),
            }
        }
        other => return Err(format!("unknown command {other}\n\n{USAGE}").into()),
    }
    Ok(())
}

fn main() {
    let options = match parse_options() {
        Ok(options) => options,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(2);
        }
    };
    // A current-thread runtime: this is one connection doing one thing, and a
    // worker pool for it would be ceremony.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .build()
        .expect("a tokio runtime");
    if let Err(error) = runtime.block_on(run(options)) {
        eprintln!("chuzz-inspect: {error}");
        std::process::exit(1);
    }
}
