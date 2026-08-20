#!/usr/bin/env bash
#
# Fail if Cargo.lock resolves any git dependency at more than one revision.
#
# Cargo treats two revs of one repository as two unrelated crates. Both get
# built, both export the same type names, and the types do not unify. Here that
# surfaces as
#
#   error[E0277]: the trait bound `VelloCpuScenePainter: PaintScene` is not satisfied
#   note: there are multiple different versions of crate `ps_anyrender_vello_cpu`
#
# which reads as a broken engine rather than as two of them.
#
# The engine is a cascade: ps-anyrender -> ps-blitz -> tauri-runtime-blitz ->
# chuzz. Repinning fewer than all of them does not leave the tree one release
# behind, it puts two copies of a crate in the graph. Every manifest in this
# workspace moves together, and `tauri-runtime-blitz` has to have been
# republished at the ps-blitz rev this repository wants.
#
# Worth its own check because chuzz's only build was, for a long time, the macOS
# release job: a mismatched rev reached master unchallenged and failed after the
# merge. Adapted from agencyzero's script of the same name, where the same class
# of mismatch cut 0.6.0 with a bundle that could not build.
#
# Reading the lockfile rather than running cargo keeps it honest on any runner
# and costs nothing, and the lockfile is the resolution the release job uses.
set -euo pipefail

lock="${1:-Cargo.lock}"

# `source = "git+URL?rev=SHA#SHORTSHA"` - strip the fragment, split on `?rev=`.
#
# `|| true` on the grep because a lockfile with no git sources at all is the
# goal, not a failure, and grep exits 1 when it matches nothing. Under
# `pipefail` that failed the check on precisely the lockfile it most wants to
# see, which is what happened the day the last git dependency went away.
duplicates=$(
    { grep -o 'source = "git+[^"]*"' "$lock" || true; } |
        sed 's/source = "git+//; s/"$//; s/#.*//' |
        sort -u |
        awk -F'\\?rev=' 'NF == 2 { count[$1]++; revs[$1] = revs[$1] "\n    " $2 }
                       END { for (url in count) if (count[url] > 1) print url revs[url] }'
)

if [[ -n $duplicates ]]; then
    echo "Cargo.lock resolves a git dependency at more than one revision:" >&2
    echo "$duplicates" >&2
    echo >&2
    echo "Point every manifest at the same rev. A dependency that pins one of" >&2
    echo "these itself has to be republished at the rev this repository wants." >&2
    exit 1
fi

if grep -q 'source = "git+' "$lock"; then
    echo "one rev per git source"
else
    echo "no git sources"
fi
