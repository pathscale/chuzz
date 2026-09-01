#!/usr/bin/env python3
"""Diff a chuzz capture tree against a reference-browser box dump.

Both sides describe the same page as boxes. The reference is the authority on
what the page is meant to look like; a divergence is a chuzz defect until shown
otherwise. Elements are keyed by identity that survives both trees: `tag#id`
where an id exists, else `tag.classes` plus an ordinal.

Nothing here ever sees a hostname. It is handed two files and an opaque id.
"""
import json
import re
import sys

# `    [NodeId(67v1)] body.a.b  0,0 1440*960  Display(514) Static`
CHUZZ = re.compile(
    r"^\s*\[NodeId\([^)]*\)\]\s+(?P<tag>[a-zA-Z0-9_-]+)"
    r"(?P<sel>[#.][^\s]*)?\s+"
    r"(?P<x>-?\d+),(?P<y>-?\d+)\s+(?P<w>\d+)\*(?P<h>\d+)"
)


def key_of(tag, sel, seen):
    ident = tag
    if sel:
        m = re.match(r"#([^.]+)", sel)
        ident = f"{tag}#{m.group(1)}" if m else f"{tag}{sel}"
    n = seen.get(ident, 0)
    seen[ident] = n + 1
    return ident if n == 0 else f"{ident}[{n}]"


def load_chuzz(path):
    boxes, seen = {}, {}
    for line in open(path, encoding="utf-8", errors="replace"):
        m = CHUZZ.match(line)
        if not m:
            continue
        k = key_of(m.group("tag"), m.group("sel"), seen)
        boxes[k] = tuple(int(m.group(g)) for g in ("x", "y", "w", "h"))
    return boxes


def load_ref(path):
    boxes, seen = {}, {}
    for e in json.load(open(path, encoding="utf-8")):
        k = key_of(e["tag"], e.get("sel") or "", seen)
        boxes[k] = tuple(int(round(e[g])) for g in ("x", "y", "w", "h"))
    return boxes


def main():
    site_id, chuzz_path, ref_path = sys.argv[1], sys.argv[2], sys.argv[3]
    tol = int(sys.argv[4]) if len(sys.argv) > 4 else 2
    ours, theirs = load_chuzz(chuzz_path), load_ref(ref_path)

    shared = sorted(set(ours) & set(theirs))
    # An element the reference paints and we do not is the worst class of
    # defect, so it is counted separately from one that merely moved.
    missing = sorted(set(theirs) - set(ours))
    extra = sorted(set(ours) - set(theirs))

    drift = []
    for k in shared:
        a, b = ours[k], theirs[k]
        delta = max(abs(x - y) for x, y in zip(a, b))
        if delta > tol:
            drift.append((delta, k, a, b))
    drift.sort(reverse=True)

    agree = len(shared) - len(drift)
    print(f"{site_id}: {len(shared)} shared elements, {agree} agree within {tol}px "
          f"({100 * agree // max(len(shared), 1)}%)")
    print(f"{site_id}: {len(missing)} absent from chuzz, {len(extra)} only in chuzz")
    for delta, k, a, b in drift[:25]:
        print(f"  {delta:>6}px  {k}")
        print(f"          chuzz {a[0]},{a[1]} {a[2]}*{a[3]}   ref {b[0]},{b[1]} {b[2]}*{b[3]}")
    for k in missing[:15]:
        b = theirs[k]
        print(f"  ABSENT  {k}   ref {b[0]},{b[1]} {b[2]}*{b[3]}")


if __name__ == "__main__":
    main()
