#!/usr/bin/env bash
# Does the vendored copy of the desktop's state table still match apex-shell?
#
# `AgentStateAgreementTest` proves the Kotlin agrees with
# `core/src/test/resources/desktop/agentstate.js`. That is agreement with a
# SNAPSHOT. The two repositories are separate and neither CI checks out the
# other, so this is the part that can only be run where both exist.
#
# It reports NOT CHECKED and exits 0 when there is no apex-shell to compare
# against. A script that printed "ok" because it could not look would be the
# "permission denied is not absence" mistake, in a shell script.
set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
vendored="$here/../core/src/test/resources/desktop/agentstate.js"
shell_dir="${APEX_SHELL_DIR:-/var/tmp/apex-work/int-shell}"
live="$shell_dir/src/services/agentstate.js"

if [ ! -f "$vendored" ]; then
    echo "FAIL: the vendored copy is missing: $vendored"
    exit 1
fi

if [ ! -r "$live" ]; then
    echo "NOT CHECKED: no readable apex-shell checkout at $shell_dir"
    echo "  the vendored copy is $(sha256sum "$vendored" | cut -c1-16)... (see desktop/PROVENANCE.md)"
    echo "  set APEX_SHELL_DIR to an apex-shell checkout to compare"
    exit 0
fi

if diff -q "$vendored" "$live" >/dev/null 2>&1; then
    echo "OK: the vendored agentstate.js is identical to $live"
    echo "  sha256 $(sha256sum "$vendored" | cut -d' ' -f1)"
    exit 0
fi

echo "FAIL: apex-shell's agentstate.js has changed since it was vendored."
echo "  vendored: $vendored"
echo "  live:     $live"
echo
diff -u "$vendored" "$live" | head -60
echo
echo "Re-vendor it, update desktop/PROVENANCE.md, and re-run"
echo "AgentStateAgreementTest -- which is what will tell you whether the"
echo "Kotlin table needs to move too."
exit 1
