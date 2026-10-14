// Apply each named group independently in an integrator's mutant build.
export const defects = [
  {
    "name": "stash-retain-worker-owner",
    "check": "stash-teardown-complete",
    "changes": [
      {
        "repo": "chuzz",
        "path": "apps/chuzz/src/workers/mod.rs",
        "search": "        for endpoint in endpoints.values() { endpoint.terminate(); }",
        "replacement": "        for endpoint in endpoints.values() { let _ = endpoint.id; }"
      }
    ]
  },
  {
    "name": "stash-ignore-sensitive-classification",
    "check": "stash-sensitive-empty",
    "changes": [
      {
        "repo": "ps-blitz",
        "path": "packages/blitz-dom/src/stash_policy.rs",
        "search": "    pub fn stash_sensitive(&self, id: NodeId) -> bool {\n        if self.stash_policy.no_stash { return true; }\n        let mut cursor = Some(id);\n        while let Some(id) = cursor {\n            if self.stash_policy.sensitive.contains(&id) { return true; }\n            cursor = self.get_node(id).and_then(|node| node.parent);\n        }\n        let Some(element) = self.get_node(id).and_then(|node| node.element_data()) else { return false; };\n        let form = element.attr(crate::local_name!(\"form\"))\n            .and_then(|name| self.get_element_by_id(name))\n            .or_else(|| self.controls_to_form.get(&id).copied());\n        form.is_some_and(|form| self.stash_policy.sensitive.contains(&form))\n    }\n}",
        "replacement": "    pub fn stash_sensitive(&self, _id: NodeId) -> bool { false }\n}"
      }
    ]
  },
  {
    "name": "stash-ignore-origin",
    "check": "stash-origin-before-script",
    "changes": [
      {
        "repo": "chuzz",
        "path": "apps/chuzz/src/tab_stash.rs",
        "search": "    fn accepts(&self, destination: &url::Url, entry: u64) -> bool {\n        self.entry == entry && self.origin == destination.origin()\n            && self.route.units().iter().copied().eq(destination.as_str().encode_utf16())\n    }",
        "replacement": "    fn accepts(&self, _destination: &url::Url, entry: u64) -> bool { self.entry == entry }"
      }
    ]
  },
  {
    "name": "tab-replay-post-on-wake",
    "check": "tab-restore-failure-repost",
    "changes": [
      {
        "repo": "chuzz",
        "path": "apps/chuzz/src/tab_lifecycle.rs",
        "search": "            if !matches!(tab.request().method.as_str(), \"GET\" | \"HEAD\") {",
        "replacement": "            if tab.request().method.as_str().is_empty() {"
      },
      {
        "repo": "chuzz",
        "path": "apps/chuzz/src/browser.rs",
        "search": "            if tab.life.discarded && tab.mounted.is_none() && !matches!(tab.request().method.as_str(), \"GET\" | \"HEAD\") {",
        "replacement": "            if tab.life.discarded && tab.mounted.is_none() && tab.request().method.as_str().is_empty() {"
      }
    ]
  },
  {
    "name": "visibility-use-main-document",
    "check": "tab-visible-never-sleeps",
    "changes": [
      {
        "repo": "izumo",
        "path": "crates/izumo/src/runtime.rs",
        "search": "                    let on_screen = window_visible && root.surface_on_screen(mount);",
        "replacement": "                    let on_screen = page.document_id == page.owner_document_id && window_visible && root.surface_on_screen(mount);"
      }
    ]
  },
  {
    "name": "tab-refresh-age-on-script",
    "check": "tab-five-minute-expiry",
    "changes": [
      {
        "repo": "chuzz",
        "path": "apps/chuzz/src/tab_lifecycle.rs",
        "search": "        page.set_was_discarded(discarded);",
        "replacement": "        page.set_was_discarded(discarded);\n        let refresh_owner = Arc::downgrade(&self.0);\n        page.add_poll_hook(move |_, _| {\n            if let Some(inner) = refresh_owner.upgrade() {\n                if let Some(tab) = inner.state.lock().unwrap().tabs.iter_mut().find(|tab| tab.id == id) { tab.life.hidden_at = nagoya::now_ns(); }\n            }\n            false\n        });"
      },
      {
        "repo": "chuzz",
        "path": "apps/chuzz/src/tab_lifecycle.rs",
        "search": "            if tab.loading && !automatic || tab.life.phase != TabPhase::Live || automatic && (tab.life.visible",
        "replacement": "            if tab.loading && !automatic || tab.life.phase != TabPhase::Live || automatic && (nagoya::now_ns().saturating_sub(tab.life.hidden_at) < self.0.sleep_delay_ns || tab.life.visible"
      }
    ]
  }
] as const;
