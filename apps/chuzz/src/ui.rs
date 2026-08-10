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
  height: 100%;
  font-family: system-ui, -apple-system, "Segoe UI", sans-serif;
  font-size: 14px;
  color: #202124;
  background: #dee1e6;
}

#frame {
  display: flex;
  flex-direction: column;
  height: 100%;
  width: 100%;
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

/* Page area */

#page-area {
  flex-grow: 1;
  background: #ffffff;
  overflow: hidden;
}

.page {
  width: 100%;
  height: 100%;
}

#loading-bar {
  height: 2px;
  background: #1a73e8;
  flex-shrink: 0;
}
"#;
