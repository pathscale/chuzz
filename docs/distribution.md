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

## Why the URL carries no version

Releases overwrite `Chuzz.app.tar.gz` at a fixed path rather than accumulating
one file per version, the same way AgencyZero does. Two consequences, both
deliberate:

- `latest.json` is the only record of the current version.
- The cask is pinned to `version :latest` with `sha256 :no_check`, because the
  bytes behind a fixed URL change. The upside is that the cask never needs a
  commit per release, so no release touches the tap repo at all.

The cost is that the release workflow has to purge the BunnyCDN edge cache, and
has to do it in the right order: the tarball goes live and is confirmed live
*before* `latest.json` advertises it. Reversed, a client reads the new version
and fetches an edge-cached older tarball. Those purge steps are hard failures in
[`release.yml`](../.github/workflows/release.yml) for that reason.

**One thing this costs that it does not cost AgencyZero.** Homebrew cannot
detect a new release, and AgencyZero does not care because its self-updater
pulls it forward. Chuzz has no updater yet, so until it does, moving an
installed copy forward is `brew reinstall --cask chuzz`.

## Signing

Two signatures, unrelated to each other, and conflating them wastes an
afternoon.

**Apple ad-hoc signing** is `codesign --sign -`, applied by
`apps/chuzz/build-app.sh`. It takes no certificate and no secret; that is what
"ad-hoc" means. The release workflow verifies the result and asserts the bundle
is still ad-hoc, so acquiring a Developer ID certificate cannot silently leave
the cask stripping a Gatekeeper check users should be getting.

**minisign signing** is applied by the release runner to the tarball, producing
`Chuzz.app.tar.gz.sig` beside it and the `signature` field in `latest.json`. It
signs the payload rather than the bundle, so a client can check that the bytes
it fetched came from this pipeline. It uses `TAURI_SIGNING_PRIVATE_KEY`, the
same rsign2 key and format Tauri uses, which is why the signing step shells out
to the Tauri CLI's `signer sign` rather than reimplementing it.

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
| `TAURI_SIGNING_PRIVATE_KEY` | signing the tarball, see above |

The cask itself needs no secret and no automation: [`packaging/chuzz.rb`](../packaging/chuzz.rb)
is copied into `pathscale/homebrew-tap` as `Casks/chuzz.rb` once, and a release
never touches it again.

## The engine the release builds against

Chuzz path-depends on `../ps-blitz`, which on a developer's machine is a local
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
