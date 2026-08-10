//! Styling for the browser UI.
//!
//! This stylesheet only ever applies to the shell document. Page content lives
//! in a child document inside `.page`, so a site's CSS cannot reach the
//! toolbar and this cannot reach the site.

pub const BROWSER_UI_CSS: &str = r#"
* { box-sizing: border-box; }

html, body {
  margin: 0;
  padding: 0;
  height: 100vh;
  overflow: hidden;
  font-family: system-ui, -apple-system, "Segoe UI", sans-serif;
  font-size: 14px;
  color: #202124;
  background: #dee1e6;
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

/* Tab strip */

#tab-strip {
  display: flex;
  flex-direction: row;
  align-items: flex-end;
  gap: 4px;
  padding: 6px 8px 0 8px;
  background: #dee1e6;
  flex-shrink: 0;
}

.tab {
  display: flex;
  flex-direction: row;
  align-items: center;
  gap: 8px;
  min-width: 80px;
  max-width: 240px;
  height: 34px;
  padding: 0 10px;
  border-radius: 8px 8px 0 0;
  background: #c8ccd1;
  color: #3c4043;
  cursor: pointer;
}

.tab:hover { background: #d4d8dd; }

.tab.active {
  background: #ffffff;
  color: #202124;
}

.tab-title {
  flex-grow: 1;
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
}

.tab-close {
  width: 18px;
  height: 18px;
  line-height: 18px;
  text-align: center;
  border-radius: 9px;
  color: #5f6368;
  flex-shrink: 0;
}

.tab-close:hover { background: rgba(0, 0, 0, 0.12); color: #202124; }

#new-tab {
  width: 28px;
  height: 28px;
  line-height: 28px;
  text-align: center;
  border-radius: 14px;
  color: #3c4043;
  font-size: 18px;
  cursor: pointer;
  flex-shrink: 0;
}

#new-tab:hover { background: rgba(0, 0, 0, 0.10); }

/* Toolbar */

#toolbar {
  display: flex;
  flex-direction: row;
  align-items: center;
  gap: 4px;
  padding: 6px 8px;
  background: #ffffff;
  flex-shrink: 0;
}

.tool-button {
  width: 32px;
  height: 32px;
  line-height: 32px;
  text-align: center;
  border-radius: 16px;
  color: #3c4043;
  font-size: 16px;
  cursor: pointer;
  flex-shrink: 0;
}

.tool-button:hover { background: rgba(0, 0, 0, 0.08); }

.tool-button.disabled { color: #bdc1c6; }
.tool-button.disabled:hover { background: transparent; }

#url-bar {
  flex-grow: 1;
  height: 32px;
  margin: 0 8px;
  padding: 0 14px;
  border: none;
  border-radius: 16px;
  background: #f1f3f4;
  color: #202124;
  font-size: 14px;
  font-family: system-ui, -apple-system, "Segoe UI", sans-serif;
}

#url-bar:focus {
  background: #ffffff;
  outline: 2px solid #1a73e8;
}

/* Content row: page on the left, side panel on the right.
   Mirrors AgencyZero's shell: a column frame whose content row fills the
   remaining height and never scrolls as a whole. */

#content-row {
  display: flex;
  flex-direction: row;
  flex-grow: 1;
  min-height: 0;
  overflow: hidden;
}

/* min-width/min-height:0 stop a flex item from refusing to shrink below its
   content size, which is what leaves a page clipped mid-paragraph. */
#page-area {
  flex-grow: 1;
  min-width: 0;
  min-height: 0;
  background: #ffffff;
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
  width: 320px;
  flex-shrink: 0;
  background: #f7f8fa;
  border-left: 1px solid #d9dce0;
  overflow: hidden;
}

#side-panel.collapsed {
  width: 36px;
}

#side-panel-header {
  display: flex;
  flex-direction: row;
  align-items: center;
  gap: 8px;
  height: 36px;
  padding: 0 6px 0 10px;
  flex-shrink: 0;
  border-bottom: 1px solid #d9dce0;
}

#side-panel-title {
  flex-grow: 1;
  font-size: 12px;
  font-weight: 600;
  color: #5f6368;
  overflow: hidden;
  white-space: nowrap;
}

#side-panel-body {
  flex-grow: 1;
  min-height: 0;
  padding: 12px;
  overflow-y: auto;
  color: #5f6368;
  font-size: 13px;
}

#side-panel-toggle {
  width: 24px;
  height: 24px;
  line-height: 24px;
  text-align: center;
  border-radius: 12px;
  color: #3c4043;
  flex-shrink: 0;
}

#side-panel-toggle:hover { background: rgba(0, 0, 0, 0.10); }

#loading-bar {
  height: 2px;
  background: #1a73e8;
  flex-shrink: 0;
}
"#;
