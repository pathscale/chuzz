# Distribution

How Chuzz reaches a Mac that is not this one. Apple Silicon only, macOS 11 or
later.

## Install

```bash
brew tap pathscale/tap
brew trust pathscale/tap
brew install --cask chuzz
```

The `brew trust` step is required: Homebrew refuses to load a cask from a
third-party tap until the tap is trusted.

**This is the only install route that works.** Handing someone the tarball URL
does not, and the reason is worth understanding rather than rediscovering.

## Why Homebrew and not a download link

The bundle is ad-hoc signed, not notarized, so Gatekeeper rejects it: `spctl -a`
returns `rejected` on every Mac including the one that built it. Whether that
rejection is ever consulted comes down to the `com.apple.quarantine` flag, which
is set by the *downloading* program rather than by macOS:

| Arrival route | Quarantined? | Launches? |
|---|---|---|
| Built locally | No | Yes |
| `brew install --cask` | Yes, then stripped by the cask's `postflight` | Yes |
| Browser download | Yes | No, reports itself as damaged |
| `curl` / `scp` | No | Yes |

Ad-hoc signatures are portable. There is nothing machine-specific about them, so
an unquarantined copy runs anywhere. The cask's `postflight` removes the flag
Homebrew applies, and that single line is what makes the install work.

macOS Sequoia removed the Control-click bypass, so a user who downloads the
tarball in a browser has no quick way out: System Settings, Privacy & Security,
Open Anyway.

## Ad-hoc signing needs no key

Worth stating because the neighbouring project's setup invites the opposite
conclusion. `apps/chuzz/build-app.sh` signs with `codesign --sign -`, which takes
no certificate and no secret. That is what "ad-hoc" means. The release workflow
verifies the result and fails if the bundle ever stops being ad-hoc, because at
that point the cask's quarantine `postflight` should be dropped instead.

AgencyZero's `TAURI_SIGNING_PRIVATE_KEY` is a **minisign** key, unrelated to
Apple signing: it signs the update payloads its self-updater verifies. Chuzz has
no updater, so nothing here would ever read such a signature and the secret is
not used.

## Why the URL carries the version

The opposite of AgencyZero, deliberately.

AgencyZero overwrites a fixed, versionless path and pins its cask to
`version :latest` with `sha256 :no_check`. It can, because it carries a
self-updater that pulls it forward from the same CDN, so `brew upgrade` never
needing to fire is a feature rather than a defect.

**Chuzz has no updater.** A versionless URL would mean Homebrew cannot detect a
new release and the app cannot update itself either, so every installed copy
would sit on whatever it first fetched until someone thought to
`brew reinstall`. So each release writes `Chuzz-<version>.app.tar.gz`, the cask
pins a real `version` and a real `sha256`, and `brew upgrade` works.

That also removes the failure mode that dominates AgencyZero's pipeline, where
overwriting a live object let an edge cache a partial response and serve a
truncated gzip for as long as its max-age allowed. Each release here writes a
path no edge has ever held, so the purge is belt and braces rather than the
thing holding the release together.

The cost is a commit to the tap per release, which the workflow makes for you
when `TAP_GITHUB_TOKEN` is set.

## Cutting a release

Bump `[workspace.package] version` in the root `Cargo.toml` and push to master.
That is the whole ritual.

That one field is the only version in the repo. `apps/chuzz/build-app.sh` stamps
it into the bundle's `Info.plist` at build time, and the workflow refuses to
publish if the two disagree, so the cask can never advertise a version the app
does not report.

[`release.yml`](../.github/workflows/release.yml) decides whether to publish by
comparing that version against the one in the **live** `latest.json`, not against
git history. That makes it idempotent: a release that failed halfway is retried
by the next push, a re-run is harmless, and squashes and force-pushes cannot
confuse it. A push that changes code but not the version publishes nothing.

`workflow_dispatch` takes a `force` input for republishing the same version.

## Secrets

Four, all on `pathscale/chuzz`:

| Secret | Used for |
|---|---|
| `BUNNYCDN_STORAGE_API_KEY` | uploading the tarball and `latest.json` |
| `BUNNYCDN_STORAGE_NAME` | the storage zone name |
| `BUNNYCDN_ZONE_API_KEY` | purging the pull zone |
| `BUNNYCDN_ZONE_ID` | which pull zone to purge |

Optional: `TAP_GITHUB_TOKEN`, with push access to `pathscale/homebrew-tap`. With
it the workflow commits the rendered cask; without it the release still
completes and attaches the cask as a build artifact to paste in by hand. A
missing token is not a release failure, because failing there would leave a
published tarball nobody can install.

## The engine the release builds against

Chuzz path-depends on `../blitz-rust`, which on a developer's machine is a local
checkout and in CI is a fresh clone of `pathscale/ps-blitz` at the ref named in
[`release.yml`](../.github/workflows/release.yml).

**Those are not the same engine today.** The local checkout sits on a branch that
is 58 commits behind that repository's master, with uncommitted changes on top.
Until those are reconciled, a CI-built Chuzz.app is not the binary anyone has
been testing. See
[HANDOVER-24x-rendering.md](HANDOVER-24x-rendering.md#the-engine-tree-has-forked-and-that-needs-a-decision).

## What buying a Developer ID would change

Signing and notarizing is worth roughly $99/year. It would let a browser download
work and drop the `postflight` quarantine strip from the cask. The bundle script
would take a real identity instead of `-`, and the workflow's ad-hoc assertion
becomes the reminder to update both.
