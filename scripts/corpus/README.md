# Corpus tooling

Support for capturing a large set of real sites through `chuzz-gui --capture`
and judging what came back. `../render-check.sh` does the capturing; these do
the reading.

## Anonymity is the point, not a detail

No site name appears in this repository. Captures are labelled with opaque ids
(`s001`…), and the only mapping from id to hostname lives outside any checkout,
in `~/.chuzz-corpus/corpus.map.tsv`. Never commit it, never quote a hostname in
a commit message, an issue, or a PR.

That is not satisfied by renaming the file. A capture log names every CDN it
fetched from, and a tree dump carries class names and element ids that identify
a site as surely as its hostname would. **Scrub before quoting anything.**

## The tools

### `scrub.py` — remove identity from a log or a tree dump

    python3 scrub.py --identity <token> < s101.log.txt > safe.txt

Replaces URLs and hostnames with stable per-file tokens, so two mentions of one
host stay recognisably the same host and an error about a script is still
traceable to the request that fetched it.

`--identity` takes a word to redact, from the local-only map, and it is not
optional in practice: hostname matching alone left one leak in a tree dump,
because the site's name was in a CSS class. Pass every distinctive word.

### `classify.py` — a verdict per site

    python3 classify.py results.tsv

Reads a TSV of `id, status, nodes, boxed, errors` and buckets each site.

**Read the logs, not the exit code.** `render-check.sh` exits non-zero for both
a watchdog kill and a failed load, so a run that was refused with HTTP 403 looks
identical to one that timed out. A whole corpus was once reported as "26
timeouts" that were really 15x403 plus assorted 401/429/406/404/400 — the
servers declined to serve the client, and no amount of rendering work would have
changed it.

### `compare.py` — diff a capture against a reference browser

    python3 compare.py <site-id> <capture>.tree.txt <reference>.json

Both sides describe the page as boxes; the reference is the authority. Elements
are keyed by `tag#id` where an id exists, else `tag.classes` plus an ordinal.

The reference dump is a `getBoundingClientRect` sweep taken from a real browser.
Getting it out of that browser is the awkward part and has no committed tool,
deliberately — see below.

## Reading the numbers without fooling yourself

- **Node count is not monotonic under improvement.** A page that starts working
  can *drop* from 215 nodes to 28, because it finally cleared its
  server-rendered markup, began rebuilding, and died at the next missing API.
  That is the measurement working. Compare trees, not counts.
- **Check it is the same page.** A WAF interstitial is a 28-node "please wait"
  that reads as a catastrophic regression.
- **Viewports must match.** Two engines laying the same page out at different
  widths disagree on every percentage width and every breakpoint, so a diff
  becomes noise that reads exactly like a rendering fault. `render-check.sh`
  honours `CHUZZ_CAPTURE_WIDTH`/`HEIGHT`; it did not always, and a re-capture at
  another width returned a byte-identical tree, which is how that was found.
- **`X is not defined` is only half the missing APIs.** A method absent from an
  existing prototype throws `TypeError: not a callable function`, which names
  nothing and ranks nowhere. That string covered 48 of 104 sites in the first
  corpus run and is the largest unexamined surface in it.

## Why there is no collector here

An earlier version shipped a loopback HTTP collector for the reference browser
to POST its dump to. It is not here because it does not work, and the reasons
are worth keeping so nobody rebuilds it:

- a collector started from a sandboxed shell binds that sandbox's own loopback,
  so `curl` from the same shell reaches it and the browser never does;
- Brave blocks page-to-localhost requests outright — no preflight arrives at the
  socket at all, and the fetch hangs rather than failing;
- `navigator.clipboard` refuses on a background tab, "Document is not focused";
- a Blob download does write, but stalls behind the save dialog unless "ask
  where to save each file" is off.

Pulling a filtered dump back through the automation tool works and needs no
infrastructure. Prefer it.
