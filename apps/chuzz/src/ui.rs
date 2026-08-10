//! Styling for the browser UI, ported from AgencyZero's workspace.
//!
//! The palette is theirs: a dark navy desk, panels lifted above it, a text
//! ladder that never reaches pure white, and a gold accent. Two rules the
//! design encodes and this keeps: nothing dimmer than the faint rung, and no
//! pure white anywhere.
//!
//! Values are resolved rather than computed. AgencyZero derives every surface
//! from `oklch()` and `color-mix()` driven by a theme picker; there is no
//! picker here, so the ladder is written out directly.
//!
//! This stylesheet only ever applies to the shell document. Page content lives
//! in a child document inside `.page`, so a site's CSS cannot reach the
//! browser UI and this cannot reach the site.

pub const BROWSER_UI_CSS: &str = r#"
* { box-sizing: border-box; }

html, body {
  margin: 0;
  padding: 0;
  height: 100vh;
  overflow: hidden;
  font-family: system-ui, -apple-system, "Segoe UI", sans-serif;
  font-size: 14px;
  color: #dbe2ea;
  /* The desk. */
  background: #131c2b;
}

/* Viewport units, not height:100%. A percentage height only resolves when
   every ancestor has a resolved height; when that chain breaks the frame
   collapses to its content and the rest of the window paints as dead space. */
#frame {
  display: flex;
  flex-direction: column;
  height: 100vh;
  width: 100vw;
  overflow: hidden;
}

/* Titlebar: tabs sit in the window's title row, beside the traffic lights.
   The left pad clears the macOS window buttons. */

#titlebar {
  display: flex;
  flex-direction: row;
  align-items: center;
  gap: 8px;
  height: 52px;
  flex-shrink: 0;
  padding: 0 14px 0 96px;
  background: #131c2b;
}

#nav-back {
  width: 30px;
  height: 30px;
  line-height: 30px;
  text-align: center;
  border-radius: 15px;
  color: #8fa0b8;
  font-size: 15px;
  flex-shrink: 0;
  cursor: pointer;
}

#nav-back:hover { background: rgba(255, 255, 255, 0.07); color: #dbe2ea; }

#tab-strip {
  display: flex;
  flex-direction: row;
  align-items: center;
  gap: 4px;
  flex-grow: 1;
  min-width: 0;
  overflow: hidden;
}

/* Pills that change surface, not shape, when active. */
.tab {
  display: flex;
  flex-direction: row;
  align-items: center;
  gap: 9px;
  min-width: 90px;
  max-width: 260px;
  height: 34px;
  padding: 0 14px;
  border-radius: 17px;
  border: 1px solid transparent;
  color: #8fa0b8;
  cursor: pointer;
}

.tab:hover { background: rgba(255, 255, 255, 0.05); color: #dbe2ea; }

.tab.active {
  background: #1b2739;
  border-color: rgba(255, 255, 255, 0.10);
  color: #ffffff;
  font-weight: 600;
}

/* Per-tab status dot: the point is seeing a background tab change state. */
.tab-dot {
  width: 7px;
  height: 7px;
  border-radius: 4px;
  background: #3fb950;
  flex-shrink: 0;
}

.tab-dot.loading { background: #d2ad3f; }

.tab-title {
  flex-grow: 1;
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
  font-size: 13px;
}

.tab-close {
  width: 18px;
  height: 18px;
  line-height: 18px;
  text-align: center;
  border-radius: 9px;
  color: #8fa0b8;
  flex-shrink: 0;
  font-size: 13px;
}

.tab-close:hover { background: rgba(255, 255, 255, 0.14); color: #ffffff; }

/* Dashed outline: an empty slot waiting to be filled. */
#new-tab {
  width: 34px;
  height: 34px;
  line-height: 32px;
  text-align: center;
  border-radius: 17px;
  border: 1px dashed rgba(255, 255, 255, 0.18);
  color: #8fa0b8;
  font-size: 16px;
  cursor: pointer;
  flex-shrink: 0;
}

#new-tab:hover { border-color: rgba(255, 255, 255, 0.34); color: #dbe2ea; }

.titlebar-button {
  width: 32px;
  height: 32px;
  line-height: 30px;
  text-align: center;
  border-radius: 16px;
  border: 1px solid rgba(255, 255, 255, 0.10);
  color: #8fa0b8;
  font-size: 14px;
  flex-shrink: 0;
  cursor: pointer;
}

.titlebar-button:hover { background: rgba(255, 255, 255, 0.07); color: #dbe2ea; }

#avatar {
  width: 32px;
  height: 32px;
  line-height: 30px;
  text-align: center;
  border-radius: 16px;
  border: 1px solid rgba(255, 255, 255, 0.14);
  background: #1b2739;
  color: #dbe2ea;
  font-size: 12.5px;
  font-weight: 600;
  flex-shrink: 0;
}

/* Toolbar */

#toolbar {
  display: flex;
  flex-direction: row;
  align-items: center;
  gap: 4px;
  padding: 0 14px 10px 14px;
  flex-shrink: 0;
}

.tool-button {
  width: 30px;
  height: 30px;
  line-height: 30px;
  text-align: center;
  border-radius: 15px;
  color: #8fa0b8;
  font-size: 15px;
  cursor: pointer;
  flex-shrink: 0;
}

.tool-button:hover { background: rgba(255, 255, 255, 0.07); color: #dbe2ea; }

.tool-button.disabled { color: #4a5768; }
.tool-button.disabled:hover { background: transparent; color: #4a5768; }

#url-bar {
  flex-grow: 1;
  height: 32px;
  margin: 0 8px;
  padding: 0 14px;
  border: 1px solid rgba(255, 255, 255, 0.10);
  border-radius: 16px;
  background: #1b2739;
  color: #dbe2ea;
  font-size: 13px;
  font-family: system-ui, -apple-system, "Segoe UI", sans-serif;
}

#url-bar:focus {
  background: #223047;
  border-color: #4c8dff;
  outline: none;
}

/* Content row: page card on the left, panel on the right. */

#content-row {
  position: relative;
  display: flex;
  flex-direction: row;
  flex-grow: 1;
  min-height: 0;
  padding: 0 14px 10px 14px;
  gap: 34px;
  overflow: hidden;
}

/* min-width/min-height:0 stop a flex item from refusing to shrink below its
   content size, which is what leaves a page clipped mid-paragraph. */
#page-area {
  flex-grow: 1;
  min-width: 0;
  min-height: 0;
  /* Dark until a page paints over it: a white card on a dark desk strobes on
     every load. */
  background: #0f1622;
  border: 1px solid rgba(255, 255, 255, 0.10);
  border-radius: 14px;
  overflow: hidden;
}

.page {
  width: 100%;
  height: 100%;
}

/* Side panel */

#side-panel {
  display: flex;
  flex-direction: column;
  width: 332px;
  flex-shrink: 0;
  min-height: 0;
  overflow: hidden;
}

#side-panel.collapsed {
  width: 34px;
  align-items: center;
  padding-top: 4px;
}

.panel-rail-toggle {
  width: 26px;
  height: 26px;
  line-height: 24px;
  text-align: center;
  border-radius: 13px;
  color: #8fa0b8;
  background: #1b2739;
  border: 1px solid rgba(255, 255, 255, 0.10);
  cursor: pointer;
}

.panel-rail-toggle:hover {
  color: #ffffff;
  background: #33445e;
  border-color: #4c8dff;
}

#side-panel-header {
  display: flex;
  flex-direction: row;
  align-items: center;
  gap: 8px;
  height: 30px;
  flex-shrink: 0;
  padding: 0 6px 0 4px;
}

#side-panel-title {
  flex-grow: 1;
  font-size: 11.5px;
  font-weight: 600;
  color: #8fa0b8;
  overflow: hidden;
  white-space: nowrap;
}

/* The collapse affordance is an edge handle, not a button in a header: a
   rounded tab that protrudes from the panel's leading edge, vertically
   centred, present whether the panel is open or collapsed. It rides the seam
   between the page card and the panel. */
/* Ported from AgencyZero's ProjectPanelToggle. The handle starts at the page
   boundary and occupies the whole gap on its right: a slim rectangular tab in
   the primary blue, rounded on its right side only, reaching neither panel's
   scrollbar. The arrow points toward the action, so right closes the visible
   panel and left restores the hidden one. */
.panel-edge-handle {
  position: absolute;
  top: 50%;
  z-index: 20;
  display: flex;
  align-items: center;
  justify-content: center;
  width: 6px;
  height: 36px;
  margin-top: -18px;
  border: 1px solid rgba(76, 141, 255, 0.40);
  border-left: none;
  border-radius: 0 6px 6px 0;
  background: rgba(76, 141, 255, 0.20);
  color: #4c8dff;
  font-size: 10px;
  font-weight: 700;
  cursor: pointer;
}

.panel-edge-handle:hover {
  border-color: rgba(76, 141, 255, 0.60);
  background: rgba(76, 141, 255, 0.30);
}

/* Hidden panel: the same tab mirrored, so the arrow still points at what the
   click will do. */
.panel-edge-handle.rotate {
  border: 1px solid rgba(76, 141, 255, 0.40);
  border-right: none;
  border-radius: 6px 0 0 6px;
}

#side-panel-scroll {
  display: flex;
  flex-direction: column;
  gap: 10px;
  flex-grow: 1;
  min-height: 0;
  overflow-y: auto;
}

/* Accordion sections: a card with a hairline, radius 14, whose body exists
   only while open. */

.section-panel {
  background: #1b2739;
  border: 1px solid rgba(255, 255, 255, 0.10);
  border-radius: 14px;
  overflow: hidden;
  flex-shrink: 0;
}

.section-header {
  display: flex;
  flex-direction: row;
  align-items: center;
  gap: 10px;
  padding: 11px 14px;
  cursor: pointer;
}

.section-header:hover { background: rgba(255, 255, 255, 0.04); }

.section-title {
  flex-grow: 1;
  font-size: 12.5px;
  font-weight: 600;
  color: #dbe2ea;
}

.section-count {
  border-radius: 10px;
  padding: 1px 8px;
  font-size: 11px;
  font-weight: 600;
  background: #26344a;
  color: #8fa0b8;
  flex-shrink: 0;
}

.section-count.primary {
  background: rgba(76, 141, 255, 0.18);
  color: #7fb0ff;
}

.section-chevron {
  width: 20px;
  height: 20px;
  line-height: 18px;
  text-align: center;
  border-radius: 10px;
  background: rgba(255, 255, 255, 0.06);
  color: #7fb0ff;
  font-size: 11px;
  flex-shrink: 0;
}

.section-header:hover .section-chevron {
  background: rgba(76, 141, 255, 0.20);
  color: #ffffff;
}

.section-body { border-top: 1px solid rgba(255, 255, 255, 0.07); }

.section-row {
  display: flex;
  flex-direction: row;
  align-items: center;
  gap: 10px;
  padding: 9px 14px;
  font-size: 12px;
}

.section-row-label {
  flex-grow: 1;
  color: #b9c4d2;
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
}

.section-row-value {
  color: #8fa0b8;
  flex-shrink: 0;
  font-family: ui-monospace, monospace;
  font-size: 11.5px;
}

.section-empty {
  padding: 12px 14px;
  font-size: 12px;
  color: #8fa0b8;
}

/* Status strip along the bottom. */

#status-strip {
  display: flex;
  flex-direction: row;
  align-items: center;
  gap: 8px;
  height: 28px;
  flex-shrink: 0;
  padding: 0 18px;
  font-family: ui-monospace, monospace;
  font-size: 11.5px;
  color: #6f7f96;
}

.status-spacer { flex-grow: 1; }

.status-dot {
  width: 7px;
  height: 7px;
  border-radius: 4px;
  background: #3fb950;
  flex-shrink: 0;
}

.status-dot.loading { background: #d2ad3f; }

.status-accent { color: #7fb0ff; }

#loading-bar {
  height: 2px;
  background: #4c8dff;
  flex-shrink: 0;
}
"#;
