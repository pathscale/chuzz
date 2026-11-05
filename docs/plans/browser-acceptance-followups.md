# Browser acceptance follow-ups

This plan records cross-cutting behavior that still needs focused verification or implementation. Recheck current ownership and code paths before acting; older static reviews and design notes are leads, not evidence of present behavior.

## Security boundaries

- Verify that each document-created request retains its initiator through navigation, reload, history traversal, redirects, workers, and resource loading. Enforce file access, private-network restrictions, credentials, and request/byte limits at the appropriate shared boundary.
- Check SVG image loads for host-file access and ensure cross-origin video remains tainted when drawn to canvas or WebGPU. Verify WebAuthn requires the right site identity, focus, and user activation, with arbitration scoped per tab.
- Review CSP behavior at document and fetch boundaries. Keep it distinct from browser-owned capability and resource policy.
- Review release workflows and updater/package integrity boundaries, including when dependency code runs relative to signing credentials. Confirm provenance and integrity for distributed assets.

## Runtime and rendering evidence

- Verify native document lifecycle behavior through the browser's GUI and headless control paths: polling, diagnostics, observation delivery, session capture, and idle revisions. An acknowledgement alone does not prove event delivery.
- Validate browser compatibility APIs against observable behavior before advertising them. In particular, exercise abort and reentrant event handling, option validation, and any media APIs that claim stream support.
- Add or maintain a small, redistributable cached-page fixture set with stable viewport, scale, and fonts. Compare rendered output and report per-page failures without relying on live-site rankings.
- Establish representative performance regression checks and parser fuzz inputs with stable, bounded workloads. Avoid unsupported performance claims until measurements exist.

## Distribution decisions

- Decide the update and signature policy for direct distribution, and separately assess requirements for a store-delivered build. Preserve existing signing contracts while evaluating packaging changes.

## Related durable plans

- [Runtime-free browser integration](../proposals/runtime-free-browser.md) covers standalone shell activation, network behavior, and rendered acceptance.
- [Distribution](../distribution.md) documents current packaging context.
