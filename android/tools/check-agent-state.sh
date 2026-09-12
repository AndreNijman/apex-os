#!/usr/bin/env bash
# Do the vendored copies of the desktop's tables still match apex-shell?
#
# `AgentStateAgreementTest` proves the Kotlin agrees with
# `core/src/test/resources/desktop/agentstate.js`, and `ToneColoursTest` proves
# it agrees with `desktop/Colors.qml`. Those are agreements with a SNAPSHOT.
# The two repositories are separate and neither CI checks out the other, so
# this is the part that can only be run where both exist.
#
# It reports NOT CHECKED and exits 0 when there is no apex-shell to compare
# against. A script that printed "ok" because it could not look would be the
# "permission denied is not absence" mistake, in a shell script.
#
# One more thing it checks, and the reason is the same mistake in a different
# costume: apex-shell's DEFAULT branch does not have these files in the state
# they were vendored from — `info` and `attention` only exist on `roadmap/v2.2`
# — so a checkout sitting on some other branch would produce a confident,
# meaningless diff. The branch is named in the report.
set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
vendored_dir="$here/../core/src/test/resources/desktop"
shell_dir="${APEX_SHELL_DIR:-/var/tmp/apex-work/int-shell}"

# vendored basename -> path within apex-shell
pairs=(
    "agentstate.js|src/services/agentstate.js"
    "Colors.qml|src/theme/Colors.qml"
)

status=0
checked=0
skipped=0

for entry in "${pairs[@]}"; do
    name="${entry%%|*}"
    rel="${entry#*|}"
    vendored="$vendored_dir/$name"
    live="$shell_dir/$rel"

    if [ ! -f "$vendored" ]; then
        echo "FAIL: the vendored copy is missing: $vendored"
        status=1
        continue
    fi

    if [ ! -r "$live" ]; then
        echo "NOT CHECKED: $name — no readable apex-shell file at $live"
        echo "  the vendored copy is $(sha256sum "$vendored" | cut -c1-16)... (see desktop/PROVENANCE.md)"
        skipped=$((skipped + 1))
        continue
    fi

    if diff -q "$vendored" "$live" >/dev/null 2>&1; then
        echo "OK: $name is identical to $live"
        echo "  sha256 $(sha256sum "$vendored" | cut -d' ' -f1)"
        checked=$((checked + 1))
        continue
    fi

    echo "FAIL: apex-shell's $rel has changed since it was vendored."
    echo "  vendored: $vendored"
    echo "  live:     $live"
    echo
    diff -u "$vendored" "$live" | head -60
    echo
    status=1
done

if [ "$skipped" -gt 0 ]; then
    echo
    echo "  set APEX_SHELL_DIR to an apex-shell checkout to compare the $skipped that were skipped"
fi

if [ "$checked" -gt 0 ] && [ -e "$shell_dir/.git" ]; then
    branch="$(git -C "$shell_dir" rev-parse --abbrev-ref HEAD 2>/dev/null || echo unknown)"
    echo
    echo "  compared against $shell_dir on branch '$branch'"
    if [ "$branch" != "roadmap/v2.2" ]; then
        echo "  NOTE: PROVENANCE.md vendored from roadmap/v2.2, and this checkout is not on it."
        echo "        A difference reported above may be a branch difference, not a drift."
    fi
fi

if [ "$status" -ne 0 ]; then
    echo
    echo "Re-vendor the changed file, update desktop/PROVENANCE.md, and re-run"
    echo "AgentStateAgreementTest and ToneColoursTest -- which is what will tell"
    echo "you whether the Kotlin tables need to move too."
fi

exit "$status"
