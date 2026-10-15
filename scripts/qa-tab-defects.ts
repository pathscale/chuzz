// Apply each named group independently in an integrator's mutant build.
export const defects = [
  {
    "name": "stash-retain-worker-owner",
    "check": "stash-teardown-complete",
    "changes": [
      {
        "repo": "chuzz",
        "path": "apps/chuzz/src/workers/mod.rs",
        "search": "        for endpoint in endpoints.values() {\n            endpoint.terminate();\n        }",
        "replacement": "        for endpoint in endpoints.values() {\n            let _ = endpoint.id;\n        }"
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
        "search": "    pub fn stash_sensitive(&self, id: NodeId) -> bool {\n        if self.stash_policy.no_stash {\n            return true;\n        }\n        let mut cursor = Some(id);\n        while let Some(id) = cursor {\n            if self.stash_policy.sensitive.contains(&id) {\n                return true;\n            }\n            cursor = self.get_node(id).and_then(|node| node.parent);\n        }\n        let Some(element) = self.get_node(id).and_then(|node| node.element_data()) else {\n            return false;\n        };\n        let form = element\n            .attr(crate::local_name!(\"form\"))\n            .and_then(|name| self.get_element_by_id(name))\n            .or_else(|| self.controls_to_form.get(&id).copied());\n        form.is_some_and(|form| self.stash_policy.sensitive.contains(&form))\n    }",
        "replacement": "    pub fn stash_sensitive(&self, _id: NodeId) -> bool {\n        false\n    }"
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
        "search": "    fn accepts(&self, destination: &url::Url, entry: u64) -> bool {\n        self.entry == entry\n            && self.origin == destination.origin()\n            && self\n                .route\n                .units()\n                .iter()\n                .copied()\n                .eq(destination.as_str().encode_utf16())\n    }",
        "replacement": "    fn accepts(&self, _destination: &url::Url, entry: u64) -> bool {\n        self.entry == entry\n    }"
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
        "search": "            if tab.life.discarded\n                && tab.mounted.is_none()\n                && !matches!(tab.request().method.as_str(), \"GET\" | \"HEAD\")\n            {",
        "replacement": "            if tab.life.discarded\n                && tab.mounted.is_none()\n                && tab.request().method.as_str().is_empty()\n            {"
      },
      {
        "repo": "chuzz",
        "path": "apps/chuzz/src/browser.rs",
        "search": "            if replay {",
        "replacement": "            if replay {\n                if tab.life.discarded && tab.mounted.is_none() {\n                    request.initiator = net.browser_initiator(request.url.clone());\n                }"
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
        "search": "                    events.push(crate::surfaces::SurfaceEvent::Visibility { page, visible });",
        "replacement": "                    events.push(crate::surfaces::SurfaceEvent::Visibility { page, visible: page.document_id == page.owner_document_id && visible });"
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
        "replacement": "        page.set_was_discarded(discarded);\n        let refresh_owner = Arc::downgrade(&self.0);\n        page.add_poll_hook(move |_, _| {\n            if let Some(inner) = refresh_owner.upgrade() {\n                if let Some(tab) = inner\n                    .state\n                    .lock()\n                    .unwrap()\n                    .tabs\n                    .iter_mut()\n                    .find(|tab| tab.id == id)\n                {\n                    tab.life.hidden_at = nagoya::now_ns();\n                }\n            }\n            false\n        });"
      },
      {
        "repo": "chuzz",
        "path": "apps/chuzz/src/tab_lifecycle.rs",
        "search": "            if tab.loading && !automatic\n                || tab.life.phase != TabPhase::Live\n                || automatic\n                    && (tab.life.visible\n                        || tab\n                            .life\n                            .deadline\n                            .as_ref()\n                            .is_none_or(|deadline| nagoya::now_ns() < deadline.due))\n            {",
        "replacement": "            if tab.loading && !automatic\n                || tab.life.phase != TabPhase::Live\n                || automatic\n                    && (nagoya::now_ns().saturating_sub(tab.life.hidden_at)\n                        < self.0.sleep_delay_ns\n                        || tab.life.visible\n                        || tab\n                            .life\n                            .deadline\n                            .as_ref()\n                            .is_none_or(|deadline| nagoya::now_ns() < deadline.due))\n            {"
      }
    ]
  }
] as const;
