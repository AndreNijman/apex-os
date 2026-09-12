#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  check-suites-run-in-ci.sh — a suite nobody runs is not a gate.
#
#  CI invokes suites BY NAME. Writing tests/test-foo.sh, landing it, and never
#  adding a line to a workflow leaves a file that looks like coverage, passes
#  review, and gates nothing. Measured 2026-09-12: 70 test-*.sh suites existed
#  and 13 were named in no workflow — among them tests/test-apex-lid.sh, sixty
#  assertions for a feature the owner had asked for by name, which no CI run
#  had ever executed.
#
#  Same species as a path selector that matches nothing and a lint list that
#  omits new files: the step runs, reports success, and inspects nothing. This
#  repository's own workflow comments record five instances.
#
#  A suite must therefore be EITHER named in .github/workflows/, OR listed in
#  tests/suites-not-in-ci.txt with the reason on the same line. Both arms fail:
#
#    a suite in neither place            -> fail, named
#    a listed suite now run by CI        -> fail, "delete the line"
#
#  The second arm is what stops the exemption list becoming permanent.
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail
cd "$(dirname "$0")/.." || exit 1

EXEMPT=tests/suites-not-in-ci.txt
WORKFLOWS=.github/workflows

[ -d "$WORKFLOWS" ] || { echo "FATAL: no $WORKFLOWS directory"; exit 1; }

# A filename is never split mid-token by a line continuation -- a backslash
# wraps BETWEEN arguments -- so the raw files are searched directly. An earlier
# draft joined continuations with sed first; the escaping inside its own
# heredoc differed from what was tested at the prompt, and it silently matched
# nothing, reporting suites as unrun that CI runs twice. Simpler is checkable.
exempt=()
[ -f "$EXEMPT" ] && mapfile -t exempt < <(grep -vE '^\s*(#|$)' "$EXEMPT" | awk '{print $1}')
is_exempt() { local n; for n in ${exempt[@]+"${exempt[@]}"}; do [ "$n" = "$1" ] && return 0; done; return 1; }

missing=(); resurrected=(); run=0
for s in tests/test-*.sh; do
    [ -f "$s" ] || continue
    base="${s#tests/}"
    if grep -rqF "$base" "$WORKFLOWS"/; then
        run=$((run + 1))
        is_exempt "$s" && resurrected+=("$s")
    else
        is_exempt "$s" || missing+=("$s")
    fi
done

total=$(ls tests/test-*.sh 2>/dev/null | wc -l | tr -d ' ')
printf '\nsuite coverage: %s suites, %d run by CI, %d exempt, %d unrun and undeclared\n' \
    "$total" "$run" "${#exempt[@]}" "${#missing[@]}"

rc=0
if [ "${#missing[@]}" -gt 0 ]; then
    echo "  FAIL — these suites are named in no workflow and are not in $EXEMPT:"
    printf '    %s\n' "${missing[@]}"
    echo "  Add the suite to a workflow, or record it in $EXEMPT with the reason."
    rc=1
fi
if [ "${#resurrected[@]}" -gt 0 ]; then
    echo "  FAIL — these are in $EXEMPT but CI now runs them. Delete their lines:"
    printf '    %s\n' "${resurrected[@]}"
    rc=1
fi
[ "$rc" -eq 0 ] && echo "  every suite is either run by CI or a recorded, reasoned exception"
exit "$rc"
