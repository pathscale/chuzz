# Chuzz working agreement

- Chuzz is a pure Rust web browser. The invariant is the implementation stack, not the
  authoring language of the UI: no C++ engine, no JIT, no OS webview, every dependency Rust.
  Do not add WebKit, Chromium, or Electron. Tauri, Boa and a Solid bundle are all Rust or data
  the Rust stack interprets, and are in scope.
- Use the Pathscale Blitz stack for DOM, CSS, layout, networking, painting, and window integration.
- The browser chrome is a SolidJS app in `apps/chuzz/frontend`, built with rsbuild and
  `@pathscale/ui`, the same stack AgencyZero ships. It talks to Rust through one interface
  (`src/api/client.ts`) with a Tauri implementation and a mock; nothing above that file knows
  which it has.
- Keep browser policy separate from the renderer. The browser app owns tabs, navigation, history, permissions, persistence, and downloads.
- Keep the local control surface protocol-compatible with AgencyZero's Blitz interface: `blitz.agent.control` and `blitz.diagnostics` over local MCP framing.
- Control is opt-in, local-only, and unauthenticated only because the socket and descriptor are owner-readable. Never bind it to a network interface.
- **Every dependency is a published version with a caret, not a sibling
  checkout.** `ps-blitz`, the renderers and `endpoint-libs` are ordinary
  crates.io dependencies on `^`, so `cargo update` can move them, `Cargo.lock`
  records exactly what a release builds, and two crates asking for the same
  range share one copy instead of getting a second. Do not turn them back into
  `path = "../..."`. That is what this repository did before, and it put the
  revision CI used in a `BLITZ_REF` env var in release.yml while every developer
  built against whatever happened to be on disk. The pin sat 44 commits behind,
  and the release job broke the day a new engine package was added, because a
  path dependency resolves against the filesystem and no local build can notice.
  Do not reach for `rev` or `=` either: a git revision cannot be published and
  an exact pin is a range of one, so both split the graph the same way. A git
  `endpoint-libs` alongside the registry copy is precisely how this repository
  ended up with two of it. There are no exceptions left: `tauri-runtime-blitz`
  was the last git dependency, and it is `^0.1.0` from crates.io like the rest.
- **Building against a working checkout is opt-in and never edits a tracked
  file.** Put the `[patch]` tables in `.cargo/local-engine.toml`, which is
  gitignored, and reach for them per command:
  `scripts/local-engine.sh check -p chuzz-gui`. The wrapper snapshots and
  restores `Cargo.lock`, because a redirected build rewrites it to point at
  directories that exist on one machine. Patch only the crates you are actually
  changing; every entry is a pin that stops being tested.
- When you move the engine version, move `tauri-runtime-blitz`'s to match. Both
  resolve a ps-blitz, and two different ones put two engines in the graph, which
  surfaces as missing methods and unrelated `PaintScene` traits rather than as a
  version error. A caret on both sides makes the shared range the thing that
  keeps them equal, instead of a revision someone has to remember to move twice.
- Work on a branch and ship through a pull request. Do not commit to `main`.
- Run `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, and `cargo test --workspace --all-features` before delivery.

## Git workflow

- **Always specify the branch when pushing**: `git push origin branch-name`
- **Branch naming**: `fix/issue-description` or `feat/issue-description`
- **Force-push your own branch freely.** Rebasing a feature branch onto a moved base, or
  amending before review, is normal and correct — use `--force-with-lease` so you don't
  clobber someone else's push.
- **Never force-push the default branch.** That is the history everyone
  else builds on, and it is protected server-side for a reason.
- **Never create merge commits — this is a hard ban.** Not locally, not to refresh a
  branch, not to land a pull request. If your branch has fallen behind, **rebase** it onto
  the moved base (`git rebase origin/master`, then `--force-with-lease`). `git merge master`
  into a feature branch is not an acceptable shortcut: it adds a commit whose only content
  is the fact that you were behind, and it turns a readable line of work into a diamond.
- **Rebase is the default everywhere** — refreshing a branch, and landing a pull request.
  Individual commits carry information: what was tried, in what order, and why. A rebase
  keeps that granularity on the base branch, so write commits worth keeping and land them
  intact.
- **Landing a pull request means rebase, then fast-forward.** `git rebase origin/master`
  on the branch, then `git merge --ff-only <branch>` on the base, then push. Those two
  commands are the whole job, so don't reach for `gh pr merge`: its default writes a
  merge commit. Rebasing rewrites the commit SHAs, so GitHub cannot always detect that
  a branch landed — close such pull requests explicitly and say why.
- **Don't delete remote branches by hand.** Once the work is on the default branch it is
  reaped automatically. Deleting your own local copy is fine.
- **Squash is acceptable** where it genuinely makes things easier or is the more
  appropriate shape for the branch — one logical change scattered across fixup commits, or
  a long branch whose intermediate states aren't worth preserving. It is a judgement call,
  not a violation. Merging is the only thing that is never allowed.
- **Delete what is deprecated.** A superseded file, flag, branch or code path gets removed
  in the change that supersedes it, not left behind with a deprecation note.
