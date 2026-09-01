#!/usr/bin/env python3
"""Turn raw corpus capture counts into a verdict per site.

The counts alone separate the coarse outcomes. What they cannot separate is a
page we failed to render from a page we were never sent, and that distinction
decides whether a row is an engine defect or a bot wall. Anything landing in
BLANK or SPARSE is a question for the reference browser, not an answer.
"""
import sys


# A refusal is only a refusal when the *document* was refused.
#
# The first version of this matched `HttpStatus { status: N }` anywhere in the
# log, which also matches a third-party script that came back 403 on a page that
# loaded perfectly. Two sites with thousands of nodes were scored REFUSED that
# way, so the refusal count read 23 when it was 21 and the render count read 59
# when it was 61. Match the `capture failed:` line, which is the document.
CAPTURE_FAILED = "capture failed: HttpStatus"


def verdict(status, nodes, boxed, errs):
    if status != 0:
        return "TIMEOUT"
    if nodes < 10:
        return "BLANK"      # no document worth the name: stub, block, or refusal
    if boxed < 20:
        return "SPARSE"     # a document arrived, almost none of it took a box
    if errs >= 10:
        return "NOISY"      # painted, but the script environment is failing hard
    return "PAINTED"


def main():
    rows, bad = [], 0
    for line in open(sys.argv[1] if len(sys.argv) > 1 else "/tmp/corpus-results.tsv"):
        parts = line.rstrip("\n").split("\t")
        if len(parts) != 5:
            bad += 1
            continue
        site, status, nodes, boxed, errs = parts
        try:
            rows.append((site, verdict(int(status), int(nodes), int(boxed), int(errs)),
                         int(nodes), int(boxed), int(errs)))
        except ValueError:
            bad += 1

    order = ["TIMEOUT", "BLANK", "SPARSE", "NOISY", "PAINTED"]
    counts = {k: 0 for k in order}
    for _, v, *_ in rows:
        counts[v] += 1

    print(f"{len(rows)} sites captured" + (f" ({bad} malformed rows skipped)" if bad else ""))
    for k in order:
        if counts[k]:
            print(f"  {k:<8} {counts[k]:>3}  ({100 * counts[k] // len(rows)}%)")
    print()
    for k in order:
        listed = [r for r in rows if r[1] == k]
        if not listed or k == "PAINTED":
            continue
        print(f"{k}:")
        for site, _, nodes, boxed, errs in listed:
            print(f"  {site}  {nodes:>6} nodes  {boxed:>5} boxed  {errs:>3} js errors")


if __name__ == "__main__":
    main()
