//! Serve one page over the inspection socket, with no window.
//!
//! # Why this is here and not in a second crate
//!
//! It used to be `qa-inspect-host`, a separate binary in ps-observability that
//! built its own document out of a dist directory. That made two headless
//! browsers: one that a person browses with and one that QA drives, and the web
//! platform existed in only the first of them. `document_loader`'s shim is what
//! makes `@solidjs/router` reach its first render at all, and a host without it
//! reported every routed page in the fleet as blank. Every gap closed for the
//! browser had to be closed a second time for the harness, by hand, or the
//! harness kept measuring a browser nobody ships.
//!
//! So the host is a mode of the browser instead. What `ps-qa` drives is the
//! same loader, the same shim and the same engine a tab uses; the only thing
//! missing is the window.
//!
//! `ps-qa` still links none of this. It speaks `blitz-control-protocol` over
//! the socket and is forbidden from depending on blitz, tauri, winit or wgpu,
//! which is satisfied by a socket, not by a crate boundary.
//!
//! # Use
//!
//! ```sh
//! chuzz-headless /path/to/dist            # or a file, or an http(s) URL
//! QA_INSPECT_PAGE=/path/to/dist chuzz-headless
//! ```
//!
//! It prints its descriptor path on stdout when it is ready, then serves until
//! killed. That line, and one page per process, are the two properties
//! `ps-qa sweep-components` is built on: it launches one host per component and
//! waits for the line, so a component that wedges the engine cannot poison the
//! next one's verdict.

use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc;

use blitz_dom::Document as _;
use blitz_script::ScriptDocument;
use blitz_traits::events::{BlitzImeEvent, UiEvent};
use blitz_traits::net::Url;
use blitz_traits::shell::{ColorScheme, Viewport};
use tauri_runtime_blitz::control_protocol::{
    AgentAction, AgentControlRequest, DebugError, DebugEvent, DebugResponse, DiagnosticsRequest,
    InputCommand, KeyPhase, WindowComposition,
};
use tauri_runtime_blitz::{
    AgentControlServer, ControlBridgeRequest, DocumentCapture, click_agent_node, focus_agent_node,
    hover_agent_node, inspect_document, press_agent_key, snapshot_document,
};

fn trace(message: &str) {
    eprintln!("chuzz-headless: {message}");
}

/// Timestamped tracing of the loop and the socket, off unless `CHUZZ_TRACE` is
/// set in the environment.
///
/// The question this exists for is not "did it happen" but "when did it happen
/// relative to what the harness was doing", which no snapshot of the tree can
/// answer. It is what found the outcome poll that went blind to the rest of the
/// document, and it is deliberately millisecond-stamped so its lines can be
/// read against a driver's own timings.
pub(crate) fn verbose(message: &str) {
    if std::env::var_os("CHUZZ_TRACE").is_none() {
        return;
    }
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_millis() % 1_000_000)
        .unwrap_or_default();
    eprintln!("chuzz-headless: [{at}] {message}");
}

/// A positive integer from the environment, or `default`.
fn dimension(variable: &str, default: u32) -> Result<u32, String> {
    let Some(value) = std::env::var_os(variable) else {
        return Ok(default);
    };
    let text = value
        .into_string()
        .map_err(|_| format!("{variable} is not valid UTF-8"))?;
    text.parse::<u32>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| format!("{variable} must be a positive integer, got {text:?}"))
}

/// What to load, from the command line or the environment.
///
/// `QA_INSPECT_PAGE` is how `ps-qa` tells a host which page to serve: it runs
/// the binary named by `--host` with no arguments and that variable set. An
/// explicit argument wins, so the same binary is usable by hand.
pub fn target_from(args: &[String]) -> Result<String, String> {
    if let Some(argument) = args.iter().skip(1).find(|arg| !arg.starts_with("--")) {
        return Ok(argument.clone());
    }
    std::env::var("QA_INSPECT_PAGE")
        .map_err(|_| "no page to serve: pass one as an argument or set QA_INSPECT_PAGE".to_owned())
}

/// What the caller named.
#[derive(Debug, PartialEq, Eq)]
pub enum Target {
    /// A built site, which needs an origin before it is a site at all. See
    /// [`crate::page_server`].
    Directory(std::path::PathBuf),
    /// A built page inside a directory: served as a site, and opened at that
    /// page rather than at `index.html`.
    ///
    /// A page in a build is no more a file than a whole site is. The component
    /// sweep names one page per component, `button.html` beside `button.js`,
    /// and each of those references `/static/js/button.js` absolutely, which a
    /// `file://` base resolves to the filesystem root. The bundle is then never
    /// fetched and the page renders as an empty mount point, which reads as a
    /// component that draws nothing.
    File {
        root: std::path::PathBuf,
        page: String,
    },
    /// A single file, or a page already being served somewhere.
    Page(Url),
}

/// Decide what the caller named.
///
/// A filesystem path is tried first, and only a string that is not a path is
/// handed to the address-bar rule. That order matters: `dist` is a directory
/// here and a bare hostname to `nav::request_from_input`, and guessing wrong
/// means fetching `https://dist/` and reporting whatever comes back as the page
/// under test.
pub fn classify(target: &str) -> Result<Target, String> {
    let path = Path::new(target);
    if path.exists() {
        let canonical = path
            .canonicalize()
            .map_err(|error| format!("could not resolve {target}: {error}"))?;
        if canonical.is_dir() {
            if !canonical.join("index.html").is_file() {
                return Err(format!(
                    "{} is a directory with no index.html in it",
                    canonical.display()
                ));
            }
            return Ok(Target::Directory(canonical));
        }
        // A built page needs an origin exactly as a whole site does, so its
        // directory is served and the page opened on it. Only a document:
        // anything else named directly is taken as given.
        if canonical
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("html") || ext.eq_ignore_ascii_case("htm"))
            && let (Some(root), Some(page)) = (
                canonical.parent(),
                canonical.file_name().and_then(|name| name.to_str()),
            )
        {
            return Ok(Target::File {
                root: root.to_path_buf(),
                page: page.to_owned(),
            });
        }
        return Url::from_file_path(&canonical)
            .map(Target::Page)
            .map_err(|()| format!("{} is not a path a URL can name", canonical.display()));
    }

    crate::nav::request_from_input(target)
        .map(|request| Target::Page(request.url))
        .ok_or_else(|| format!("{target} is neither a path that exists nor a URL"))
}

/// Drain synchronous script and reactive work without imposing a timer on every
/// control.
///
/// Delayed outcomes are polled by `ps-qa` against the exact declared verdict,
/// so sleeping here only makes fast controls slow and duplicates the caller's
/// timeout.
struct SettleFailure {
    error: DebugError,
    painted: bool,
}

fn settle_immediate(
    document: &mut ScriptDocument,
    clock: &std::time::Instant,
    deadline: std::time::Duration,
) -> Result<bool, SettleFailure> {
    let before = document.inner().paint_damage().generation;
    let started = std::time::Instant::now();
    let mut iterations = 0_u32;
    loop {
        if !document.poll(None) {
            break;
        }
        iterations = iterations.saturating_add(1);
        if started.elapsed() >= deadline {
            document.inner_mut().resolve(clock.elapsed().as_secs_f64());
            return Err(SettleFailure {
                painted: document.inner().paint_damage().generation != before,
                error: DebugError {
                    code: "documentNotQuiescent".into(),
                    message: format!(
                        "the document still had immediate work after {iterations} settle \
                         iterations and {}ms",
                        deadline.as_millis()
                    ),
                },
            });
        }
    }
    document.inner_mut().resolve(clock.elapsed().as_secs_f64());
    Ok(document.inner().paint_damage().generation != before)
}

/// How long the loop waits for a request before letting the page run.
///
/// Short enough that a page which is waiting on a timer or a response is not
/// noticeably slowed by it, long enough that an idle host is not a spin.
const IDLE_TICK: std::time::Duration = std::time::Duration::from_millis(8);

/// A bound on one idle turn, so a page that always has work still leaves the
/// loop able to answer. The next tick picks up where this one stopped.
const MAX_IDLE_TURNS: u32 = 512;

/// How many missed turns one reply may hand back to the page.
///
/// A request that took a long time owes the page the turns it spent, but the
/// debt has to end somewhere: without a cap, a single slow snapshot would let
/// the page run unbounded before the next request is even read, which is the
/// starvation this fixes pointed the other way.
const MAX_CATCHUP_TURNS: u32 = 64;

/// Let the page get on with what it started, while nothing is being asked of it.
///
/// Without this the document only advances inside a request. A page whose
/// bootstrap is a chain of asynchronous steps -- fetch a version, append a
/// script, wait for its `load`, mount -- gets exactly as far as the last step
/// that finished before the loader went quiet, and then stops. That is not a
/// slow page: it is a stopped one, and it reads as a site that renders nothing.
/// nofilter.io is the case that found it, and it stayed at 23 nodes for as long
/// as it was left running.
///
/// Returns whether the page painted.
fn tick_document(document: &mut ScriptDocument, clock: &std::time::Instant) -> bool {
    let before = document.inner().paint_damage().generation;
    let mut turns = 0_u32;
    while document.poll(None) {
        turns = turns.saturating_add(1);
        if turns >= MAX_IDLE_TURNS {
            break;
        }
    }
    document.inner_mut().resolve(clock.elapsed().as_secs_f64());
    let painted = document.inner().paint_damage().generation != before;
    if turns > 0 || painted {
        verbose(&format!("tick turns={turns} painted={painted}"));
    }
    painted
}

fn settle_response(
    document: &mut ScriptDocument,
    clock: &std::time::Instant,
    deadline: std::time::Duration,
    painted: &mut bool,
) -> DebugResponse {
    match settle_immediate(document, clock, deadline) {
        Ok(did_paint) => {
            *painted = did_paint;
            DebugResponse::Ack
        }
        Err(failure) => {
            *painted = failure.painted;
            DebugResponse::Error(failure.error)
        }
    }
}

/// Where a link click asks to go.
///
/// A plain `<a href>` is not the router's: `@solidjs/router` intercepts only its
/// own `<A>`, so every `Button href=` and `Link href=` in the fleet is an
/// ordinary anchor whose activation is the shell's to carry out. The windowed
/// browser has a shell. Without one the click is dispatched, acknowledged, and
/// nothing moves, which reads as a dead control rather than a missing
/// capability -- and it is most of the navigation on every site here.
///
/// A slot rather than a queue. Two navigations before the loop looks again
/// means the second won, exactly as it would in a browser, and a queue would
/// replay a page the user never waited for.
#[derive(Default)]
struct RequestedNavigation(std::sync::Mutex<Option<Url>>);

/// A clipboard, because a page with no window still has one.
///
/// The default `ShellProvider` refuses both directions, which is right for a
/// provider that has no shell to ask. It is wrong for a page: the usual copy
/// button is `await navigator.clipboard.writeText(t)` followed by the only
/// feedback a clipboard write ever has, and a rejected promise skips that line
/// and leaves an unhandled rejection behind. Under Solid 2 an error that
/// escapes every boundary halts the scheduler outright, so a headless run of a
/// page with a copy button is one press away from a frozen application that
/// still paints.
///
/// In memory and per host, which is the honest scope: nothing here reaches the
/// machine's clipboard, and a check that reads it back is reading what the page
/// wrote rather than what the operating system holds.
#[derive(Default)]
struct HostClipboard(std::sync::Mutex<String>);

impl blitz_traits::shell::ShellProvider for HostClipboard {
    fn get_clipboard_text(&self) -> Result<String, blitz_traits::shell::ClipboardError> {
        self.0
            .lock()
            .map(|held| held.clone())
            .map_err(|_| blitz_traits::shell::ClipboardError)
    }

    fn set_clipboard_text(&self, text: String) -> Result<(), blitz_traits::shell::ClipboardError> {
        let mut held = self
            .0
            .lock()
            .map_err(|_| blitz_traits::shell::ClipboardError)?;
        *held = text;
        Ok(())
    }
}

impl blitz_traits::navigation::NavigationProvider for RequestedNavigation {
    fn navigate_to(&self, options: blitz_traits::navigation::NavigationOptions) {
        if let Ok(mut slot) = self.0.lock() {
            *slot = Some(options.url);
        }
    }
}

impl RequestedNavigation {
    fn take(&self) -> Option<Url> {
        self.0.lock().ok().and_then(|mut slot| slot.take())
    }
}

/// What the page put in storage, carried from one document to the next.
///
/// The shim's `localStorage` is in-memory and per document, which is right for
/// a capture and wrong the moment the host follows a link: an application
/// writes its settings on one route and reads them while booting the next, and
/// a fresh store makes that read a miss. Every check of the shape "save, go
/// somewhere, come back" is undecidable without this.
///
/// It is a copy, not real storage. Nothing here is on disk, no quota is
/// enforced, and no `storage` event is delivered; what it buys is that the same
/// origin sees what it wrote.
struct StoredState {
    origin: String,
    local: String,
    session: String,
}

/// Read both stores out of the document that is about to be replaced.
///
/// Through the public interface -- `length`, `key`, `getItem` -- rather than
/// through the shim's internals, so this keeps working the day storage stops
/// being a closure over an object.
fn snapshot_storage(document: &mut ScriptDocument, origin: &str) -> StoredState {
    fn dump(document: &mut ScriptDocument, store: &str) -> String {
        let script = format!(
            "(function () {{
               try {{
                 var out = {{}};
                 for (var i = 0; i < {store}.length; i++) {{
                   var key = {store}.key(i);
                   if (key !== null) {{ out[key] = {store}.getItem(key); }}
                 }}
                 return JSON.stringify(out);
               }} catch (error) {{ return '{{}}'; }}
             }})()"
        );
        document.eval(&format!("globalThis.__chuzz_dump = {script};"));
        document
            .eval_json("globalThis.__chuzz_dump")
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned))
            .unwrap_or_else(|| "{}".to_owned())
    }

    StoredState {
        origin: origin.to_owned(),
        local: dump(document, "localStorage"),
        session: dump(document, "sessionStorage"),
    }
}

/// The prelude that puts it back, or nothing when it does not belong here.
///
/// Storage is keyed by origin in a browser and is keyed by origin here. A link
/// that leaves the site under test lands on a page that must not see the site's
/// keys, and carrying them across would be a leak invented by the harness.
fn restore_script(stored: Option<&StoredState>, destination: &Url) -> String {
    let Some(stored) =
        stored.filter(|stored| stored.origin == destination.origin().ascii_serialization())
    else {
        return String::new();
    };
    format!(
        "(function () {{
           try {{
             var local = JSON.parse({local});
             for (var key in local) {{ localStorage.setItem(key, local[key]); }}
             var session = JSON.parse({session});
             for (var key in session) {{ sessionStorage.setItem(key, session[key]); }}
           }} catch (error) {{}}
         }})();",
        // Serialized twice on purpose: the inner JSON is the data, and the
        // outer encoding makes it a JavaScript string literal that cannot end
        // the statement early. A page's own key is untrusted input here.
        local = serde_json::to_string(&stored.local).unwrap_or_else(|_| "\"{}\"".to_owned()),
        session = serde_json::to_string(&stored.session).unwrap_or_else(|_| "\"{}\"".to_owned()),
    )
}

fn commit_render(events: &tokio::sync::watch::Sender<Option<DebugEvent>>, revision: &mut u64) {
    *revision = revision.saturating_add(1);
    events.send_replace(Some(DebugEvent::PaintCommitted {
        revision: *revision,
    }));
}

/// Load `target` and serve it over the inspection socket until killed.
pub fn serve(target: &str) -> Result<(), String> {
    let target = classify(target)?;
    let width = dimension("QA_HOST_WIDTH", 1344)?;
    let height = dimension("QA_HOST_HEIGHT", 900)?;
    let settle_deadline =
        std::time::Duration::from_millis(u64::from(dimension("QA_HOST_SETTLE_MS", 100)?));

    // Multi-threaded, and entered for the whole run.
    //
    // The page keeps fetching after it is first laid out: images, fonts and
    // anything a script asks for arrive on the document's channel and are
    // applied by the next `resolve`. Blitz's net provider issues those with
    // `tokio::spawn`, which panics outright with no reactor entered, so the
    // dispatch loop below has to run inside the runtime rather than after it.
    // The blocking script fetch also takes `block_in_place`, which only the
    // multi-threaded runtime has.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("could not start a tokio runtime: {error}"))?;
    let _runtime_guard = runtime.enter();

    // A directory becomes a site before it becomes a document: its markup names
    // `/static/...` absolutely and its router owns paths that were never built
    // as files, neither of which a `file://` base can answer.
    let url = match &target {
        Target::Directory(root) => {
            let origin = runtime.block_on(crate::page_server::start(root))?;
            trace(&format!("serving {} at {origin}", root.display()));
            // A route can be opened directly rather than navigated to.
            //
            // A built single-page application has one file and many addresses:
            // `/spec` and `/designer` are the router's, not the filesystem's,
            // and the server already answers them with the document so the
            // router resolves them exactly as a production host does. Without
            // this the only way to reach such a page was to press whatever
            // links to it, which makes every check on a deep route depend on
            // the navigation above it and leaves a route with no link
            // unreachable altogether.
            let path = std::env::var("QA_HOST_PATH").unwrap_or_default();
            let target = format!("{origin}{}", path.trim_start_matches('/'));
            Url::parse(&target).map_err(|error| format!("{target} is not a URL: {error}"))?
        }
        Target::File { root, page } => {
            let origin = runtime.block_on(crate::page_server::start(root))?;
            trace(&format!("serving {} at {origin}", root.display()));
            Url::parse(&format!("{origin}{page}"))
                .map_err(|error| format!("{origin}{page} is not a URL: {error}"))?
        }
        Target::Page(url) => url.clone(),
    };

    let net_provider = Arc::new(blitz_net::Provider::with_user_agent(
        None,
        &crate::identity::user_agent_from_env(),
    ));
    let navigation = Arc::new(RequestedNavigation::default());
    // One clipboard for the host, not one per document, so a page that copies
    // on one route and reads it back on another sees what it wrote.
    let clipboard = Arc::new(HostClipboard::default());
    let animation_clock = std::time::Instant::now();

    let load = |url: Url, carried: Option<&StoredState>| -> Result<Box<ScriptDocument>, String> {
        trace(&format!("loading {url}"));
        let loaded = runtime
            .block_on(crate::document_loader::load_for_capture(
                blitz_traits::net::Request::get(url.clone()),
                Arc::clone(&net_provider),
                &restore_script(carried, &url),
            ))
            .map_err(|error| format!("could not load {url}: {error}"))?;
        let mut document = loaded
            .into_script()
            .ok_or("this build cannot run scripts, so it cannot host a page worth inspecting")?;
        document
            .inner_mut()
            .set_viewport(Viewport::new(width, height, 1.0, ColorScheme::Dark));
        document.inner_mut().set_paint_damage_tracking(true);
        document
            .inner_mut()
            .set_navigation_provider(Arc::clone(&navigation) as _);
        document
            .inner_mut()
            .set_shell_provider(Arc::clone(&clipboard) as _);
        // The loader already ran and pumped the page's scripts. This settles
        // what the viewport change queued, rather than sleeping for a fixed
        // interval before announcing the socket.
        if let Err(failure) = settle_immediate(&mut document, &animation_clock, settle_deadline) {
            trace(&format!(
                "the page reached the settle deadline: {}",
                failure.error.message
            ));
        }
        Ok(document)
    };

    // Which origin the standing document belongs to, so what it stored is
    // offered back only to itself.
    let mut origin = url.origin().ascii_serialization();
    let mut document = load(url, None)?;
    trace("document ready");

    /*
     * The bridge hands a request to this thread and waits for the answer.
     *
     * A `SyncSender` with a zero-capacity channel would rendezvous, but the
     * server thread must not block indefinitely if this loop has gone away, so
     * the reply travels on a per-request oneshot the caller owns.
     */
    const MAX_PENDING_REQUESTS: usize = 64;
    let (request_tx, request_rx) = mpsc::sync_channel::<(
        ControlBridgeRequest,
        tokio::sync::oneshot::Sender<DebugResponse>,
    )>(MAX_PENDING_REQUESTS);

    let bridge: tauri_runtime_blitz::ControlBridge = Arc::new(move |request| {
        let (response_tx, response_rx) = tokio::sync::oneshot::channel();
        match request_tx.try_send((request, response_tx)) {
            Ok(()) => response_rx,
            Err(mpsc::TrySendError::Full((_, response_tx))) => {
                verbose("request refused: queue full");
                let _ = response_tx.send(DebugResponse::Error(DebugError {
                    code: "documentBusy".into(),
                    message: format!(
                        "the document already has {MAX_PENDING_REQUESTS} pending inspection \
                         requests"
                    ),
                }));
                response_rx
            }
            Err(mpsc::TrySendError::Disconnected((_, response_tx))) => {
                verbose("request refused: document gone");
                let _ = response_tx.send(DebugResponse::Error(DebugError {
                    code: "documentUnavailable".into(),
                    message: "the document is no longer serving".into(),
                }));
                response_rx
            }
        }
    });

    let (render_events, render_event_receiver) = tokio::sync::watch::channel(None);
    let server = AgentControlServer::start_with_events(bridge, render_event_receiver)
        .map_err(|error| format!("could not host the control socket: {error}"))?;
    trace(&format!(
        "inspection socket listening: {}",
        server.descriptor_path().display()
    ));
    // The descriptor path on stdout, so a caller can attach without guessing
    // it. `ps-qa --app` takes a descriptor, and a sweep that has to search a
    // directory races every other instance on the machine.
    println!("{}", server.descriptor_path().display());
    use std::io::Write as _;
    let _ = std::io::stdout().flush();

    let mut revision = 0_u64;
    let mut render_revision = 0_u64;
    let mut capture = DocumentCapture::new();
    // When the page was last allowed to run. A request resets nothing on its
    // own, so this is what keeps the guarantee below true under load.
    let mut last_tick = std::time::Instant::now();
    loop {
        let (request, reply) = match request_rx.recv_timeout(IDLE_TICK) {
            Ok(pair) => pair,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if tick_document(&mut document, &animation_clock) {
                    commit_render(&render_events, &mut render_revision);
                }
                last_tick = std::time::Instant::now();
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        verbose(match &request {
            ControlBridgeRequest::Agent(AgentControlRequest::Inspect { .. }) => "request inspect",
            ControlBridgeRequest::Agent(AgentControlRequest::Act(_)) => "request act",
            _ => "request other",
        });
        let mut painted = false;
        let response = match request {
            ControlBridgeRequest::Agent(request) => match request {
                AgentControlRequest::Inspect { root, max_depth } => {
                    revision += 1;
                    inspect_document(&mut document, root, max_depth, revision)
                }
                AgentControlRequest::Act(AgentAction::Focus { node_id }) => {
                    let node_id = blitz_dom::NodeId::from_u64(node_id);
                    match focus_agent_node(&mut document, node_id) {
                        Ok(()) => settle_response(
                            &mut document,
                            &animation_clock,
                            settle_deadline,
                            &mut painted,
                        ),
                        Err(error) => DebugResponse::Error(error),
                    }
                }
                AgentControlRequest::Act(AgentAction::Click { node_id }) => {
                    match click_agent_node(&mut document, node_id, 1) {
                        Ok(_) => settle_response(
                            &mut document,
                            &animation_clock,
                            settle_deadline,
                            &mut painted,
                        ),
                        Err(error) => DebugResponse::Error(error),
                    }
                }
                AgentControlRequest::Act(AgentAction::ScrollIntoView { node_id }) => {
                    /*
                     * Really scrolled, not acknowledged.
                     *
                     * The host this replaces served one component on a page
                     * that never overflowed, so it answered Ack and was right
                     * by accident. A fleet site scrolls, and a control below
                     * the fold that is never brought into view is hovered at
                     * whatever happens to be at its coordinates, which reads as
                     * a control that does not respond.
                     */
                    let node_id = blitz_dom::NodeId::from_u64(node_id);
                    document.inner_mut().scroll_to_node(node_id);
                    settle_response(
                        &mut document,
                        &animation_clock,
                        settle_deadline,
                        &mut painted,
                    )
                }
                AgentControlRequest::Act(AgentAction::Hover { node_id }) => {
                    /*
                     * A control revealed on hover is unreachable without this,
                     * and a defect that only shows on the second entry is
                     * unreachable even with one hover: a pill whose hover
                     * appends a shadow layer and never removes it looks right
                     * once.
                     */
                    match hover_agent_node(&mut document, node_id) {
                        Ok(_) => settle_response(
                            &mut document,
                            &animation_clock,
                            settle_deadline,
                            &mut painted,
                        ),
                        Err(error) => DebugResponse::Error(error),
                    }
                }
                AgentControlRequest::Act(AgentAction::DoubleClick { node_id }) => {
                    match click_agent_node(&mut document, node_id, 2) {
                        Ok(_) => settle_response(
                            &mut document,
                            &animation_clock,
                            settle_deadline,
                            &mut painted,
                        ),
                        Err(error) => DebugResponse::Error(error),
                    }
                }
                AgentControlRequest::Act(AgentAction::SetValue { node_id, value }) => {
                    let node_id = blitz_dom::NodeId::from_u64(node_id);
                    let current = document
                        .inner()
                        .get_node(node_id)
                        .and_then(|node| node.element_data())
                        .and_then(|element| element.text_input_data())
                        .map(|input| input.editor.text().to_string());
                    match current {
                        None => DebugResponse::Error(DebugError {
                            code: "notEditable".into(),
                            message: "node is not a text input".into(),
                        }),
                        Some(current) => {
                            document.inner_mut().set_focus_to(node_id);
                            /*
                             * Clear by byte count, not by selecting the text
                             * first.
                             *
                             * `select_all` builds its selection with
                             * `move_lines(&layout, isize::MAX)`, and
                             * `select_byte_range` resolves its ends through
                             * `Cursor::from_byte_index(&layout, ..)`. Both read
                             * the laid out text, so both depend on a font
                             * catalogue being present: with none registered
                             * every glyph shapes to nothing, the selection
                             * comes back collapsed, and the commit below
                             * inserts at the caret instead of replacing.
                             *
                             * This build has faces, so `select_all` would work
                             * here. It stays byte arithmetic anyway, because a
                             * host that behaves differently depending on which
                             * fonts the machine has is a harness that reports
                             * different verdicts on CI and on a laptop.
                             * `delete_bytes_before_selection` and
                             * `delete_bytes_after_selection` clamp to the ends
                             * of the buffer, so between them they empty it from
                             * wherever the caret is, with no layout involved.
                             */
                            if let Some(len) = NonZeroUsize::new(current.len()) {
                                document.inner_mut().with_text_input(node_id, |mut editor| {
                                    editor.delete_bytes_before_selection(len);
                                    editor.delete_bytes_after_selection(len);
                                });
                            }
                            document.handle_ui_event(UiEvent::Ime(BlitzImeEvent::Commit(value)));
                            settle_response(
                                &mut document,
                                &animation_clock,
                                settle_deadline,
                                &mut painted,
                            )
                        }
                    }
                }
                AgentControlRequest::Act(AgentAction::Input(InputCommand::Key {
                    key,
                    code,
                    phase,
                    ..
                })) => {
                    /*
                     * One press per Down, and nothing on the matching Up.
                     *
                     * `press_agent_key` sends both halves, because a control
                     * that acts on keyup never fires if only a keydown arrives.
                     * A client that sends the pair would otherwise press the
                     * key twice, and Escape pressed twice closes a menu and
                     * then whatever was behind it.
                     */
                    if matches!(phase, KeyPhase::Up) {
                        DebugResponse::Ack
                    } else {
                        match press_agent_key(&mut document, &key, &code) {
                            Ok(()) => settle_response(
                                &mut document,
                                &animation_clock,
                                settle_deadline,
                                &mut painted,
                            ),
                            Err(error) => DebugResponse::Error(error),
                        }
                    }
                }
                // Everything else needs runtime state this mode does not have,
                // and saying so is better than a plausible-looking Ack: a check
                // that silently did nothing reports the page as broken.
                _ => DebugResponse::Error(DebugError {
                    code: "unsupported".into(),
                    message: "the headless page serves Inspect, Focus, Hover, Click, \
                              DoubleClick, ScrollIntoView, SetValue and Key only"
                        .into(),
                }),
            },
            ControlBridgeRequest::Diagnostics(DiagnosticsRequest::Capture(request)) => {
                if !request.scale.is_finite() || !(0.25..=8.0).contains(&request.scale) {
                    DebugResponse::Error(DebugError {
                        code: "invalidArgument".into(),
                        message: "capture scale must be finite and between 0.25 and 8".into(),
                    })
                } else {
                    match capture.capture(&mut document, request) {
                        Ok(captured) => DebugResponse::Captured(captured),
                        Err(error) => DebugResponse::Error(error),
                    }
                }
            }
            ControlBridgeRequest::Diagnostics(DiagnosticsRequest::Snapshot(request)) => {
                revision += 1;
                match snapshot_document(&mut document, request, revision) {
                    Ok(snapshot) => DebugResponse::Snapshot(snapshot),
                    Err(error) => DebugResponse::Error(error),
                }
            }
            ControlBridgeRequest::Diagnostics(DiagnosticsRequest::WindowComposition) => {
                // There is no window, so there is nothing composited over the
                // page. Reporting the default is the true answer here, and it
                // is what lets a spill check run headlessly at all.
                DebugResponse::WindowComposition(WindowComposition::default())
            }
            ControlBridgeRequest::Diagnostics(_) => DebugResponse::Error(DebugError {
                code: "unsupported".into(),
                message: "the headless page serves diagnostics Capture, Snapshot and \
                          WindowComposition only"
                    .into(),
            }),
        };
        if painted {
            commit_render(&render_events, &mut render_revision);
        }
        if reply.send(response).is_err() {
            break;
        }
        // The page runs on a schedule, not only when the socket goes quiet.
        //
        // `recv_timeout` ticks the document when *no* request arrives, which
        // is exactly backwards for a harness: a check waiting for an outcome
        // inspects the tree continuously, the timeout never fires, and the
        // page it is waiting on never advances. The check then waits out its
        // whole deadline for something that completes moments after it gives
        // up. Measured on honey.id, whose sign-in completed three seconds
        // after a 150-second check declared it had not.
        //
        // Anything driven by a host callback rather than by a request lands
        // here: socket messages, timers, promise continuations.
        //
        // One turn per reply is not enough, and the arithmetic is the reason.
        // A semantic snapshot of a signed-in page costs upwards of 150ms, and
        // a driver polling for an outcome sends the next one the moment it has
        // the last. That bought the page six turns a second while the harness
        // watched, against roughly a hundred when it did not, so a login that
        // needed a few hundred turns of timers and promise continuations
        // completed only after the harness gave up and disconnected. Thirty
        // seconds of polling produced 179 turns; six seconds of quiet produced
        // about six hundred.
        //
        // So the page is owed the turns the request consumed. `last_tick`
        // advances by one interval per turn rather than being reset to now,
        // which is what makes this a catch-up rather than a single tick, and
        // the cap keeps one slow request from handing the page an unbounded
        // slice.
        //
        // After the reply, never before it. Ticking first resolves layout
        // underneath the request that is about to be answered, and an inspect
        // then reports a tree whose every box is 0x0.
        let mut owed = 0_u32;
        while last_tick.elapsed() >= IDLE_TICK && owed < MAX_CATCHUP_TURNS {
            if tick_document(&mut document, &animation_clock) {
                commit_render(&render_events, &mut render_revision);
            }
            last_tick += IDLE_TICK;
            owed += 1;
        }
        // A request that took longer than the cap allows leaves `last_tick` in
        // the past, which would make the next reply owe those turns again for
        // ever. The debt is forgiven at the cap: the page is behind, and the
        // honest thing is to carry on from now.
        if owed >= MAX_CATCHUP_TURNS {
            last_tick = std::time::Instant::now();
        }

        /*
         * A link the last action followed.
         *
         * After the reply, not before it, so the action is acknowledged with
         * the timing it actually had rather than with a page load added to it.
         * A caller inspects again before it asserts anything, and by then the
         * new document is standing.
         *
         * A load that fails leaves the old page up and says so. Serving an
         * error document instead would make every check after it fail against
         * a page that is not the one under test, and the reason would be four
         * checks back in the log.
         */
        if let Some(destination) = navigation.take() {
            trace(&format!("following a link to {destination}"));
            // Read out before the document that holds it goes. A page writes
            // its settings on one route and reads them while booting the next,
            // so a store that starts empty makes that read a miss and the
            // application look like it never saved.
            let carried = snapshot_storage(&mut document, &origin);
            let arriving = destination.origin().ascii_serialization();
            match load(destination, Some(&carried)) {
                Ok(next) => {
                    document = next;
                    origin = arriving;
                    commit_render(&render_events, &mut render_revision);
                }
                Err(error) => trace(&format!("the navigation failed, staying put: {error}")),
            }
        }
    }

    trace("inspection host finished");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Target, Url, classify, target_from};

    fn fixture_root(label: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "chuzz-serve-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock is after the epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).expect("create fixture root");
        root
    }

    /// A built page is a directory, and `ps-qa sweep-components` hands one over
    /// per component. It gets an origin rather than a `file://` path to its
    /// index, because a built site names its bundle absolutely and its router
    /// owns paths that were never built as files.
    #[test]
    fn a_directory_is_served_as_a_site() {
        let root = fixture_root("dir");
        std::fs::write(root.join("index.html"), "<html></html>").expect("write index");
        let target = classify(root.to_str().expect("utf-8 fixture path")).expect("classify");
        assert!(
            matches!(target, Target::Directory(_)),
            "a built page needs an origin, got {target:?}"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// A directory that is not a built page is refused rather than served
    /// empty, because an empty tree over a working socket reads as a component
    /// that renders nothing.
    #[test]
    fn a_directory_with_no_index_is_refused() {
        let root = fixture_root("empty");
        let error =
            classify(root.to_str().expect("utf-8 fixture path")).expect_err("no index to serve");
        assert!(
            error.contains("no index.html"),
            "unhelpful refusal: {error}"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// The ordering that matters: a relative path is checked against the
    /// filesystem before it is offered to the address-bar rule. `dist` is a
    /// directory here and a bare hostname to `request_from_input`, and guessing
    /// wrong fetches `https://dist/` and reports the result as the page under
    /// test.
    #[test]
    fn a_path_that_exists_is_never_read_as_a_hostname() {
        let root = fixture_root("host");
        let page = root.join("example.com");
        std::fs::create_dir(&page).expect("create fixture page");
        std::fs::write(page.join("index.html"), "<html></html>").expect("write index");
        let target = classify(page.to_str().expect("utf-8 fixture path")).expect("classify");
        assert!(
            matches!(target, Target::Directory(_)),
            "a path named like a host was fetched over the network: {target:?}"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// A built page is served from its own directory rather than opened as a
    /// file. Its markup names `/static/...` absolutely, which a `file://` base
    /// resolves to the filesystem root, so the bundle is never fetched and the
    /// page renders as an empty mount point.
    #[test]
    fn a_built_page_is_served_from_its_directory() {
        let root = fixture_root("file");
        let page = root.join("page.html");
        std::fs::write(&page, "<html></html>").expect("write page");
        let target = classify(page.to_str().expect("utf-8 fixture path")).expect("classify");
        let Target::File {
            root: served,
            page: name,
        } = target
        else {
            panic!("a built page should be served from its directory, got {target:?}");
        };
        assert_eq!(name, "page.html");
        assert!(served.ends_with(root.file_name().expect("fixture directory name")));
        let _ = std::fs::remove_dir_all(root);
    }

    /// Anything else named directly is still taken as given. The engine's own
    /// fixtures include files that are not documents.
    #[test]
    fn a_file_that_is_not_a_document_is_taken_as_a_page() {
        let root = fixture_root("plain-file");
        let page = root.join("data.json");
        std::fs::write(&page, "{}").expect("write file");
        let target = classify(page.to_str().expect("utf-8 fixture path")).expect("classify");
        let Target::Page(url) = target else {
            panic!("a plain file should be a page, got {target:?}");
        };
        assert_eq!(url.scheme(), "file");
        let _ = std::fs::remove_dir_all(root);
    }

    /// A remote page is still a page. The fleet is deployed, and pointing the
    /// harness at what is actually serving is the whole point of sharing the
    /// browser's loader.
    #[test]
    fn a_url_is_taken_as_written() {
        let target = classify("https://support.cafe/").expect("resolve url");
        assert_eq!(
            target,
            Target::Page(Url::parse("https://support.cafe/").unwrap())
        );
    }

    #[test]
    fn a_target_that_is_neither_is_refused() {
        let error = classify("not a page").expect_err("prose is not a page");
        assert!(
            error.contains("neither a path that exists nor a URL"),
            "unhelpful refusal: {error}"
        );
    }

    /// `ps-qa` runs the host binary with no arguments and `QA_INSPECT_PAGE`
    /// set, so a host that only reads argv is not a host it can launch.
    #[test]
    fn an_argument_wins_over_the_environment() {
        let args = vec!["chuzz-headless".to_owned(), "example.com".to_owned()];
        assert_eq!(target_from(&args).expect("argument"), "example.com");
    }

    /// A flag is not the page. The argument scan skips anything beginning with
    /// `--`, or `--serve` itself would be loaded as an address.
    #[test]
    fn a_flag_is_not_mistaken_for_the_page() {
        let args = vec![
            "chuzz-headless".to_owned(),
            "--serve".to_owned(),
            "example.com".to_owned(),
        ];
        assert_eq!(target_from(&args).expect("argument"), "example.com");
    }
}
