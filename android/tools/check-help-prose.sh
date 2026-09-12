#!/usr/bin/env bash
# Does the in-app guide read like something a person wrote?
#
# P1-060 carries the `stop_slop` gate, and the desktop's own help has the same
# one (`apex-shell/tests/check-agent-help.sh`, which reports TOTAL 0). This is
# that check for the Android guide.
#
# It scans PROSE, not Kotlin. `help-prose.py` pulls out exactly what a user
# sees — section titles, block texts, kv terms, the entry label — because
# handing the checker `Help.kt` hands it declarations too, and three one-line
# factory functions in a row read as three sentences of the same length. That
# is true, and it is not prose. The desktop splits its words into their own
# file for the same reason.
#
# It reports NOT CHECKED and exits 0 when the stop-slop skill is not installed.
# A script that printed "ok" because it could not look would be the
# "permission denied is not absence" mistake in a shell script — the same rule
# check-agent-state.sh follows.
set -uo pipefail

here="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
help="$here/../core/src/main/kotlin/com/apexos/remote/core/agent/Help.kt"
allow="$here/../.slopcheck-allow"
slop="${SLOPCHECK:-$HOME/.claude/skills/stop-slop/scripts/slopcheck.py}"

if [ ! -f "$help" ]; then
    echo "FAIL: $help is missing, and this check is about its contents"
    exit 1
fi

prose="$(mktemp)"
trap 'rm -f "$prose"' EXIT
if ! python3 "$here/help-prose.py" "$help" > "$prose"; then
    echo "FAIL: the guide's prose could not be extracted from $help"
    exit 1
fi
# An extractor that silently matched nothing would make this pass on an empty
# file, which is the same failure in a different place.
words="$(wc -w < "$prose")"
if [ "$words" -lt 500 ]; then
    echo "FAIL: only $words words were extracted from the guide; the extractor has stopped matching"
    exit 1
fi

if [ ! -f "$slop" ]; then
    echo "NOT CHECKED: slopcheck is not installed at $slop"
    echo "  ($words words of guide prose were extracted and not examined)"
    exit 0
fi

args=("$slop")
[ -f "$allow" ] && args+=(--allow-file "$allow")
out="$("${args[@]}" "$prose" 2>&1)"
total="$(printf '%s\n' "$out" | sed -n 's/^TOTAL: \([0-9]*\)$/\1/p' | tail -1)"

if [ -z "$total" ]; then
    echo "FAIL: slopcheck printed no TOTAL, so nothing was measured"
    printf '%s\n' "$out"
    exit 1
fi
if [ "$total" != "0" ]; then
    printf '%s\n' "$out"
    echo "FAIL: $total slop hits in $words words of guide prose"
    exit 1
fi

echo "OK: $words words of guide prose, slopcheck TOTAL 0"
if [ -f "$allow" ]; then
    echo "  (with exemptions from $allow)"
else
    echo "  (no allowlist; nothing is exempted)"
fi
