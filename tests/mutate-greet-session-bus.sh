#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  mutate-greet-session-bus.sh — prove test-apex-greet-session-bus.sh can go red.
#
#  That suite asserts ABSENCE: no session bus, no accessibility bus, on either
#  host. Absence is the easiest thing in the world to assert by accident — a
#  probe that never runs, a chain that dies on its first line and a grep that
#  finds nothing all look exactly like the truth. So each mutant here either
#  SUPPLIES the missing thing (the suite must notice it is fixed) or breaks the
#  probe (the suite must notice it can no longer tell).
#
#  Restores are `git checkout --`, never `cp -p` and never `mv`: authoritative
#  about content, and a fresh mtime. The file set is compared against HEAD after
#  every mutate AND every restore.
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail
cd "$(dirname "$0")/.." || exit 2

TOML="files/desktop/apex-greet/greetd-config.toml"
SWAYC="files/desktop/apex-greet/sway-greet.conf"
AUTO="files/desktop/apex-greet/labwc-greet/autostart"
SUITE_F="tests/test-apex-greet-session-bus.sh"
FILES="$TOML $SWAYC $AUTO $SUITE_F"
SUITE="./tests/test-apex-greet-session-bus.sh"

applied=0; noapply=0; caught=0; survived=0

tree_clean() { [ -z "$(git diff --name-only -- $FILES 2>/dev/null)" ]; }

restore() {
    git checkout -- $FILES 2>/dev/null
    if ! tree_clean; then
        echo "ABORT: tree still dirty after restore; verdicts would be meaningless" >&2
        git diff --stat -- $FILES >&2
        exit 3
    fi
}

run_suite() { "$SUITE" 2>&1; }

# mutate <id> <file> <from> <to> <assertion substring that must go red>
mutate() {
    local id="$1" file="$2" from="$3" to="$4" want="$5"

    tree_clean || { echo "ABORT: tree dirty BEFORE $id" >&2; exit 3; }
    if ! grep -qF -- "$from" "$file"; then
        printf '%-5s NO-APPLY  anchor absent in %s — this mutant proves nothing\n' "$id" "$file"
        noapply=$((noapply + 1)); return
    fi
    python3 - "$file" "$from" "$to" <<'EDIT'
import sys
p, a, b = sys.argv[1], sys.argv[2], sys.argv[3]
s = open(p).read()
assert s.count(a) >= 1
open(p, 'w').write(s.replace(a, b, 1))
EDIT
    if tree_clean; then
        printf '%-5s NO-APPLY  the edit changed nothing\n' "$id"
        noapply=$((noapply + 1)); restore; return
    fi
    applied=$((applied + 1))

    local out; out="$(run_suite)"
    if printf '%s' "$out" | grep -q "^FAIL  .*$want"; then
        printf '%-5s CAUGHT    %s\n' "$id" "$want"
        caught=$((caught + 1))
    else
        printf '%-5s SURVIVED  %s\n' "$id" "$want"
        printf '      ── what the suite said instead ──\n'
        printf '%s\n' "$out" | grep -E '^(FAIL|SKIP|apex-greet-session-bus)' | sed 's/^/      /'
        survived=$((survived + 1))
    fi
    restore
}

echo "── baseline: green, or nothing below means anything ──"
base="$(run_suite)"
printf '%s\n' "$base" | grep -E '^apex-greet-session-bus'
if ! printf '%s' "$base" | grep -qE '^apex-greet-session-bus: [0-9]+ passed, 0 failed'; then
    echo "ABORT: the suite is not green to begin with" >&2
    printf '%s\n' "$base" | grep -E '^(FAIL|SKIP)' >&2
    exit 3
fi

echo
echo "── the mutants ──"

# C1 — somebody fixes the session bus. This is the whole point of the suite:
#      when the hole is closed the suite must say so instead of passing on.
mutate C1 "$TOML" \
    'command = "sway --unsupported-gpu' \
    'command = "dbus-run-session -- sway --unsupported-gpu' \
    "the greeter's client can reach no session bus at all"

# C2 — the live command becomes the abandoned cage host, which is one of the two
#      lines already sitting in this file as comments. A grep cannot tell a live
#      line from a commented one; tomllib can.
mutate C2 "$TOML" \
    'command = "sway --unsupported-gpu -c /usr/share/apex-greet/sway-greet.conf"' \
    'command = "cage -ds -- qs -p /usr/share/apex-greet/shell.qml"' \
    "the live command is not one of the two hosts kept in comments"

# C3 — the host stops launching the greeter's client. Every assertion about what
#      that client can see is then vacuously true, so the floor assertion is the
#      only thing between this and a clean green run.
mutate C3 "$SWAYC" \
    'exec "qs -p /usr/share/apex-greet/shell.qml; swaymsg exit"' \
    'exec "swaymsg exit"' \
    "the shipped chain really reaches the greeter's own client"

# C4 — the fallback host is fixed and the primary one is not. A suite that only
#      looked at sway would call the whole thing done.
mutate C4 "$AUTO" \
    'qs -p /usr/share/apex-greet/shell.qml' \
    'dbus-run-session -- qs -p /usr/share/apex-greet/shell.qml' \
    "the labwc fallback's client can reach no session bus either"

# C5 — the vacuity floor, and the most important mutant here. Break the probe so
#      it reports "no bus" whatever it is given, and every absence assertion in
#      the suite still passes. Only the sensitivity check can see it.
mutate C5 "$SUITE_F" \
    '        printf '"'"'session_bus=yes\n'"'"'' \
    '        printf '"'"'session_bus=no\n'"'"'' \
    "the same probe, given a session bus, finds one"

# C6 — somebody fixes the accessibility bus too: the host execs the launcher, as
#      tests/lib/atspi.sh does. The a11y half of the finding must then flip.
LAUNCHER=""
for c in /usr/libexec/at-spi-bus-launcher /usr/lib/at-spi2-core/at-spi-bus-launcher \
         /usr/libexec/at-spi2-core/at-spi-bus-launcher; do
    [ -x "$c" ] && { LAUNCHER="$c"; break; }
done
if [ -n "$LAUNCHER" ]; then
    mutate C6 "$SWAYC" \
        'exec "qs -p /usr/share/apex-greet/shell.qml; swaymsg exit"' \
        "exec \"$LAUNCHER --launch-immediately & sleep 2\"
exec \"qs -p /usr/share/apex-greet/shell.qml; swaymsg exit\"" \
        "a session bus alone is not enough"
else
    printf '%-5s NO-APPLY  at-spi-bus-launcher not found; the a11y half cannot be mutated here\n' C6
    noapply=$((noapply + 1))
fi

echo
printf 'mutants applied=%d, failed-to-apply=%d, caught=%d, SURVIVED=%d\n' \
    "$applied" "$noapply" "$caught" "$survived"
tree_clean || { echo "ABORT: tree dirty at end of run" >&2; exit 3; }
echo "the tree matches HEAD"
[ "$survived" -eq 0 ] && [ "$noapply" -eq 0 ]
