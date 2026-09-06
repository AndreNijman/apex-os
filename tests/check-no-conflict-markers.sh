#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  check-no-conflict-markers.sh — refuse a tree that still has merge markers.
#
#  ── Why ─────────────────────────────────────────────────────────────────────
#  `roadmap/v2.2` shipped with unresolved markers in src/state/IpcManager.qml
#  and tests/run-agent-center-smoke.sh. The shell does not load at all in that
#  state, and it went unnoticed for hours because the conflict had been resolved
#  in the file the resolver was *looking* at, and `git add -A` then staged the
#  other two with their markers intact. `cherry-pick --continue` committed them
#  without complaint.
#
#  Git will not stop you doing that, so this does. It is three lines of grep and
#  it turns a class of mistake that costs hours into one that costs seconds.
#
#  Run from anywhere.
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail
cd "$(dirname "$0")/.." || exit 2

# `^<<<<<<< ` and `^>>>>>>> ` with the trailing space: a bare run of angle
# brackets appears in legitimate content (heredocs, ASCII art, diff samples),
# and the marker form git writes always has a label after it. `^=======$` alone
# is too common in comment rules to be worth matching.
hits="$(git grep -nE '^(<<<<<<<|>>>>>>>) ' -- . 2>/dev/null \
        | grep -v 'check-no-conflict-markers.sh' || true)"

if [ -n "$hits" ]; then
    echo "FAIL  the tree still contains merge conflict markers:"
    printf '%s\n' "$hits" | sed 's/^/      /'
    echo
    echo "      Resolve them, then re-run. If a file legitimately contains a"
    echo "      line starting with seven angle brackets and a space, this check"
    echo "      needs an exclusion rather than a workaround."
    exit 1
fi

echo "PASS  no merge conflict markers in the tree"
