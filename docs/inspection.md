# Website and document inspection

Chuzz exposes one local control surface for a page: `blitz.agent.control` and `blitz.diagnostics`, carried as MCP (Model Context Protocol) `tools/call` frames over a Unix socket. The same vocabulary serves the window (`chuzz-gui` through izumo) and the windowless host (`chuzz-headless`). `ps-qa` and `chuzz-inspect` are clients. They do not link the renderer.

One headless process hosts one page. The process prints its descriptor path on stdout when the socket is listening, then serves until it exits. Read that line. A descriptor looked up by newest file can belong to another process.

## What is on master

| Piece | Where |
| --- | --- |
| Inspect, Act, Navigate, Condense, Relaunch, Quit | chuzz master, protocol from ps-observability |
| Snapshot, Capture, WindowComposition, WebGpuInspect, WebGpuCapture on the headless host | chuzz master |
| Evaluate, Metrics, WaitForIdle on the headless host | chuzz master `fefbea4` |
| `evaluate` in the protocol and `ps-qa eval` | ps-observability master `d2b4551` and `ac0349c`, local and unpublished. chuzz master reaches them only through the path patch in `.cargo/local-engine.toml`; the published `blitz-control-protocol` 0.6.2 has no `Evaluate`. |
| Page targets, `listTargets`, the diagnostic journal, `ps-qa targets`, `page-targets`, `headless-target`, `inspection-off` | `~/code/wt-pt` worktrees only, see [Page targets](#page-targets) |

## Enabling

### GUI

Inspection is off until something turns it on. The choice is stored in `~/Library/Application Support/ai.chuzz.browser/diagnostics.json`:

```json
{
  "inspection": true,
  "profiling": false
}
```

A missing, unreadable, or malformed file means both switches are off. Settings writes the file only after the runtime accepts the change. Turning inspection off unbinds the socket and deletes the descriptor.

`CHUZZ_CONTROL` set to `1`, `true`, or `on` forces inspection on for that process. The Settings switch cannot turn it off while the variable is set, and the variable cannot force inspection off. Deep profiling still follows the stored file. The runtime refuses profiling while inspection is off.

The GUI build enables izumo `agent-control` and `diagnostics`, so `tools/list` advertises both tools. The socket lives only while inspection is on.

A second, non-MCP path photographs the window as painted. Start the GUI with `TAURI_BLITZ_DRIVER=127.0.0.1:0` and `TAURI_BLITZ_DRIVER_DESCRIPTOR` pointing at a JSON file, then run `chuzz-inspect --driver <path> screenshot out.png`. That driver is separate from `blitz.diagnostics` `capture`, which renders the document offscreen.

### Headless

```sh
chuzz-headless /path/to/dist
chuzz-headless /path/to/page.html
chuzz-headless https://example.com/
QA_INSPECT_PAGE=/path/to/dist chuzz-headless
```

`CHUZZ_HEADLESS_MODE` is `qa` when unset, or `bot`. Any other value makes the process exit with an error. QA is the permissive host `ps-qa` drives. Bot is a short-lived child with a fixed budget and a public-HTTPS target rule.

Stdout is the descriptor path, one line. Status and errors go to stderr. `CHUZZ_TRACE` adds millisecond traces on stderr.

QA reads three optional inputs:

| Variable | Meaning |
| --- | --- |
| `QA_INSPECT_PAGE` | Page when no positional argument is given. An argument wins. |
| `QA_INSPECT_STORAGE` | Path to a JSON object of string values. Installed into `localStorage` for the page origin before page scripts. Missing, unreadable, or invalid prints `chuzz-headless: QA_INSPECT_STORAGE: ...` and exits 2. |
| `QA_HOST_PATH` | Route appended to a directory origin, such as `spec`, so a built single-page app can open somewhere other than `/`. |
| `QA_HOST_WIDTH`, `QA_HOST_HEIGHT` | Viewport in CSS pixels. Defaults are 1344 and 900. Each value must be a positive integer. |
| `QA_HOST_SETTLE_MS` | Per-request settle budget. Default 100. Values above 1000 are clamped to 1000. |
| `CHUZZ_DEBUG_PRELUDE` | Script file run before the page's own scripts. Ignored for a bot child. |

The viewport is CSS pixels at scale 1, dark color scheme. QA opens the profile cookie jar. A top-level load that fails in QA mode keeps the socket up and shows a failure page, so the caller can read the error or `navigate` elsewhere.

Bot mode is the launch in `apps/chuzz/headless/README.md`:

```sh
CHUZZ_HEADLESS_MODE=bot \
CHUZZ_HEADLESS_WALL_TIMEOUT_MS=20000 \
CHUZZ_HEADLESS_ALLOWED_DOMAINS=supplier.example \
CHUZZ_HEADLESS_ALLOW_FORM_SUBMIT=true \
chuzz-headless https://supplier.example/search
```

`CHUZZ_HEADLESS_WALL_TIMEOUT_MS` is a positive integer up to 20000. The default is 20000. Crossing it prints `chuzz-headless: bot child reached its wall-time limit` and exits 124. `CHUZZ_HEADLESS_ALLOW_FORM_SUBMIT` accepts `1`, `true`, `yes`, `0`, `false`, `no`. The default is false. `CHUZZ_HEADLESS_ALLOWED_DOMAINS` is a comma-separated list of DNS hostnames. Each entry allows that host and its subdomains. An empty value leaves every public host allowed. Local names, credentials, and non-public addresses stay blocked either way.

Bot mode keeps cookies and script storage in the process. It does not install `QA_INSPECT_STORAGE`. It does not open the profile cookie jar. `tools/list` omits `blitz.diagnostics`.

## Socket

The listener is a Unix socket. The parent directory is mode `0700`, created before the socket. The socket and the descriptor are mode `0600`. There is no session and no token. The directory mode is the access check.

The descriptor is `$TMPDIR/tauri-blitz-agent/<instance-id>.json`. The socket is the same path with `.sock`. Dead descriptors are removed when a new server starts. The file looks like this:

```json
{
  "protocolVersion": 1,
  "pid": 12345,
  "instanceId": "instance-id",
  "address": "unix:///tmp/tauri-blitz-agent/instance-id.sock",
  "renderer": "blitz",
  "rendererRevision": "0.1.0"
}
```

Connect to the path in `address` after the `unix://` prefix. Headless `serverInfo.name` is `chuzz-headless`. The GUI server name is the izumo package name. `serverInfo.version` is that crate's package version.

Frames are length-delimited JSON text. A frame larger than 16 MiB is rejected. `encode_response` also refuses a reply whose text would exceed that cap (`PayloadTooLarge`). Capture leaves about 64 KiB of that budget for the JSON around the base64 pixels.

Several clients may connect. A slow reader does not stall the others. Observation uses a latest-value slot, so a slow client sees the newest event rather than a backlog.

`chuzz-inspect` finds a GUI descriptor by newest JSON file under the temp `tauri-blitz-agent` directory, or `--descriptor <path>`. `TAURI_BLITZ_CONTROL_DESCRIPTOR` names one file and the client searches that file's directory. Headless automation should use the stdout path. `ps-qa` does that when it launches the host.

## MCP tools/call envelope

Handshake, tool list, and calls are JSON-RPC 2.0. `initialize` and `tools/list` need an id. `notifications/initialized` must not have one.

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "initialize",
  "params": {}
}
```

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": {
    "protocolVersion": "2024-11-05",
    "capabilities": { "tools": { "listChanged": false } },
    "serverInfo": { "name": "chuzz-headless", "version": "0.1.0" }
  }
}
```

The `protocolVersion` string is endpoint-libs' MCP version. The descriptor's `protocolVersion` number is the Blitz debug protocol, currently `1`.

```json
{ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }
```

The result lists `blitz.agent.control` and, when the host advertises diagnostics, `blitz.diagnostics`. Each tool carries a JSON Schema of its request enum. A build that cannot collect diagnostics omits the second tool. Bot mode does the same.

A call wraps the typed request in `arguments`:

```json
{
  "jsonrpc": "2.0",
  "id": 3,
  "method": "tools/call",
  "params": {
    "name": "blitz.agent.control",
    "arguments": {
      "command": "inspect",
      "params": { "root": null, "max_depth": 40 }
    }
  }
}
```

The reply puts the typed payload in `structuredContent`. The `content` array is a one-line summary. Read `structuredContent`.

```json
{
  "jsonrpc": "2.0",
  "id": 3,
  "result": {
    "content": [{ "type": "text", "text": "semantic snapshot with 2 nodes" }],
    "structuredContent": {
      "result": "agentSnapshot",
      "value": {
        "revision": 1,
        "activeWindow": null,
        "focusedNode": null,
        "viewport": [0.0, 0.0, 1344.0, 900.0],
        "nodes": [
          {
            "id": 9,
            "parent": null,
            "role": "button",
            "name": "Search",
            "value": null,
            "enabled": true,
            "focusable": true,
            "viewportFixed": false,
            "visible": true,
            "selected": false,
            "bounds": [16.0, 24.0, 88.0, 32.0],
            "domId": "search",
            "slot": "button"
          }
        ]
      }
    }
  }
}
```

`isError` is present only when the payload is an error:

```json
{
  "jsonrpc": "2.0",
  "id": 4,
  "result": {
    "content": [{ "type": "text", "text": "form submission requires explicit permission from the caller" }],
    "structuredContent": {
      "result": "error",
      "value": {
        "code": "policyDenied",
        "message": "form submission requires explicit permission from the caller"
      }
    },
    "isError": true
  }
}
```

A malformed frame is a JSON-RPC error on the caller's id, code `-32600`. An error with a null id is easy to miss: the client matches replies by id. `tools/call` without an id is rejected. An unknown tool name is rejected. Notifications that share the socket are not replies. Ignore frames whose id is not the one you sent.

Events are notifications, with no id:

```json
{
  "jsonrpc": "2.0",
  "method": "notifications/blitz/diagnostics",
  "params": { "event": "paintCommitted", "value": { "revision": 3 } }
}
```

Agent notifications use `notifications/blitz/agent` and the same `event` / `value` shape: `ready`, `treeChanged` (`{"revision": 12}`), and `lifecycle` (`relaunching`, `ready`, `quitting`).

## Wire shape

Command enums are adjacently tagged. The tag is camelCase. Fields inside those enums stay snake_case. `{"action":"click","node_id":9}` does not decode. The node id belongs under `params`.

Structs that declare their own camelCase rename do use it. Snapshot and capture fields are camelCase (`includeDom`, `nodeId`). Semantic nodes use `domId`, `focusedNode`, `viewportFixed`. Condense option fields are `previewNodes`, `summarySentences`, `maxKeywords`.

Boxes on a semantic node are `[x, y, width, height]`. A layout row uses the same array shapes: `bounds` and `border` and `padding` are four numbers (edges are top, right, bottom, left), and sizes and scroll offsets are two numbers. `nodeId` on a layout row is camelCase because that row is a struct.

A unit command is `{"command":"metrics"}` with no `params`. A unit result is `{"result":"ack"}` with no `value`. `max_depth` of `0` means the whole tree inside the document inspector.

## Agent requests

Tool name: `blitz.agent.control`. Examples below are the `arguments` object.

### inspect

Semantic tree, including shadow trees flattened into it on the headless host. `root` null is the document. `max_depth` 0 walks the whole tree. Bot mode reduces a depth above 12 to 12, and leaves 0 as 0.

```json
{ "command": "inspect", "params": { "root": null, "max_depth": 40 } }
```

```json
{ "command": "inspect", "params": { "root": 9, "max_depth": 2 } }
```

Result `agentSnapshot`. `bounds` is omitted when the node has no box. `domId` and `slot` are omitted when absent. `viewport` is `[x, y, width, height]` in CSS pixels.

### act

```json
{ "command": "act", "params": { "action": "focus", "params": { "node_id": 9 } } }
```

```json
{ "command": "act", "params": { "action": "click", "params": { "node_id": 9 } } }
```

```json
{ "command": "act", "params": { "action": "doubleClick", "params": { "node_id": 9 } } }
```

```json
{ "command": "act", "params": { "action": "hover", "params": { "node_id": 9 } } }
```

```json
{
  "command": "act",
  "params": { "action": "setValue", "params": { "node_id": 12, "value": "fixture" } }
}
```

```json
{ "command": "act", "params": { "action": "scrollIntoView", "params": { "node_id": 13 } } }
```

```json
{
  "command": "act",
  "params": { "action": "scrollBy", "params": { "node_id": 14, "delta_x": 0.0, "delta_y": -200.0 } }
}
```

```json
{
  "command": "act",
  "params": {
    "action": "input",
    "params": {
      "input": "key",
      "phase": "down",
      "key": "Enter",
      "code": "Enter",
      "modifiers": { "shift": false, "control": false, "alt": false, "meta": true }
    }
  }
}
```

Send `phase` `up` as a second call. `chuzz-inspect key` does down then up.

```json
{
  "command": "act",
  "params": {
    "action": "input",
    "params": {
      "input": "pointer",
      "phase": "down",
      "x": 40.0,
      "y": 28.0,
      "button": 0,
      "modifiers": { "shift": false, "control": false, "alt": false, "meta": false }
    }
  }
}
```

Pointer phases are `move`, `down`, `up`, `cancel`.

```json
{
  "command": "act",
  "params": {
    "action": "input",
    "params": {
      "input": "wheel",
      "delta_x": 0.0,
      "delta_y": 80.0,
      "phase": "moved",
      "modifiers": { "shift": false, "control": false, "alt": false, "meta": false }
    }
  }
}
```

Wheel phases are `started`, `moved`, `ended`.

`click` dispatches pointer, mouse, and click on the node. `focus` does not activate it. `setValue` replaces the field and does not submit it. A successful act returns `{"result":"ack"}` after the headless host settles the page.

Bot mode allows `click`, `setValue`, `scrollIntoView`, and `scrollBy`. Every other action returns `policyDenied` with `this action is not part of the bot browser control surface`.

### navigate

Resolved against the standing document URL. The headless host loads the result in the same process.

```json
{ "command": "navigate", "params": { "url": "/spec" } }
```

```json
{ "command": "navigate", "params": { "url": "https://example.com/next" } }
```

Success is `ack`. An unresolvable URL is `invalidArgument`. A load that fails is `navigationFailed`. The GUI runtime returns `unsupportedRequest` for this command.

### condense

One level of the settled tree. The default level is `keywords`. Default options are 40 preview nodes, 4 summary sentences, and 24 keywords. Widening is another call. Bot mode returns `policyDenied`, `page condensation is disabled in bot-child mode`. The GUI runtime returns `unsupportedRequest`.

```json
{
  "command": "condense",
  "params": {
    "level": "preview",
    "options": { "previewNodes": 40, "summarySentences": 4, "maxKeywords": 24 }
  }
}
```

Levels: `keywords`, `summary`, `preview`, `structure`, `content`.

```json
{
  "result": "condensation",
  "value": {
    "level": "keywords",
    "value": {
      "value": [{ "term": "search", "score": 15 }],
      "dropped": {
        "nodesKept": 2,
        "nodesPresent": 40,
        "bytesKept": 12,
        "bytesPresent": 800
      }
    }
  }
}
```

`summary` carries `value` as an array of sentences. `preview` carries `{id, role, name}` rows. `structure` carries the same fields plus `children`. `content` carries one string. Each level includes `dropped`.

### relaunch and quit

```json
{ "command": "relaunch" }
```

```json
{ "command": "quit" }
```

The GUI answers both. `quit` returns `ack` and exits the event loop. `relaunch` restarts the process, or delegates to a lifecycle handler the embedder registered, and returns `ack` or `relaunchFailed`. The headless host returns `unsupported` with `the headless page does not handle this request`.

## Diagnostics requests

Tool name: `blitz.diagnostics`. Bot mode does not advertise the tool. A call that arrives anyway returns `unsupported` with `the bot browser page serves no diagnostics`.

### observe

Answered by the socket, before the page. Success is `ack`. The connection then receives later events whose stream was named. The cursor starts at the current latest event, so a paint that already happened is not replayed.

```json
{ "command": "observe", "params": { "streams": ["paint", "console"] } }
```

Stream names: `snapshots`, `metrics`, `console`, `runtimeErrors`, `paint`.

Event payloads:

```json
{ "event": "paintCommitted", "value": { "revision": 4 } }
```

```json
{ "event": "console", "value": { "sequence": 1, "level": "error", "message": "boom" } }
```

```json
{
  "event": "runtimeError",
  "value": {
    "sequence": 2,
    "message": "TypeError",
    "stack": "",
    "revisions": { "document": 3, "style": 0, "layout": 0, "paint": 0 }
  }
}
```

`snapshot` and `metrics` events use the same `value` as the matching request results. The GUI and the headless host currently publish `paintCommitted` only. A `console` or `runtimeErrors` subscription is accepted and then stays quiet on these hosts.

### snapshot

```json
{
  "command": "snapshot",
  "params": {
    "includeDom": true,
    "includeLayout": true,
    "includeComputedStyle": false,
    "nodeIds": [9, 12]
  }
}
```

Omit `nodeIds` or pass `[]` for the whole document. `includeComputedStyle` adds the legibility properties only.

On the headless host, `includeDom: false` still fills `dom` with the committed TLS state of the loaded URL. That object is the TLS inspection record, not a DOM tree. The GUI uses `includeDom: false` to skip DOM collection.

```json
{
  "result": "snapshot",
  "value": {
    "revisions": { "document": 2, "style": 0, "layout": 0, "paint": 0 },
    "activeWindow": null,
    "activeElement": 12,
    "dom": null,
    "layout": [
      {
        "nodeId": 12,
        "bounds": [10.0, 20.0, 200.0, 32.0],
        "scrollOffset": [0.0, 0.0],
        "clientSize": [200.0, 32.0],
        "scrollSize": [200.0, 32.0],
        "scrollRange": [0.0, 0.0],
        "border": [0.0, 0.0, 0.0, 0.0],
        "padding": [4.0, 8.0, 4.0, 8.0],
        "contentSize": [184.0, 24.0]
      }
    ],
    "metrics": {
      "revisions": { "document": 2, "style": 0, "layout": 0, "paint": 0 },
      "queueDepth": null,
      "invalidationsCoalesced": 0
    }
  }
}
```

Unmeasured metric fields are omitted rather than sent as zero. `style`, `layout`, and `paint` revisions stay 0 because Blitz versions the document as one number.

### metrics

```json
{ "command": "metrics" }
```

Result `metrics`. The GUI fills frame, script, and snapshot costs from the running renderer. Empty `Option` fields mean the runtime did not measure them. Headless returns document revision equal to its paint counter, `queueDepth` of 0 or 1 from whether script jobs are pending, and the other fields at their empty defaults.

### waitForIdle

```json
{ "command": "waitForIdle" }
```

Result `idle`, value a revision set. Headless drains pending jobs for at most 5 seconds, then answers with whatever revision it has. The GUI collects one diagnostic snapshot and returns that revision set.

### windowComposition

```json
{ "command": "windowComposition" }
```

```json
{
  "result": "windowComposition",
  "value": {
    "supported": true,
    "surfaceTransparent": true,
    "glassEnabled": true,
    "glassBackend": "nativeGlass",
    "tintRgba": [174, 50, 112, 0],
    "radius": 12.0
  }
}
```

Headless has no window. It returns the default: `supported` false and the other flags false, with the optional fields omitted. That default is what lets a spill check run without a display.

### capture

Offscreen RGBA8 of the document, or of one node. `scale` must be finite and from 0.25 through 8.

```json
{ "command": "capture", "params": { "nodeId": 9, "scale": 2.0 } }
```

```json
{
  "result": "captured",
  "value": {
    "width": 176,
    "height": 64,
    "rgbaBase64": "AAAA",
    "nodeId": 9
  }
}
```

`nodeId` is omitted when the request named the whole document. A scale outside the range is `invalidArgument`. An image that cannot fit in the 16 MiB frame is `captureTooLarge`.

### webGpuInspect

Renews a ten-second lease on pass and draw bookkeeping.

```json
{ "command": "webGpuInspect" }
```

```json
{
  "result": "webGpu",
  "value": {
    "devices": [],
    "resources": [],
    "errors": { "validation": 0, "outOfMemory": 0, "internal": 0, "unknown": 0, "messages": [] },
    "frames": []
  }
}
```

Resource `details` is tagged by `kind`: `buffer`, `texture`, `sampler`, `shader-module`, `render-pipeline`, `compute-pipeline`, `bind-group`, `bind-group-layout`, `pipeline-layout`. A host with no page WebGPU inspector returns its unsupported error. The GUI runtime returns `unsupportedRequest`.

### webGpuCapture

`target` is `texture:<label>` or `canvas:#id`. The result is `captured`, the same image as document capture.

```json
{ "command": "webGpuCapture", "params": { "target": "canvas:#gpu" } }
```

```json
{ "command": "webGpuCapture", "params": { "target": "texture:scene-color" } }
```

### evaluate

Runs in the page. The completion value comes back as JSON.

```json
{ "command": "evaluate", "params": { "script": "document.title" } }
```

```json
{ "result": "evaluated", "value": "Example" }
```

A thrown script is `scriptError`. The GUI runtime returns `unsupportedRequest`.

## Where each request is answered

| Request | QA headless | Bot child | GUI |
| --- | --- | --- | --- |
| inspect, click, setValue, scrollIntoView, scrollBy | yes | yes, with the bot policy | yes |
| focus, doubleClick, hover, input | yes | policyDenied | yes |
| navigate, condense | yes | navigate under the URL policy; condense denied | unsupportedRequest |
| relaunch, quit | unsupported | unsupported | yes |
| observe | ack, then paint events | tool omitted | ack, then paint events |
| snapshot, metrics, waitForIdle, capture, windowComposition | yes | tool omitted | yes |
| webGpuInspect, webGpuCapture, evaluate | yes | tool omitted | unsupportedRequest |

Headless `metrics` and `windowComposition` are the reduced values above. A QA load failure stays inspectable. A bot load failure ends the process. After a bot failure, a later `navigate` returns `policyDenied`, `a failed bot page cannot restart its browser budgets`.

## Limits

These bot numbers are fixed in the host.

| Limit | Value |
| --- | --- |
| Wall time | 20 s, exit 124. `CHUZZ_HEADLESS_WALL_TIMEOUT_MS` may only lower it. |
| Control requests | 24. The request that crosses the cap returns `policyDenied` and the process stops. |
| Inspections (`inspect`) | 16, inside the 24. |
| Actions (`act`) | 8, inside the 24. |
| Queued requests | 16. QA allows 64. Overflow is `documentBusy`, `the inspection request queue is full`, and the process keeps serving. |
| Inspect depth | A requested depth above 12 becomes 12. Depth 0 stays 0, which the inspector treats as the whole tree. |
| Returned semantic nodes | 512. |
| Snapshot wire size | 60 KiB. Nodes are dropped from the end until the JSON fits. |
| Field trims inside that budget | Active window 128, DOM id 128, role 64, name 512, value 512, slot 128, counted in bytes. |
| Live DOM nodes | 20,000. Crossing it on load fails the load. Crossing it later stops the loop. |
| Fill value | 512 bytes. |
| Viewport | At most 1600 by 1000 and 1,500,000 pixels. QA defaults to 1344 by 900 and has no pixel cap. |
| Egress tunnels | 16. A further tunnel gets HTTP 503. |
| Bytes through the egress filter | 32 MiB. |
| Proxy header | 16 KiB. |

The egress filter accepts HTTPS `CONNECT` only, checks the destination, and connects to a resolved public address. Any other request is HTTP 403. File and data resource URLs do not pass through that filter, and the engine disk cache is not disabled in this mode. Those two provider behaviors need a separate check before bot mode is treated as a complete network boundary.

Bot navigation must be HTTPS, with a host, with no username or password. `localhost`, `*.localhost`, `*.local`, `*.internal`, and `*.lan` are rejected. Every resolved address must be public, including IPv4-mapped IPv6. A redirect that lands outside the policy fails the load. Link and form-action URLs on a click are checked the same way.

A click is refused when the control, an ancestor within 16 levels, or a short sample of descendant text looks like a credential, identity, or payment surface. The match is on whole words: sign-in, login, password, payment, checkout, reservation, purchase, register, billing, email, username, phone, passport, OTP, card verification, book, booking, and the pairs and triples around "sign in", "pay now", "place order", "create account", "credit card", "first name", "date of birth", and "one time code". The same check clears `value` on those nodes in a snapshot. Downloads and `target="_blank"` are refused. A submit control inside a form needs `CHUZZ_HEADLESS_ALLOW_FORM_SUBMIT`. `setValue` is limited to `textarea`, `select`, and `input` of type empty, text, search, date, month, week, time, or number.

QA deadlines, outside the bot budget: a control request and a local load have 10 seconds, a remote load 60 seconds, a script poll, resolve, or action 15 seconds, storage 2 seconds. Each control then settles for `QA_HOST_SETTLE_MS`. The page also receives idle turns of 8 ms. After a reply the host catches up at most 64 of those turns, and at most 8 ms of wall time, so a slow snapshot cannot freeze the next request. `ps-qa` itself defaults to a 15 s request timeout (`--request-timeout-ms` or `PS_QA_REQUEST_TIMEOUT_MS`, from 1 to 300000). Check budgets can raise that transport limit. `--timeout-scale` or `QA_TIMEOUT_SCALE` multiplies interaction deadlines from 1 to 10.

A page that stops answering inside a supervisor deadline returns `pageUnresponsive`.

## ps-qa

`ps-qa` attaches to a descriptor and judges the renderer. Output is TOON. `--descriptor` selects the file. Otherwise it tries `target/blitz-control.json`, then the newest advertised descriptor. `--app` is the application profile (`ps-qa.ron` in the working directory). Sweeps, cover, inventory, `qa`, `run-plan`, `qa-hosted`, and `sweep-components` require that profile. Renderer readings such as `tree`, `metrics`, and `capture` do not.

`--headless` and `PS_QA_HEADLESS` do not launch `chuzz-headless`. They allow a click by node id when the control was laid out with no painted height, which is what a host without fonts does. Visual checks are still meaningless there. `--require-paint-events` fails pixel checks when the runtime never emits `paintCommitted`. `--trace` names the node each step addressed. `--pace` is the delay between injected events, default one frame. `--pixel-artifact-dir` keeps before and after captures for a failed pixel check.

Commands that launch a page:

```sh
ps-qa qa-hosted --host chuzz-headless --page dist/ --checks tests/ps-qa
ps-qa sweep-components --host chuzz-headless --dists dist/components --checks tests/ps-qa
ps-qa run-plan plan.ron
```

`qa-hosted` is one page. `sweep-components` is one process per component, defaulting to a fresh host per check (`--mode isolated`). `--mode sweep` shares one host across the component's checks. Both wait for the descriptor line, default startup timeout 30 seconds. The host is started with `QA_INSPECT_PAGE` set to the page path and no positional argument. A check that declares storage is written to a JSON file and passed as `QA_INSPECT_STORAGE`. An invalid seed exits the host with status 2, so the check cannot pass on an empty store. `run-plan` launches the configured host from a RON plan, clears the environment, and points `TMPDIR` at a private directory so the socket stays there.

Useful readings against a process that is already up:

| Command | What it asks for |
| --- | --- |
| `tree`, `find`, `layout`, `nodes`, `panes` | `inspect` |
| `metrics`, `frames`, `idle`, `drift`, `blink` | `metrics`, sometimes `observe` |
| `watch` | `observe` on metrics, console, and runtime errors |
| `dom`, `paint`, `contrast`, `spill`, `transcript` | `snapshot` |
| `eval <script>` | `evaluate` |
| `capture`, `gpu`, `gpu-capture` | `capture`, `webGpuInspect`, `webGpuCapture` |
| `click`, `press`, `scroll`, `drag`, `type`, `key`, `reveal` | `act` |
| `audit` | inspect and capture, no clicks |
| `qa`, `sweep`, `cover`, `inventory` | drive the profile's controls |

`chuzz-inspect` is the small client for a running window: `tree`, `nodes`, `find`, `click`, `press`, `set-value`, `type`, `key`, `overlap`, and `raw <json>` on the agent tool. `console`, `metrics`, `settle`, `dom`, and `diag <json>` call the diagnostics tool. `console` sends `observe` for `console` and `runtimeErrors` and prints the ack. It does not stay to print events.

## Page targets

Page targets are not on chuzz master yet. They live in the pagetarget worktrees under `~/code/wt-pt`: the protocol and `ps-qa` commands in ps-observability `9603f6f`, tab routing in izumo `91712dc`, bounded console and error batches in ps-blitz `1fa32759`, and the GUI and headless hosts plus `chuzz-inspect --tab-id` in chuzz `2ee8a62`. The four land together, protocol first. A request with none of the target keys keeps the shape described above, except `listTargets`, which is always treated as targeted.

Targets are the browser chrome or one tab. A target id is the JSON string `"chrome"` or a tab number. A target selector is `"chrome"` or `{"tabId": 7}`.

These fields sit beside `command` and `params`:

| Field | Meaning |
| --- | --- |
| `target` | `"chrome"` or `{"tabId": N}`. Absent means the host default. |
| `expectedGeneration` | Mount generation the caller last observed. |
| `expectedDocumentId` | Document id the caller last observed. |
| `allTargets` | Observe every target. Cannot be combined with `target`. |
| `afterSequence` | Deliver diagnostics events after this journal sequence. |

```json
{
  "target": { "tabId": 7 },
  "expectedGeneration": 4,
  "expectedDocumentId": 9,
  "command": "act",
  "params": { "action": "click", "params": { "node_id": 12 } }
}
```

```json
{ "command": "listTargets" }
```

```json
{
  "command": "webGpuCapture",
  "target": { "tabId": 7 },
  "params": { "target": "canvas:#gpu" }
}
```

`listTargets`, `quit`, `relaunch`, and `windowComposition` are instance-scoped. They do not need a tab generation. Any other request that names nodes does, when the target is a tab: `inspect` with a `root`, every `act` except `input`, `snapshot` with a non-empty `nodeIds`, and `capture` with a `nodeId`. Missing either expected id is `targetIdentityRequired`. A generation mismatch is `staleGeneration`. A document mismatch is `staleDocument`.

`allTargets` and `afterSequence` are valid only on `observe`. Using them elsewhere is `invalidArgument`. Combining `allTargets` with `target` is `invalidArgument`. An `afterSequence` past the current journal is `invalidSequence`.

A successful targeted reply adds `context` next to the existing `result` and `value`. An error is not wrapped.

```json
{
  "result": "ack",
  "context": { "targetId": 7, "generation": 4, "documentId": 9 }
}
```

```json
{
  "result": "targets",
  "value": [
    {
      "id": "chrome",
      "url": "",
      "title": "Chrome",
      "active": true,
      "generation": 1,
      "capabilities": {
        "agent": true,
        "diagnostics": true,
        "capture": false,
        "console": false,
        "runtimeErrors": false,
        "webGpu": false
      }
    },
    {
      "id": 7,
      "url": "https://example.com/",
      "title": "Example",
      "active": true,
      "generation": 4,
      "documentId": 9,
      "capabilities": {
        "agent": true,
        "diagnostics": true,
        "capture": true,
        "console": true,
        "runtimeErrors": true,
        "webGpu": true
      }
    }
  ]
}
```

`documentId` is omitted when the target has no document. Capability flags tell the caller what that target can answer.

Observed diagnostics become a journal. Each notification flattens the target into the event:

```json
{
  "jsonrpc": "2.0",
  "method": "notifications/blitz/diagnostics",
  "params": {
    "targetId": 7,
    "generation": 4,
    "documentId": 9,
    "sequence": 12,
    "event": "paintCommitted",
    "value": { "revision": 3 }
  }
}
```

A gap in that journal is reported as `diagnosticOverflow` with `firstSequence` and `lastSequence`, on the same notification method.

### How each host serves targets

- **Headless** (chuzz `2ee8a62`, `serve.rs`): the one page is tab `0`. `listTargets` returns a single entry with `active: true`, `generation` built from the process id and the document id, and capabilities `agent: true`, `diagnostics`, `capture` and `webGpu` true outside bot mode, `console` and `runtimeErrors` false. Any other `target` is `unknownTarget` (`headless exposes only page 0`). A targeted `observe` is acknowledged with a context and delivers nothing.
- **GUI** (izumo `91712dc`, chuzz `2ee8a62` `browser.rs`): every tab is a target with its own `mounted_generation`, bumped each time a new document mounts in it. Agent and diagnostics requests run against the addressed tab's mounted document. Capability flags are all true when the tab runs script and all false otherwise. Omitting `target` keeps the old default, the chrome document.
- **Journal**: izumo keeps 4096 diagnostic records across targets. Each page script runtime keeps at most 1000 console entries and 1000 runtime errors, read in bounded batches that report how many were lost (ps-blitz `1fa32759`). A lost range shows up as `diagnosticOverflow`.
- **ps-qa**: `--tab-id N` or `--chrome` selects a target for any command, and node guards are copied from the inspection it made. `ps-qa targets` lists targets. `page-targets <url>` drives the fixture in `apps/chuzz/tests/page-targets/` through two GUI tabs, navigation and closure. `headless-target` checks the headless single-page discovery and legacy default. `inspection-off` turns inspection off in Settings and asserts the socket and descriptor are gone.
