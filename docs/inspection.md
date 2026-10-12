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
  "protocolVersion": 2,
  "pid": 12345,
  "instanceId": "instance-id",
  "address": "unix:///tmp/tauri-blitz-agent/instance-id.sock",
  "renderer": "blitz",
  "rendererRevision": "0.1.0"
}
```

Connect to the path in `address` after the `unix://` prefix. Headless `serverInfo.name` is `chuzz-headless`. The GUI server name is the izumo package name. `serverInfo.version` is that crate's package version.

Frames are length-delimited JSON text. A frame larger than 16 MiB (`MAX_DEBUG_FRAME_BYTES`) is rejected. The headless supervisor uses the same constant on its worker link, so nothing is accepted on one hop and refused on the next. 16 MiB fits the largest real payload, a capture, which is itself capped to fit. `encode_response` also refuses a reply whose text would exceed that cap (`PayloadTooLarge`). Capture leaves about 64 KiB of that budget for the JSON around the base64 pixels.

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
      "params": { "root": null, "maxDepth": 40 }
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

Command enums are adjacently tagged. Tags and fields are camelCase (protocol 2). Hosts still accept the protocol 1 snake_case field names (`node_id`, `max_depth`, `delta_x`, `delta_y`) on input, so older clients keep working; replies and events are camelCase only. A protocol 2 client refuses a descriptor that reports protocol 1. `{"action":"click","nodeId":9}` does not decode. The node id belongs under `params`.

Structs that declare their own camelCase rename do use it. Snapshot and capture fields are camelCase (`includeDom`, `nodeId`). Semantic nodes use `domId`, `focusedNode`, `viewportFixed`. Condense option fields are `previewNodes`, `summarySentences`, `maxKeywords`.

Boxes on a semantic node are `[x, y, width, height]`. A layout row uses the same array shapes: `bounds` and `border` and `padding` are four numbers (edges are top, right, bottom, left), and sizes and scroll offsets are two numbers. `nodeId` on a layout row is camelCase because that row is a struct.

A unit command is `{"command":"metrics"}` with no `params`. A unit result is `{"result":"ack"}` with no `value`. `maxDepth` of `0` means the whole tree inside the document inspector.

## Agent requests

Tool name: `blitz.agent.control`. Examples below are the `arguments` object.

### inspect

Semantic tree, including shadow trees flattened into it on the headless host. `root` null is the document. `maxDepth` 0 walks the whole tree. Bot mode reduces a depth above 12 to 12, and leaves 0 as 0.

```json
{ "command": "inspect", "params": { "root": null, "maxDepth": 40 } }
```

```json
{ "command": "inspect", "params": { "root": 9, "maxDepth": 2 } }
```

Result `agentSnapshot`. `bounds` is omitted when the node has no box. `domId` and `slot` are omitted when absent. `viewport` is `[x, y, width, height]` in CSS pixels.

### act

```json
{ "command": "act", "params": { "action": "focus", "params": { "nodeId": 9 } } }
```

```json
{ "command": "act", "params": { "action": "click", "params": { "nodeId": 9 } } }
```

```json
{ "command": "act", "params": { "action": "doubleClick", "params": { "nodeId": 9 } } }
```

```json
{ "command": "act", "params": { "action": "hover", "params": { "nodeId": 9 } } }
```

```json
{
  "command": "act",
  "params": { "action": "setValue", "params": { "nodeId": 12, "value": "fixture" } }
}
```

```json
{ "command": "act", "params": { "action": "scrollIntoView", "params": { "nodeId": 13 } } }
```

```json
{
  "command": "act",
  "params": { "action": "scrollBy", "params": { "nodeId": 14, "deltaX": 0.0, "deltaY": -200.0 } }
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
      "deltaX": 0.0,
      "deltaY": 80.0,
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

Unmeasured metric fields are omitted rather than sent as zero. On the headless host `style` and `layout` count the Stylo traversals and layout passes blitz-dom actually ran (an idle resolve leaves them alone), and `paint` counts committed renders. The GUI host still reports 0 for `style`, `layout` and `paint` until izumo reads the same counters.

### metrics

```json
{ "command": "metrics" }
```

Result `metrics`. The GUI fills frame, script, and snapshot costs from the running renderer. Empty `Option` fields mean the runtime did not measure them. Headless returns `document` and `paint` equal to its committed-render count, real `style` and `layout` revisions, and `queueDepth` as the page's pending task count: every due timer and animation-frame callback, plus one while any promise job is runnable (Boa exposes only whether its job queues are empty). Other fields keep their empty defaults.

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
  "params": { "action": "click", "params": { "nodeId": 12 } }
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

## Driver plane (TAURI_BLITZ_DRIVER)

The driver plane is a second control socket. `chuzz_control::driver` reads pixels from it. `crate::client` reads the document tree and sends clicks and typing on another socket.

The listener is `blitz_script::DebugController`. It owns a `blitz_debug_control::DebugServer`. They do not contain `DebugServer`'s HTTP router, session table, token generator, or descriptor writer.

Izumo starts it from `run` when the crate is built with `debug-control`. `DebugController::start_from_env` gets the izumo package version as `renderer_revision`. The controller is installed with `set_debug_controller`.

### Driver screenshot and diagnostics capture

`GET screenshot` is handled by `DebugController::screenshot`. It calls `wait_for_idle`, then returns `latest_screenshot`. The paint path is `commit_cpu_frame`. That uses `VelloCpuImageRenderer` and `blitz_paint::paint_scene_at_time`. The buffer is RGBA8. `PngEncoder` writes a PNG. The PNG is standard base64.

The CSS viewport for that paint is `TAURI_BLITZ_DRIVER_SCREENSHOT_SIZE`, or `1344` by `932`. The document scale is kept. Device width is `round(css_width * scale)` and at least 1. Device height is the same for height. The previous viewport is saved, the screenshot size is installed, the document is resolved, the PNG is painted, the previous viewport is restored, and the document is resolved again.

`paint_revision` increments by 1 after a successful encode. The screenshot success value is only the base64 string. It does not include `paintRevision`.

Diagnostics capture is a different request. On the agent-control bridge, `DiagnosticsRequest::Capture` calls `capture_image`, which calls `DocumentControl::capture`. That code is compiled for `diagnostics` on unix. The comment there says the capture uses `blitz_paint::paint_scene`, the window renderer's paint entry. A build without `diagnostics` answers that bridge with `diagnosticsUnavailable` and the message `this build has no diagnostics feature; rebuild with izumo/diagnostics`.

Agent control and the driver start in the same `run` function on separate conditions. `--blitz-control` or `set_agent_control_enabled` starts `AgentControlServer`. The driver starts only from `TAURI_BLITZ_DRIVER` plus `TAURI_BLITZ_DRIVER_DESCRIPTOR`.

### Environment variables

`TAURI_BLITZ_DRIVER` is read with `var_os`. If it is unset, `start_from_env` returns `Ok(None)` and no listener starts. If it is set, the value is parsed as `SocketAddr` after a lossy string conversion. Parse failure returns `InvalidInput` and `invalid TAURI_BLITZ_DRIVER address: {error}`. Izumo's `?` then fails `run`.

`TAURI_BLITZ_DRIVER_DESCRIPTOR` is required once the address is set. If it is missing, startup returns `InvalidInput` and `TAURI_BLITZ_DRIVER_DESCRIPTOR is required`. The value becomes `ServerConfig.descriptor_path`. The chuzz helper `descriptor_from_env` maps that same variable to a `PathBuf`. It does not look at `TAURI_BLITZ_DRIVER`.

`TAURI_BLITZ_DRIVER_SCREENSHOT_SIZE` is optional. Izumo reads it with `var`. The value must split on one `x` into two `u32` values, and both must be greater than 0. Any miss falls through. The fallback size is `1344` by `932`. That pair is passed to `with_cpu_screenshot`. A direct `DebugController::start` leaves `screenshot_size` as `None` until something calls `with_cpu_screenshot`.

The client documents this launch line:

```text
TAURI_BLITZ_DRIVER=127.0.0.1:0 \
TAURI_BLITZ_DRIVER_DESCRIPTOR=/tmp/chuzz-driver.json \
CHUZZ_CONTROL=1 target/release/chuzz-gui
```



### Transport and bind address

The contract says loopback HTTP, a session token, and a server thread that only sends requests to the UI thread. `DebugController` says the same split. The network thread produces `ControlRequest`. `service_pending` and `service_one` run on the thread that owns `ScriptDocument`. `handle` answers one request, then sends one `ControlResponse`.

`start_from_env` passes any `SocketAddr` through, but `DebugServer::start` refuses a non-loopback address with `debug control must bind to a loopback address` before it binds (`blitz-debug-control` `lib.rs`). Port 0 asks the OS for a free port. The descriptor carries the bound address.

The chuzz client opens a TCP connection to the descriptor's `address` string. It does not parse that string first. Each call is one HTTP/1.1 request with `Connection: close`. Read and write timeouts are 30 seconds. The client reads until EOF, splits headers on `\r\n\r\n`, and parses the remainder as JSON. It does not read the status code.

```text
POST /session HTTP/1.1
Host: 127.0.0.1:53177
Content-Type: application/json
Content-Length: 78
Connection: close

{"capabilities":{"alwaysMatch":{"blitz:token":"<token>"}}}
```

`Content-Length` is the byte length of `Value::to_string()`. GET and DELETE send that length as 0 and an empty body. `Content-Type` is still `application/json`.

### Descriptor

The chuzz client requires two JSON fields. `address` is a string. `token` is a string. A missing or non-string `address` fails with `the driver descriptor has no address`. The same check for `token` says `the driver descriptor has no token`.

The contract example is:

```json
{
  "pid": 12345,
  "address": "127.0.0.1:53177",
  "token": "<random 256-bit value>",
  "protocolVersion": 1,
  "renderer": "blitz",
  "rendererRevision": "<git revision>"
}
```

The contract says the file is owner-readable where modes exist, written atomically, written only after `GET /status` can succeed, and removed on clean shutdown. It says a stale file is rejected when its PID or health check does not match. The client comment calls it a `0600` descriptor. They do not show the write, the mode bits, the PID, or the health check. Izumo's `renderer_revision` is `CARGO_PKG_VERSION`, not a git revision.

### Session

`POST /session` is the chuzz create call. It is not an arm in `route`.

Request:

```json
{"capabilities":{"alwaysMatch":{"blitz:token":"<token from the descriptor>"}}}
```

The client treats a success as JSON `value.sessionId`, a string. If that string is absent, the client returns `could not open a driver session ({value}). Another client may still hold the one the browser allows; restart the browser if nothing else is connected.` `{value}` is the `value` field's display text.

`DELETE /session/{sessionId}` is sent from `release`, and `Drop` always calls `release`. The response is ignored. The client comment says the server allows one session and keeps it after the socket closes, until DELETE or process restart.

`GET /status` needs no session and answers `{"value":{"ready":true,"message":"Blitz debug control is ready","protocolVersion":1}}`. `POST /session` compares `capabilities.alwaysMatch.blitz:token` with the server token; a mismatch is `invalid argument` / `invalid blitz:token capability`. Only one session exists at a time; a second create is `session not created` / `only one session is supported`. Every `/session/{id}/...` request checks the id, and a wrong one is `invalid session id` / `session is not active`.

Later chuzz calls put the session id in the path and do not send the token again.

### Commands

`route` matches `request.method` and `request.path`. Paths below are those route keys. The chuzz client uses the full URL `/session/{sessionId}/screenshot` and `/session/{sessionId}/source`.path`.

Success values below are the `Value` inside `ControlResponse::Success`. The chuzz client reads that payload from JSON `value` for session create, screenshot, and source.

Initial counters are `document_generation` 1, `document_revision` 1, `style_revision` 0, `layout_revision` 0, `paint_revision` 0. A new document id bumps `document_generation` with wrapping add, resets those revisions to 1, 0, 0, and 0, and clears `latest_screenshot`.

`document_revision` increments by 1 after a successful action sequence, sync script, async script, element click, or send-keys.

Element references use the key `element-6066-11e4-a52e-4f735466cecf`. The value is `{document_generation}:{node_id.as_u64()}` in decimal, for example `1:42`. `resolve_element` accepts exactly two colon-separated `u64`s. The generation must equal `document_generation`. The node must be in the document. Anything else is a stale reference.

`NodeId` JSON in snapshots is whatever `serde_json` does with `NodeId`. Examples mark those spots as `<NodeId>`.

#### GET `url`

No body.

Success payload is a string. It is `current_url`, or `""` when there is no URL.

```json
"https://example/"
```

#### GET `source`

No body.

Success payload is `document.page_source()`.

```json
"<!DOCTYPE html><html><body></body></html>"
```

The chuzz `source` helper reads `value` as a string. A non-string becomes `""`.

#### GET `screenshot`

No body.

The handler calls `wait_for_idle`. An error from that call is returned as the screenshot result. If `latest_screenshot` is then `Some`, the success payload is that base64 string. If it is `None`, the result is `ControlResponse::unsupported` with `CPU screenshots are not configured`.

```json
"iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg=="
```

The chuzz helper reads `value` as a string and decodes standard base64. `=` and whitespace are skipped. Any other byte outside the standard alphabet fails with `not base64: byte {byte:#04x}`. A non-string `value` fails with `no screenshot: {value}`.

Izumo always calls `with_cpu_screenshot`, so a driver started from `run` has a size configured.

#### GET `window`

No body. Success payload:

```json
"blitz-main"
```

#### GET `window/handles`

No body. Success payload:

```json
["blitz-main"]
```

#### POST `window`

Request:

```json
{"handle":"blitz-main"}
```

Success payload is `null`. Any other `handle`, including a missing one, is error `no such window` with message `only the blitz-main window exists`.

#### POST `execute/sync`

Request:

```json
{"script":"return 1","args":[]}
```

`script` is required. `args` defaults to `[]` and must be an array. The text evaluated is `(function() {\n{script}\n}).apply(null, {args})` with `args` inserted as JSON.

Success payload is the JSON returned by `eval_json`. `document_revision` increments.

Missing `script` is `invalid argument` / `missing script`. A non-array `args` is `invalid argument` / `args must be an array`. Eval failure is `javascript error` and the eval message.

#### POST `execute/async`

Request:

```json
{"script":"arguments[0](42)","args":[]}
```

`script` is required. `args` must be an array when present. The callback is appended after `args`. The script is applied as a function. The callback stores `{ "done": true, "value": <argument> }` on `globalThis.__blitzAsyncResult{n}`. The loop polls until `done` is true or 5 seconds pass.

Success payload is that `value`, or `null` if `value` is absent. The global is deleted. `document_revision` increments.

Missing `script` is `invalid argument` / `missing script`. A non-array `args` is `invalid argument` / `args must be an array`. Eval failure is `javascript error`. A missed deadline is `script timeout` / `asynchronous script callback did not run`.

#### POST `actions`

Request:

```json
{
  "actions": [
    {
      "type": "pointer",
      "actions": [
        {"type": "pointerMove", "x": 10.0, "y": 20.0, "origin": "viewport"},
        {"type": "pointerDown", "button": 0},
        {"type": "pointerUp"}
      ]
    },
    {
      "type": "key",
      "actions": [
        {"type": "keyDown", "value": "a"},
        {"type": "keyUp", "value": "a"},
        {"type": "pause"}
      ]
    }
  ]
}
```

`actions` must be an array. Each source needs an `actions` array. `pause` does nothing. `pointerMove` requires numeric `x` and `y`. A string `origin` other than `viewport` is unsupported, message `only viewport-origin pointer actions are supported`. A missing `origin` is accepted. `pointerDown` allows missing `button` or `button` 0. Any other button is unsupported, message `only the primary pointer button is supported`. `pointerUp` uses the stored position and buttons. `keyDown` and `keyUp` require string `value`. The value `\uE003` is Backspace. `\uE007`, `\n`, and `\r` are Enter. Any other string is sent as that character. Any other pair is unsupported, message `input action is not implemented`.

Pointer moves, downs, and ups are `UiEvent::PointerMove`, `PointerDown`, and `PointerUp`. Keys are `KeyDown` and `KeyUp`.

Success payload is `null`. `document_revision` increments once for the whole call.

`actions` missing or not an array is `invalid argument` / `actions must be an array`. A source without an actions array is `invalid argument` / `each input source requires an actions array`. Missing coordinates are `invalid argument` / `pointerMove requires x` or `pointerMove requires y`. Missing key text is `invalid argument` / `keyDown requires value` or `keyUp requires value`.

#### DELETE `actions`

No body. Success payload is `null`. This arm does not clear the stored pointer position or buttons.

#### POST `element`

Find one.

Request:

```json
{"using":"css selector","value":"#main"}
```

`using` must be the string `css selector`. `value` must be a string.

Success payload:

```json
{"element-6066-11e4-a52e-4f735466cecf":"1:42"}
```

No match is `no such element` with message `no element matches {selector:?}`. A bad selector is `invalid argument` / `invalid CSS selector: {error:?}`. A wrong strategy is `invalid argument` / `only the css selector locator strategy is supported`. A missing `value` is `invalid argument` / `missing selector value`.

#### POST `elements`

Same request as `POST element`.

Success payload is an array of reference objects. No match is an empty array, not an error. A bad selector uses the same `invalid argument` message as find-one.

```json
[
  {"element-6066-11e4-a52e-4f735466cecf":"1:42"}
]
```

#### POST `blitz/waitForIdle`

No required body.

The loop calls `document.poll(None)` while it returns true. Each true result counts. At 1000 the call returns error `timeout` with message `document did not become idle within the work budget`. Otherwise it resolves style and layout, then `commit_cpu_frame`.

Success payload:

```json
{
  "documentRevision": 1,
  "styleRevision": 0,
  "layoutRevision": 0,
  "paintRevision": 1,
  "workTurns": 0
}
```

`workTurns` is how many polls returned true. `commit_cpu_frame` with no screenshot size returns immediately and does not change `paintRevision`. With a size, a successful PNG increments `paintRevision`. PNG failure is `unknown error` / `PNG encoding failed: {encode_error}`.

The contract's idle list is longer than this loop. It names runnable jobs, due timers, one animation frame, style, layout, a committed CPU paint, and a quiet final turn. It also asks a timeout to include queue sizes and last revisions. The error helper sets `error`, `message`, and an empty `stacktrace`.

#### GET `blitz/getDomSnapshot`

No body. This arm does not call `wait_for_idle` or `resolve`.

Success payload:

```json
{
  "documentGeneration": 1,
  "documentRevision": 1,
  "styleRevision": 0,
  "layoutRevision": 0,
  "paintRevision": 0,
  "nodes": [
    {
      "nodeId": "<NodeId>",
      "parentId": "<node.parent>",
      "children": [],
      "tagName": "main",
      "attributes": {"id": "main"},
      "text": "Hello"
    }
  ]
}
```

Every tree node is included. `tagName` is the element local name, or null when the node has no element data. `attributes` is an object of local name to string. It is `{}` when there is no element data. `text` is `node.text_content()`. `children` is the child id list.

#### GET or POST `blitz/getConsoleEntries`

`after` is a `u64`. It defaults to 0.

Request:

```json
{"after": 0}
```

Success payload:

```json
{
  "after": 0,
  "overflowed": false,
  "entries": [
    {"sequence": 1, "level": "<entry.level>", "message": "<entry.message>"}
  ]
}
```

`overflowed` is true when the first returned entry has `sequence` greater than `after + 1`. Entry fields are `sequence`, `level`, and `message`.

#### GET or POST `blitz/getRuntimeErrors`

Same `after` rule.

Request:

```json
{"after": 0}
```

Success payload:

```json
{
  "after": 0,
  "overflowed": false,
  "entries": [
    {"sequence": 1, "message": "<entry.message>", "stack": "<entry.stack>"}
  ]
}
```

`overflowed` uses the same sequence gap test. Entry fields are `sequence`, `message`, and `stack`.

#### GET or POST `blitz/traceEvent`

Same `after` rule. Entries kept in the deque with `sequence` greater than `after` are returned. There is no `overflowed` field.

Request:

```json
{"after": 0}
```

Success payload:

```json
{
  "after": 0,
  "entries": [
    {
      "sequence": 1,
      "event": "click",
      "requestedNodeId": "<NodeId>",
      "targetNodeId": "<NodeId>",
      "path": [],
      "includedDocument": true,
      "inputPath": "pointer-hit-test"
    }
  ]
}
```

The only writer in this module is element `click`. The deque holds 256 entries. At capacity, the oldest is dropped before the new one is pushed. `sequence` starts at 1.

`requestedNodeId` is the element that was addressed. `targetNodeId` is the hit-test node, or the requested node when hit-testing returns nothing. `path` is `node_chain` of the hit node. `includedDocument` is whether that path contains the document root id.

#### GET or POST `blitz/getComputedStyle`

Request:

```json
{"element": "1:42"}
```

`element` must be a string reference. The handler resolves, then reads `diagnostic_computed_style`.

Success payload:

```json
{
  "nodeId": "<NodeId>",
  "styleRevision": 0,
  "properties": {"color": "rgb(0, 0, 0)"}
}
```

`properties` is an object of property name to string.

A missing string `element` is `invalid argument` / `blitz/getComputedStyle requires an element reference`. A bad reference is `stale element reference` / `element is no longer attached`. Missing computed style is `unknown error` / `computed style is unavailable`.

#### GET `blitz/getLayoutTree`

No body. The handler resolves first. Nodes with no client rect are omitted.

Success payload:

```json
{
  "layoutRevision": 0,
  "nodes": [
    {"nodeId": "<NodeId>", "x": 0.0, "y": 0.0, "width": 100.0, "height": 20.0}
  ]
}
```

Fields per node are `nodeId`, `x`, `y`, `width`, and `height`. The contract table also names clipping and scroll offsets. This function does not emit them.

#### GET `blitz/getSvgDiagnostics`

No body. The handler resolves first. An entry is an in-document element whose local name is `svg` and that has inline SVG source.

Success payload:

```json
{
  "entries": [
    {
      "nodeId": "<NodeId>",
      "source": "<inline svg source>",
      "layout": {"x": 0.0, "y": 0.0, "width": 24.0, "height": 24.0},
      "image": {
        "kind": "svg",
        "width": 24.0,
        "height": 24.0,
        "intrinsicWidth": 24.0,
        "intrinsicHeight": 24.0,
        "rootChildren": 1,
        "contentBounds": {"x": 0.0, "y": 0.0, "width": 24.0, "height": 24.0}
      }
    }
  ]
}
```

`layout` is null when there is no client rect. `image` is null when there is no image data. Raster image:

```json
{"kind": "raster", "width": 24, "height": 24}
```

No image data:

```json
{"kind": "none"}
```

#### GET `blitz/getRendererMetrics`

No body.

Success payload:

```json
{
  "documentRevision": 1,
  "styleRevision": 0,
  "layoutRevision": 0,
  "paintRevision": 0,
  "eventTraceEntries": 0
}
```

`eventTraceEntries` is the current deque length.

#### POST `blitz/shutdown`

No body. Sets `exit_requested`. Success payload is `null`.

#### Element commands

The path is `element/{reference}/{command}`. `{reference}` is the `generation:id` string. A path that does not start with `element/` is unsupported, message `command is not implemented`. A path with no second slash is `invalid argument` / `invalid element command path`. A reference that fails `resolve_element` is `stale element reference` / `element is no longer attached`.

GET `element/1:42/text`. Success payload is `text_content()`:

```json
"Hello"
```

GET `element/1:42/rect`. Success payload:

```json
{"x": 0.0, "y": 0.0, "width": 100.0, "height": 20.0}
```

No client rect is `stale element reference` / `element has no layout box`. This arm does not call `resolve` first.

POST `element/1:42/click`. No required body. The handler resolves, takes the center of the client rect, hit-tests, then sends pointer move, pointer down, and pointer up. It records one `click` trace. Success payload is `null`. `document_revision` increments.

No client rect is `element not interactable` / `element has no layout box`. Width or height at or below 0 is `element not interactable` / `element has an empty layout box`.

POST `element/1:42/value`. Either body works:

```json
{"text": "hi"}
```

```json
{"value": ["h", "i"]}
```

`value` is an array of strings, concatenated. The handler resolves, focuses the node, and commits the text as an IME commit. Success payload is `null`. `document_revision` increments. Neither field usable is `invalid argument` / `send keys requires text or a string value array`.

POST `element/1:42/focus`. No body. Calls `set_focus_to`. Success payload is `null`.

GET `element/1:42/displayed`. The handler resolves. Success payload is a boolean. It is true when the node is present, is not `display: none`, and has a client rect with width and height greater than 0.

```json
true
```

GET `element/1:42/enabled`. Success payload is a boolean. It is true when the node is present and no attribute local name is `disabled`.

```json
true
```

GET `element/1:42/attribute/{name}`. `{name}` is the attribute local name. Success payload is the attribute string, or `null` when it is absent.

```json
"main"
```

```json
null
```

GET `element/1:42/property/{name}`. Known names:

| `name` | Success value |
| --- | --- |
| `textContent` | string from `text_content()` |
| `className` | class attribute string, or `null` |
| `value` | text-input editor text, or `null` |
| `checked` | checkbox bool, or `false` when that data is absent |

Any other name uses the attribute lookup above. A missing node on this arm is `stale element reference` / `element is no longer attached`.

Examples:

```json
"Hello"
```

```json
"title"
```

```json
null
```

```json
false
```

Any other element command is unsupported, message `element command is not implemented`.

### Error behaviour

`ControlResponse::Error` has three fields. `error()` and `invalid()` set them. `invalid` always uses the error name `invalid argument`. `stacktrace` is always `""` on this path.

```json
{"error": "invalid argument", "message": "missing script", "stacktrace": ""}
```

Error names produced in this file:

| `error` | When | `message` |
| --- | --- | --- |
| `invalid argument` | bad client input | the `invalid(...)` strings above |
| `no such window` | POST `window` | `only the blitz-main window exists` |
| `no such element` | find-one misses | `no element matches {selector:?}` |
| `stale element reference` | bad reference, or rect with no box | `element is no longer attached` or `element has no layout box` |
| `element not interactable` | click | `element has no layout box` or `element has an empty layout box` |
| `javascript error` | eval failure | the eval message |
| `script timeout` | async script | `asynchronous script callback did not run` |
| `timeout` | idle budget | `document did not become idle within the work budget` |
| `unknown error` | style or PNG | `computed style is unavailable` or `PNG encoding failed: {encode_error}` |

`ControlResponse::unsupported` is a separate constructor. Messages from this file:

- `CPU screenshots are not configured`
- `only viewport-origin pointer actions are supported`
- `only the primary pointer button is supported`
- `input action is not implemented`
- `command is not implemented`
- `element command is not implemented`

An unsupported command returns error `unsupported operation` with a message and an empty `stacktrace`.

The chuzz HTTP helper fails with `the driver answered without headers` when the split is missing. A body that is not JSON fails JSON parse. Connect and read use the 30 second socket timeouts. Screenshot decode fails on non-base64 input. Session create fails when `value.sessionId` is not a string, including when `value` is an error object.

`GET source` in the client turns a non-string `value` into `""`. It does not surface an error payload.

Responses use WebDriver envelopes: a success is `{"value": <payload>}` and an error is `{"value":{"error":...,"message":...,"stacktrace":""}}`. The chuzz client reads the top-level `value`.

### Access control

The listener is off unless `TAURI_BLITZ_DRIVER` is set. A set address with no descriptor fails startup.

The contract says bind only loopback, use a random 256-bit token, store it in an atomic owner-readable `0600` descriptor, require `blitz:token` on `POST /session`, and keep application data off `GET /status`. It says this endpoint can evaluate arbitrary JavaScript. It says not to enable it in a normal production build, not to log the token, not to put the token in command-line arguments, and not to expose it beyond loopback.

The server generates a 256-bit random token per process (`getrandom`, hex), writes the descriptor with mode `0600`, binds loopback only, and allows one session. Anyone holding the token can run page script through `execute/sync` and `execute/async`, so treat the descriptor like a credential and never enable the driver on a shared machine.

### Hosts

Izumo `run` starts this plane when `debug-control` is enabled and both driver env vars are set. That block is not gated on unix. The screenshot size helper is under the same feature.

The chuzz client comment names `target/release/chuzz-gui` as the process launched with those variables.

Diagnostics capture is a different host feature. It needs `agent-control` and unix for the bridge, and `diagnostics` for `DiagnosticsRequest::Capture`. `Host.diagnostics` is that feature flag, reported when the agent-control server starts. `--blitz-control` starts that server during `run`. It does not start the driver.

### chuzz-inspect --driver



The verified client is `Driver` in `chuzz/crates/chuzz-control/src/driver.rs`.

`descriptor_from_env` returns the descriptor path when `TAURI_BLITZ_DRIVER_DESCRIPTOR` is set.

`Driver::connect(path)` reads the file, posts the token to `/session`, and stores `address` and `sessionId`.

`screenshot` sends `GET /session/{sessionId}/screenshot` and returns PNG bytes.

`source` sends `GET /session/{sessionId}/source` and returns a string.

Drop sends `DELETE /session/{sessionId}` so the single session can be opened again.

<!--
Claims and sources. HTML comments cannot contain a double hyphen, so the inspect flag is written "driver flag" here.

What it is. chuzz driver is a separate socket from crate::client, for painted pixels: chuzz/crates/chuzz-control/src/driver.rs:1-8. Controller type and UI-thread rule: ps-blitz/packages/blitz-script/src/debug_control.rs:28-35,170-174. DebugServer and ServerConfig come from blitz_debug_control: ps-blitz/packages/blitz-script/src/debug_control.rs:11,54-55. Izumo starts it under feature debug-control via start_from_env(CARGO_PKG_VERSION) and set_debug_controller: izumo/crates/izumo/src/runtime.rs:644-650. No service_pending in runtime.rs. Contract process boundary: izumo/docs/06-debug-control.md:11-28.

Screenshot implementation. screenshot calls wait_for_idle and returns latest_screenshot or unsupported: ps-blitz/packages/blitz-script/src/debug_control.rs:819-835. commit_cpu_frame uses VelloCpuImageRenderer, paint_scene_at_time, viewport save/restore, device size from scale, RGBA8 PngEncoder, standard base64, paint_revision += 1: ps-blitz/packages/blitz-script/src/debug_control.rs:838-924. Imports: ps-blitz/packages/blitz-script/src/debug_control.rs:8-10. Default 1344x932 and env parse: izumo/crates/izumo/src/runtime.rs:648-649,708-714. with_cpu_screenshot: ps-blitz/packages/blitz-script/src/debug_control.rs:88-91. Screenshot success is only the string: ps-blitz/packages/blitz-script/src/debug_control.rs:832-834.

Diagnostics capture. Handle and Capture arm, paint_scene comment: izumo/crates/izumo/src/runtime.rs:332-378,381-402. diagnosticsUnavailable: izumo/crates/izumo/src/runtime.rs:449-454. Feature cfgs unix and diagnostics: izumo/crates/izumo/src/runtime.rs:277,332,387,407,465-467. Agent server start is separate, Host.diagnostics, --blitz-control: izumo/crates/izumo/src/runtime.rs:119-127,591-631,619-630. Driver env start is the later block: izumo/crates/izumo/src/runtime.rs:644-650.

Env vars. Address optional, descriptor required, SocketAddr parse errors: ps-blitz/packages/blitz-script/src/debug_control.rs:94-116. descriptor_from_env: chuzz/crates/chuzz-control/src/driver.rs:38-41. Screenshot size parse and fallback: izumo/crates/izumo/src/runtime.rs:708-714. Launch comment including CHUZZ_CONTROL and chuzz-gui: chuzz/crates/chuzz-control/src/driver.rs:10-18. screenshot_size starts None: ps-blitz/packages/blitz-script/src/debug_control.rs:65.

Transport. Contract loopback and bind rules, port 0: izumo/docs/06-debug-control.md:15-32,36-39. Client calls the server loopback: chuzz/crates/chuzz-control/src/driver.rs:10-11,20-23. No loopback check before ServerConfig: ps-blitz/packages/blitz-script/src/debug_control.rs:105-115. Test bind 127.0.0.1:0: ps-blitz/packages/blitz-script/src/debug_control.rs:1047-1051. Client TCP, 30s timeouts, HTTP/1.1 close, header split, JSON body, no status read: chuzz/crates/chuzz-control/src/driver.rs:123-152. Request template fields: chuzz/crates/chuzz-control/src/driver.rs:137-142.

Descriptor. Client reads address and token: chuzz/crates/chuzz-control/src/driver.rs:47-55. Error strings: chuzz/crates/chuzz-control/src/driver.rs:50,54. Contract JSON fields and 0600 wording: izumo/docs/06-debug-control.md:41-57. Client 0600 wording: chuzz/crates/chuzz-control/src/driver.rs:10-12. ServerConfig fields only: ps-blitz/packages/blitz-script/src/debug_control.rs:111-115. Izumo passes CARGO_PKG_VERSION: izumo/crates/izumo/src/runtime.rs:646. Historical note that rendererRevision in the doc example is a git revision: izumo/docs/06-debug-control.md:49-50.

Session. POST body and sessionId read: chuzz/crates/chuzz-control/src/driver.rs:57-78. Failure string: chuzz/crates/chuzz-control/src/driver.rs:68-76. DELETE and Drop: chuzz/crates/chuzz-control/src/driver.rs:96-109,182-185. One session comment: chuzz/crates/chuzz-control/src/driver.rs:44-45,65-67,96-101. Contract status and blitz:token: izumo/docs/06-debug-control.md:54-57. route has no status arm: ps-blitz/packages/blitz-script/src/debug_control.rs:209-250. Token not resent on screenshot, source, or DELETE: chuzz/crates/chuzz-control/src/driver.rs:83-108,112-119.

Counters and document switch: ps-blitz/packages/blitz-script/src/debug_control.rs:60-64,176-188. document_revision increments: ps-blitz/packages/blitz-script/src/debug_control.rs:430,445-446,480-481,758,783. Element key and reference format: ps-blitz/packages/blitz-script/src/debug_control.rs:23,570-577. resolve_element: ps-blitz/packages/blitz-script/src/debug_control.rs:703-720.

GET url: ps-blitz/packages/blitz-script/src/debug_control.rs:210-215. GET source: ps-blitz/packages/blitz-script/src/debug_control.rs:216. Client source unwrap_or_default: chuzz/crates/chuzz-control/src/driver.rs:112-119. GET screenshot: ps-blitz/packages/blitz-script/src/debug_control.rs:217,819-835. Client screenshot path and decode: chuzz/crates/chuzz-control/src/driver.rs:83-94,159-179. GET window, handles, POST window: ps-blitz/packages/blitz-script/src/debug_control.rs:218-223.

execute/sync: ps-blitz/packages/blitz-script/src/debug_control.rs:224,434-449. execute/async and 5s timeout: ps-blitz/packages/blitz-script/src/debug_control.rs:25,225,452-491.

actions: ps-blitz/packages/blitz-script/src/debug_control.rs:226,362-431. Key mapping: ps-blitz/packages/blitz-script/src/debug_control.rs:994-1017. DELETE actions: ps-blitz/packages/blitz-script/src/debug_control.rs:227.

POST element and elements: ps-blitz/packages/blitz-script/src/debug_control.rs:228-229,543-567.

waitForIdle, MAX_IDLE_TURNS 1000: ps-blitz/packages/blitz-script/src/debug_control.rs:24,230,795-816. Contract idle steps and timeout payload: izumo/docs/06-debug-control.md:113-143. error helper fields: ps-blitz/packages/blitz-script/src/debug_control.rs:1024-1033. Empty size skips paint: ps-blitz/packages/blitz-script/src/debug_control.rs:838-841.

getDomSnapshot: ps-blitz/packages/blitz-script/src/debug_control.rs:231,927-965. Console entries: ps-blitz/packages/blitz-script/src/debug_control.rs:232-234,494-508. Runtime errors: ps-blitz/packages/blitz-script/src/debug_control.rs:235-237,511-525. traceEvent and capacity 256 and click record: ps-blitz/packages/blitz-script/src/debug_control.rs:26,238,528-541,723-760,787-793. Sequence filter: ps-blitz/packages/blitz-script/src/debug_control.rs:530-538.

getComputedStyle: ps-blitz/packages/blitz-script/src/debug_control.rs:239-241,253-274. getLayoutTree fields: ps-blitz/packages/blitz-script/src/debug_control.rs:242,277-295. Contract layout columns: izumo/docs/06-debug-control.md:98. getSvgDiagnostics: ps-blitz/packages/blitz-script/src/debug_control.rs:243,308-359. getRendererMetrics: ps-blitz/packages/blitz-script/src/debug_control.rs:244,298-305. shutdown and exit_requested: ps-blitz/packages/blitz-script/src/debug_control.rs:48,165-168,245-248. Izumo runtime.rs has no exit_requested use in the driver block.

Element commands: ps-blitz/packages/blitz-script/src/debug_control.rs:249,579-701. text, rect, click, value, focus, displayed, enabled: ps-blitz/packages/blitz-script/src/debug_control.rs:601-645. attribute: ps-blitz/packages/blitz-script/src/debug_control.rs:594-596,650-666. property names: ps-blitz/packages/blitz-script/src/debug_control.rs:597-599,669-700. unsupported element and unknown command: ps-blitz/packages/blitz-script/src/debug_control.rs:584-586,646.

Client HTTP paths: chuzz/crates/chuzz-control/src/driver.rs:59-63,83-88,103-107,112-117. Base64 test confirms PNG signature bytes: chuzz/crates/chuzz-control/src/driver.rs:201-205.

Access. Contract production, token, loopback, JS warning: izumo/docs/06-debug-control.md:55-60,173-174. 256-bit token text: izumo/docs/06-debug-control.md:47,173. Opt-in code: ps-blitz/packages/blitz-script/src/debug_control.rs:94-104. JS execution arms: ps-blitz/packages/blitz-script/src/debug_control.rs:224-225. Unsupported contract wording: izumo/docs/06-debug-control.md:64-66. W3C envelope contract: izumo/docs/06-debug-control.md:64-65. Client value envelope: chuzz/crates/chuzz-control/src/driver.rs:68-76,90-93,119. Header failure: chuzz/crates/chuzz-control/src/driver.rs:148-152.

Hosts. debug-control cfg is not unix on the driver block: izumo/crates/izumo/src/runtime.rs:644-650,708. Agent-control unix cfg: izumo/crates/izumo/src/runtime.rs:107,244,591. chuzz-gui launch is the client comment: chuzz/crates/chuzz-control/src/driver.rs:14-18. Driver API: chuzz/crates/chuzz-control/src/driver.rs:32-120,182-185.

Minimum command list in the contract, for comparison with route: izumo/docs/06-debug-control.md:68-78,91-103. getSvgDiagnostics and blitz/shutdown are only in the route table: ps-blitz/packages/blitz-script/src/debug_control.rs:243-248.
-->
