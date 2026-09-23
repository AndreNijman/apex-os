#!/usr/bin/env bash
# The login screen takes the owner's matugen accent from
# /var/lib/apex-greet/accents/<user>. It reads it with
# `head -c 16 <file>; echo`, and the file ends in a newline, so the greeter
# receives TWO lines: the colour, then an empty one. When the handler set the
# accent per line, the empty line reset it to "" and the login screen stayed on
# the edition green with a correct, readable accent file (L16, 2026-09-23).
#
# This replays that exact output through the handler rule the QML uses.
set -u

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CTX="$ROOT/files/desktop/apex-greet/GreetContext.qml"
pass=0 fail=0
ok()  { printf 'PASS  %s\n' "$1"; pass=$((pass + 1)); }
bad() { printf 'FAIL  %s\n' "$1"; fail=$((fail + 1)); }

block="$(sed -n '/id: accentProc/,/^    }$/p' "$CTX")"
[ -n "$block" ] && ok "the accent reader is found in GreetContext.qml" \
                || { bad "the accent reader is found in GreetContext.qml"; exit 1; }

# The handler must not assign the accent per line; the exit handler applies it.
onread="$(printf '%s\n' "$block" | sed -n '/onRead:/,/^            }$/p')"
printf '%s' "$onread" | grep -q 'ctx.accent' \
    && bad "a line does not set the accent on its own (an empty line would reset it)" \
    || ok "a line does not set the accent on its own (an empty line would reset it)"
printf '%s\n' "$block" | grep -qE 'onExited:' && printf '%s\n' "$block" | grep -qE 'ctx\.accent *= *accentProc\._seen' \
    && ok "the accent is applied once the read has finished" \
    || bad "the accent is applied once the read has finished"

# The exact shape the reader produces: the file's own newline, then echo's.
if command -v node >/dev/null 2>&1; then
    got="$(printf '#fab898\n\n' | node -e '
        const lines = require("fs").readFileSync(0, "utf8").split("\n");
        if (lines[lines.length - 1] === "") lines.pop();
        let seen = "";
        for (const l of lines) { const c = l.trim(); if (/^#[0-9a-fA-F]{6}$/.test(c)) seen = c; }
        process.stdout.write(seen);')"
    [ "$got" = "#fab898" ] && ok "\"#fab898\\n\\n\" yields #fab898, not the edition fallback" \
                           || bad "\"#fab898\\n\\n\" yields #fab898, not the edition fallback (got '$got')"
    none="$(printf '\n' | node -e '
        const lines = require("fs").readFileSync(0, "utf8").split("\n"); lines.pop();
        let seen = ""; for (const l of lines) { const c = l.trim(); if (/^#[0-9a-fA-F]{6}$/.test(c)) seen = c; }
        process.stdout.write(seen);')"
    [ -z "$none" ] && ok "no accent file yields no accent, so the edition colour is used" \
                   || bad "no accent file yields no accent, so the edition colour is used"
else
    printf 'SKIP  the output replay (node not installed)\n'
fi

printf '\napex greet-accent: %d passed, %d failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ]
