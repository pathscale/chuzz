# Sandbox plan: what Chrome and Firefox do, and what we should do instead

Written 2026-08-11. Companion to
[TODO-plan-what-exists-in-open-source.md](TODO-plan-what-exists-in-open-source.md).

CSP is not the answer and neither is copying Chrome. Both are shaped by constraints we do
not have, and we have one they do not: **we are Rust, and we own the only choke point.**

---

## 0. Fix this first. It is live

`packages/blitz-net/src/lib.rs:147-149`:

```rust
"file" => {
    let file_content = std::fs::read(request.url.path())?;
    Ok((request.url.to_string(), Bytes::from(file_content)))
}
```

**No origin check anywhere above it.** Any document that can cause a subresource fetch to a
`file://` URL reads that file with the user's full permissions — a stylesheet `url()`, an
`<img src>`, an `<iframe>`, and directly via `fetch` the moment we ship it.

`https://evil.example/` embedding `file:///Users/you/.ssh/id_rsa` is a file read today. What
it can *do* with the bytes varies by context — an image leaks little, a stylesheet or script
leaks a lot, `fetch` leaks everything — but the read happens regardless.

**The fix is about five lines**: web-origin documents may not fetch `file:`. Only a
top-level `file://` navigation the user performed may. No policy language, no spec, no
subsystem. **Do this before anything else on any list.**

---

## 1. What Chrome does

- **Multi-process with a privileged broker.** Renderers have no direct filesystem or network
  access; the browser process brokers everything over IPC. This is the whole architecture —
  the sandbox is only meaningful because the renderer *cannot* do the thing directly.
- **Site Isolation** since 2018, driven by Spectre: one renderer per site (eTLD+1), so a
  compromised renderer holds only one site's data. Costs roughly **10–13% memory**.
- **macOS**: Seatbelt profiles plus entitlements, applied early — before any page content is
  touched.
- **Linux**: layered — user namespaces plus a seccomp-bpf syscall filter.
- **Windows**: restricted token, job object, desktop isolation, AppContainer.

## 2. What Firefox does

- **Fission**, site isolation, default since Firefox 100 in 2022. Chrome's is generally held
  to be more granular.
- Same per-platform primitives: Seatbelt on macOS, seccomp-bpf plus namespaces on Linux.
- **RLBox**, which is the interesting one: risky **C libraries** are compiled to WebAssembly
  and run *inside* the process with their memory access confined. Applied to about five
  libraries — font shaping, spell checking, XML, WOFF2, audio decode. Fine-grained,
  in-process, no IPC.

## 3. Why neither model transfers

**Both exist primarily to contain memory-safety exploits in C++.** That is the threat model:
attacker gets arbitrary code execution inside the renderer through a heap bug in a decoder or
a JIT, and the sandbox stops that from becoming access to the machine.

Two things make our position different:

**We are Rust, and our hostile-byte surface is almost entirely safe Rust.** The parts that
eat attacker-controlled bytes:

| Component | Language |
|---|---|
| HTML parsing (`html5ever`) | Rust |
| CSS parsing (Stylo) | Rust |
| PNG (`png`), JPEG (`zune-jpeg`), GIF (`gif`), WebP (`image-webp`) | **Rust** |
| SVG (`usvg`, `tiny-skia`) | **Rust** |
| Fonts (`skrifa`) | **Rust** |
| Brotli (`brotli`) | **Rust** |
| JS (Boa) | **Rust** |

That is the list Firefox needed RLBox for, and for us it is already memory-safe. **We start
where Firefox spent years getting to.**

**Our remaining C surface is small and mostly removable:**

| Crate | Why it is there | Action |
|---|---|---|
| `openssl-sys` / `openssl-src` | `reqwest` is configured with `native-tls` | **Switch to `rustls-tls`.** Removes OpenSSL — and its build — from the tree entirely. Cheap, and it is the largest C dependency we have |
| `dav1d-sys` | AV1 decode, C, historically CVE-prone | We do not decode video. Confirm it is not actually linked, and drop it if it is |
| `libsqlite3-sys` | transitive | trace and drop if unused |

**And we are single-process**, so Chrome's model is not available without a rearchitecture
that is not worth doing yet.

## 4. What we should do instead

Our sandbox's job is not memory containment — Rust does most of that. It is **capability
restriction**: bounding what the process may touch, and what a *page* may ask the process to
touch.

### Layer 1: the resource choke point — cheapest, highest value

`blitz-net::Provider::fetch_inner` is a single function every subresource passes through, and
it already switches on `request.url.scheme()`. One `ResourcePolicy` there gives us, in order
of value:

1. **`file:` denied to web origins** (§0 — live bug)
2. **Private and loopback addresses denied to public origins.** A page on the internet must
   not reach `127.0.0.1`, `::1`, `10/8`, `172.16/12`, `192.168/16` or `.local`. This is the
   attack CSP does nothing about: a web page port-scanning your dev servers, or hitting your
   router's admin panel. Chrome only added this recently as Private Network Access. **We can
   have it in one function** because we have one choke point
3. **Cookie scoping** — `credentials: "same-origin"` by default, since `lib.rs:77` sets one
   process-wide `cookie_store(true)`
4. Request count and total-byte caps per document, which also bounds runaway pages

None of this is a policy language. It is a Rust struct owned by the browser, not declared by
the page — which is the inversion that makes it simpler than CSP. **CSP asks the page what it
wants and enforces that. We decide what a page gets.**

#### The one thing that makes this non-trivial: a `Request` has no origin

`blitz-traits/src/net.rs:53-60`:

```rust
pub struct Request {
    pub url: Url,
    pub method: Method,
    pub content_type: Option<String>,
    pub headers: HeaderMap,
    pub body: Body,
    pub signal: Option<AbortSignal>,
}
```

There is no initiator, no origin, no referrer. So a policy at `fetch_inner` **cannot tell a
page's request for `file:///etc/passwd` from the browser's own load of a local file** — which
is exactly the distinction §0 needs. This is the actual work; the deny rule itself is trivial.

Two routes:

| | Route | Touches | Notes |
|---|---|---|---|
| **A** | Add `initiator: Option<Url>` (or an `Origin` enum: `Browser` \| `Document(Url)`) to `Request` | `blitz-traits/src/net.rs`, **13 construction sites** across `blitz/src/lib.rs` and `blitz-dom/src/net.rs`, and **5 `.fetch(` call sites** in `blitz-dom` (`document.rs:1260`, `iframe.rs:135`, `net.rs:213`, `net.rs:469`, `mutator.rs:1255`) | **Preferred.** Matches the fetch spec, which gives a request an origin. Mechanical, and the compiler finds every site |
| **B** | Resolve `doc_id` → base URL in the provider. `NetProvider::fetch(&self, doc_id: usize, …)` already carries it (`net.rs:20`) | `blitz-net` only | Less invasive but makes the network provider hold document state it otherwise does not, and `doc_id` is a `usize` with no lifetime guarantees |

Take **A**. 13 call sites is an afternoon, the type system enumerates them, and every later
policy — cookie scoping, private-network blocking, per-document caps — needs the same field.
Doing B first means doing A afterwards anyway.

The enum matters more than the URL: `Origin::Browser` for a user-typed navigation or our own
chrome, `Origin::Document(url)` for anything a page caused. The `file:` rule is then one
match arm.

### Layer 2: process sandbox — one model per platform, none of them invented here

An earlier draft of this section reached for `birdcage` (cross-platform, generic, last
updated 2024-04) and then shrugged and said "write a profile". Both were wrong. **Each
platform already has a reference implementation written for exactly our process type, and
each is permissively licensed** — which is the difference between these and fastrender.

#### macOS: WebKit's own WebContent profile

[`Source/WebKit/WebProcess/com.apple.WebProcess.sb.in`](https://github.com/WebKit/WebKit/blob/main/Source/WebKit/WebProcess/com.apple.WebProcess.sb.in)
— **62KB, BSD 2-clause, Apple, continuously maintained since 2010.** It pulls in
`Shared/Sandbox/macOS/common.sb`, `webcontent-defines.sb` and `preferences.sb`.

This is Safari's web content sandbox: a Darwin-native SBPL profile for a process whose whole
job is rendering untrusted web content — the same process we are. It is the better model
than Chrome's macOS sandbox, which is a port of a cross-platform design, and vastly better
than anything we would write, because the hard part of an SBPL profile is not the deny-all
line, it is knowing the dozens of Mach services and system paths a rendering process
genuinely needs and the ones it does not.

**BSD licence means we may actually adapt it**, not merely read it. Start from their
deny-by-default structure and cut what we do not have — no Mach IPC to a UI process, no
media stack, no WebGL. We will end up with far less than 62KB, and we will have started
from a profile Apple keeps current rather than one we guessed at.

#### Linux: `ErickJ3/sandbox-rs`

**MIT, and that matters more than the fact that it is archived.** Archived means we would
own the maintenance; MIT means we are allowed to. Unprivileged mode via user namespaces,
Landlock and `setrlimit`; privileged mode with cgroups v2 and chroot; six seccomp-BPF
profiles; auto-detection between modes. Split into `sandbox-core`, `-seccomp`, `-landlock`,
`-namespace`, `-cgroup`, `-fs`.

That is the right layering for Linux and it matches what Chrome and Firefox both do there —
namespaces plus seccomp-bpf. Vendor it or depend on it and pin.

#### Windows: not now

We do not ship Windows. When we do: restricted token, job object, AppContainer, which is
what Chrome, Firefox and fastrender's `crates/win-sandbox` all converge on. Nothing to
decide today.

#### How it actually gets applied in chuzz

**Not the App Sandbox.** `com.apple.security.app-sandbox` is entitlement-based, needs a real
Developer ID and provisioning to distribute, and would change how the cask is built and
signed. `apps/chuzz/build-app.sh:57` signs **ad-hoc** (`codesign --force --deep --sign -`)
with no entitlements file at all.

**Seatbelt instead, applied by the process to itself at runtime.** `sandbox_init` takes a
profile string and needs no entitlement, no Developer ID and no change to signing — which is
exactly why it survives our ad-hoc cask flow untouched. It has been formally deprecated since
10.8 and is still what Chrome and WebKit both use; there is no maintained Rust crate, so it is
an `extern "C"` declaration and `libc`.

**Where to call it is the part to get right.** Two constraints pull in opposite directions:

- as *early* as possible, because everything before the call is unconfined
- *after* graphics and font initialisation, because Metal, the WindowServer connection and
  fontconfig each need Mach services and system paths that are painful to allow through a
  profile and trivial to inherit if the connection already exists

So: initialise the window, renderer and font stack, then apply the profile, **then** load the
first page.

**There is no seam in `main` for that.** `apps/chuzz/src/main.rs:131` is
`dioxus_native::launch_cfg(app, contexts, vec![Box::new(window_attributes)])`, and it blocks
— it owns the event loop, the window and the renderer. Everything before it is pre-graphics;
everything after it is exit. So the two obvious options are:

1. **Before `launch_cfg:131`**, and let Metal, the WindowServer connection and fontconfig
   initialise *inside* the sandbox. Simplest by far, and it puts the profile on before
   anything else runs. The cost is that the profile must permit graphics and font
   initialisation, which is the fiddly part of an SBPL profile — but it is also precisely
   what WebKit's `com.apple.WebProcess.sb.in` already spells out, so we are copying the
   answer rather than deriving it.
2. **Inside `app()` at `main.rs:134`**, in a `use_hook` that runs once on first render, after
   the window exists. Narrower profile needed, but it runs after arbitrary dioxus and wgpu
   initialisation and it is easy for a future refactor to move content loading earlier than
   the hook.

**Take 1.** The whole reason to prefer WebKit's profile over one we write is that it already
covers what a Darwin rendering process needs at startup. Option 2 trades that advantage away
to save profile lines.

`chuzz::sandbox` as a new module in `apps/chuzz/src/`, `#[cfg(target_os = "macos")]`, called
on the first line of `main`.

Note the `--capture` path at `main.rs:57` returns before ever reaching `launch_cfg`. It takes
a URL and renders headlessly, so it needs its own call or it becomes the unsandboxed way to
render a hostile page.

#### The probe matters more than the profile

fastrender ships **three separate binaries** — `macos_sandbox_probe`, `sandbox_probe`,
`appcontainer_temp_smoke` — plus `scripts/trace_renderer_syscalls.sh`, whose only job is
asserting at runtime that the sandbox is actually applied.

Copy that discipline whatever else we do. A Seatbelt profile that fails to apply is
indistinguishable from no sandbox at all, silently, and it *will* stop applying during some
future refactor of how the app is launched or signed.

Ours needs to be about ten lines and assert the things the profile claims:

| Check | Expect |
|---|---|
| `std::fs::read("/etc/passwd")` | denied |
| read anywhere under `$HOME` outside our cache and profile dirs | denied |
| write outside the cache dir | denied |
| `std::process::Command::new("/bin/sh")` | denied |
| read our own cache dir | **allowed** — a profile that denies everything is also broken |

Run it two ways, because they catch different failures: as a `#[test]` in CI so a profile
change that over-denies fails the build, and **at startup behind a flag in the shipped
binary**, because CI does not prove the profile applied in a signed, quarantined,
cask-installed bundle. That last environment is the one that will break.

Wire it to the release workflow too. The cask strips `com.apple.quarantine` in `postflight`,
so the installed bundle differs from the built one, and that difference is exactly where a
silent failure would hide.

### Layer 3: shrink the C surface

`rustls-tls` instead of `native-tls`. Audit `dav1d-sys` and `libsqlite3-sys` and drop what is
not used. This is the RLBox-equivalent work, except that instead of sandboxing C we delete
it.

### Layer 4: process separation

The right answer eventually and not now. It is a rearchitecture, it costs 10–13% memory in
Chrome's own accounting, and every layer above delivers more per hour today.

---

## The plan

| # | Item | Size | Why now |
|---|---|---|---|
| 1 | **Deny `file:` to web origins** | ~5 lines | Live arbitrary file read |
| 2 | `credentials: "same-origin"` when `fetch` lands | a few lines | One process-wide cookie jar |
| 3 | **Deny private/loopback to public origins** | one function | Real attack, and CSP addresses none of it |
| 4 | `rustls-tls` instead of `native-tls` | a feature flag | Deletes our largest C dependency |
| 5 | **Seatbelt profile adapted from WebKit's `com.apple.WebProcess.sb.in`** (BSD, Apple's own, 62KB), **plus a probe that fails loudly** | days, not weeks, and none of it invented | We ship a signed cask today |
| 6 | Per-document request and byte caps | small | Bounds runaway and hostile pages |
| 7 | Audit and drop `dav1d-sys`, `libsqlite3-sys` | investigation | Shrink C surface |
| 8 | Linux: vendor or pin `ErickJ3/sandbox-rs` (MIT — archived, but that only means we own the maintenance) | small | Whenever we ship Linux |
| 9 | Windows: restricted token + job object + AppContainer | unknown | We do not ship Windows |
| 10 | Site isolation / process separation | rearchitecture | **Not a performance item.** fastrender's process split *is* their trust boundary — see [the concurrency model](TODO-fastrender-what-we-can-learn.md#5a-their-concurrency-model-properly). Ours would be too |

**CSP is deliberately absent.** It asks the browser to enforce what a page declares about
itself, which protects the page's authors from their own XSS and does nothing for the browser's
user. Items 1 and 3 protect the user, cost a fraction as much, and are things a page cannot
opt out of.
