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
#  Git will not stop you doing that, so this does. It turns a class of mistake
#  that costs hours into one that costs seconds.
#
#  ── The search must not depend on git, and must never pass by accident ──────
#  This file's first version was `git grep ... 2>/dev/null || true`. Measured
#  2026-09-21 on a tree with a real unresolved marker planted in the very file
#  whose breakage it was written to prevent: it printed PASS and exited 0.
#
#  Two independent reasons, and either alone is fatal:
#
#    * `git grep` searches TRACKED files. The markers arrive via `git add -A`
#      and `cherry-pick --continue`, so in CI they are tracked — but a local
#      run before staging sees nothing, which is precisely when a human wants
#      to be told.
#    * CI checkout FALLS BACK TO A TARBALL here. With no `.git`, `git grep`
#      fails, `2>/dev/null` hides the error, `|| true` turns it into success,
#      and the empty result reads as "no markers found". The gate inspected
#      nothing in the one environment it was wired into.
#
#  So: git when there is a repo (tracked AND untracked, ignoring .gitignored
#  noise), plain recursive grep when there is not, and a NAMED refusal if
#  neither engine can run. A checker that cannot search must fail, not pass.
#
#  Run from anywhere.
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail
cd "$(dirname "$0")/.." || exit 2

# `^<<<<<<< ` and `^>>>>>>> ` with the trailing space: a bare run of angle
# brackets appears in legitimate content (heredocs, ASCII art, diff samples),
# and the marker form git writes always has a label after it. `^=======$` alone
# is too common in comment rules to be worth matching.
PATTERN='^(<<<<<<<|>>>>>>>) '

engine=""
if git rev-parse --git-dir >/dev/null 2>&1; then
    engine="git"
    # --untracked adds files not yet staged while still honouring .gitignore,
    # which is the half a bare `git grep` misses and the half a human hits.
    raw="$(git grep -nE --untracked "$PATTERN" -- . 2>/dev/null)"
    rc=$?
    # git grep: 0 = found, 1 = none found, >1 = it could not run. Only 1 is
    # allowed to mean "clean"; anything else is an engine failure, not a pass.
    if [ "$rc" -gt 1 ]; then
        echo "FAIL  git grep could not search this tree (exit $rc)."
        echo "      Refusing to report a clean tree from a search that did not run."
        exit 2
    fi
else
    engine="grep"
    # No repository — the documented CI tarball checkout. -I skips binaries so
    # a stray object file cannot make this noisy or slow.
    raw="$(grep -rInE "$PATTERN" . --exclude-dir=.git --exclude-dir=node_modules 2>/dev/null)"
    rc=$?
    if [ "$rc" -gt 1 ]; then
        echo "FAIL  grep could not search this tree (exit $rc)."
        echo "      Refusing to report a clean tree from a search that did not run."
        exit 2
    fi
fi

hits="$(printf '%s' "$raw" | grep -v 'check-no-conflict-markers.sh' || true)"

if [ -n "$hits" ]; then
    echo "FAIL  the tree still contains merge conflict markers:"
    printf '%s\n' "$hits" | sed 's/^/      /'
    echo
    echo "      Resolve them, then re-run. If a file legitimately contains a"
    echo "      line starting with seven angle brackets and a space, this check"
    echo "      needs an exclusion rather than a workaround."
    exit 1
fi

echo "PASS  no merge conflict markers in the tree (searched with $engine)"
