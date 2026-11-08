# Distribution

How Chuzz reaches a Mac that is not this one. Apple Silicon only, macOS 11 or
later.

## Homebrew

The generated cask pins the archive by version and SHA-256 and preserves
macOS quarantine. Chuzz release bundles are ad-hoc signed, so Gatekeeper can
block the quarantined app from launching.

The release workflow publishes the cask archive at
`https://24x.ai/chuzz/releases/<version>/<sha256>/Chuzz.app.tar.gz`.
It verifies the downloaded bytes before saving the `chuzz-homebrew-cask`
artifact generated from [`packaging/chuzz.rb.in`](../packaging/chuzz.rb.in).
Use the generated file to update `Casks/chuzz.rb` in `pathscale/homebrew-tap`.
Keep prior content-addressed archives available for existing cask versions.

## Signing

The release build uses an ad-hoc macOS code signature.

**minisign signing** is applied by the release runner to the tarball, producing
`Chuzz.app.tar.gz.sig` beside it and the `signature` field in `latest.json`. It
signs the payload rather than the bundle, so a client can check that the bytes
it fetched came from this pipeline. It uses `TAURI_SIGNING_PRIVATE_KEY`, the
same rsign2 key and format Tauri uses, which is why the signing step shells out
to the Tauri CLI's `signer sign` rather than reimplementing it.

## Cutting a release

Bump `[workspace.package] version` in the root `Cargo.toml`, land it on
`master`, and dispatch the Release workflow.

That one field is the only version in the repo. `apps/chuzz/build-app.ts` stamps
it into the bundle's `Info.plist` at build time, and the workflow refuses to
publish if the two disagree, so the cask can never advertise a version the app
does not report.

The same release builds `chuzz-headless` on Linux x86_64 with capture,
JavaScript, scrollbars, WebP, and system-font discovery enabled. It publishes a
versioned GitHub prerelease asset and SHA-256 under `headless-v<version>`. Fleet
sites consume that verified binary through the shared headless-host action, so
the browser is compiled once in its own repository rather than once per site.
The action falls back to the identical source build if the release asset is not
yet available.

[`release.yml`](../.github/workflows/release.yml) decides whether to publish by
comparing that version against the one in the **live** `latest.json`, not against
git history. A later dispatch can retry a version that never went live.

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

## The engine the release builds against

Chuzz path-depends on `../ps-blitz`, which on a developer's machine is a local
checkout and in CI is a fresh clone of `pathscale/ps-blitz` at the ref named in
[`release.yml`](../.github/workflows/release.yml).

The release workflow pins the exact tested engine commit rather than a branch.
Move that SHA deliberately when adopting engine work, and test the render corpus
before releasing. A floating `master` would let the same Chuzz version produce
different binaries on different days.
