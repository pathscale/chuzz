# Corpus captures

`../render-check.sh` captures a page headlessly and writes a PNG, a tree dump
and a log. This directory holds the notes for running that over a large set of
real sites.

## Anonymity is the point, not a detail

No site name appears in this repository. Captures are labelled with opaque ids
(`s001`…), and the only mapping from id to hostname lives outside any checkout,
in `~/.chuzz-corpus/corpus.map.tsv`. Never commit it, never quote a hostname in
a commit message, an issue, or a PR.

That is not satisfied by renaming the file. A capture log names every CDN it
fetched from, and a tree dump carries class names and element ids that identify
a site as surely as a hostname would. Hostname matching alone is not enough
either: one leak survived it because the site's name was in a CSS class.
**Read what you are about to quote, and redact every distinctive word.**

## Reading a run

Label every capture with the opaque id, so nothing under `target/` carries a
site name:

    CHUZZ_CAPTURE_SCALE=1 RENDER_CHECK_TIMEOUT=45 \
      scripts/render-check.sh s001 "$url"

Four numbers come back per site: exit status, node count, boxed count, console
lines. They separate the coarse outcomes and nothing finer. Some traps:

- **Read the logs, not the exit code.** `render-check.sh` exits non-zero for
  both a watchdog kill and a failed load, so a site refused with HTTP 403 looks
  identical to one that timed out. A whole corpus was once reported as 26
  timeouts when 21 of them were refusals. The `capture failed:` line in the log
  is the document-level answer; a bare `HttpStatus` anywhere else may be a
  third-party script on a page that loaded perfectly.
- **A refusal is not a rendering fault.** A fifth of a 104-site corpus never
  reaches the engine at all, so the engine-relevant denominator is not the
  corpus size.
- **Clear `target/render-check` between runs, or scope your greps to the ids
  you just captured.** Labels from earlier runs sit alongside the current ones
  and silently double any count taken with a glob.
- **Bucket counts cannot measure an engine change.** Across two runs a day
  apart, nine sites changed verdict and every one traced to a bot wall
  flipping, a site serving different content, or an error count crossing a
  threshold with node counts unchanged. Count specific error signatures
  instead; those move only when the engine does.

## No tooling here

There were three Python scripts here (a classifier, a tree-vs-reference diff,
and a log scrubber). They have been removed. The scrubbing the anonymity
section calls for is currently a manual step, and if that becomes a burden the
replacement should be Rust, like everything else in this repository.
