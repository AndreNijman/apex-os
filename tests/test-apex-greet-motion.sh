#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  test-apex-greet-motion.sh — the login screen's password shapes, and the
#  motion settings they move on.
#
#  The greeter draws its password feedback with APEX Shell's PasswordShapes
#  (loaded from /usr/share/apex-shell), one geometric shape per character, on
#  the last user's own motion settings — Reduce Motion above all. Those arrive
#  the way the accent does: /usr/libexec/apex-greet-wallpaper, a NOPASSWD root
#  helper, reads the user's settings.json AS THE USER and publishes three
#  validated lines to /var/lib/apex-greet/motion/<user>.
#
#  What must hold:
#   §1 the helper's parser (sourced and run for real) emits only its closed
#      vocabulary, whatever the file says — hostile input becomes defaults;
#   §2 the greeter's reader validates each line again and applies them once the
#      read has finished (the accent reader's trailing-empty-line lesson);
#   §3 the greeter hands the shapes the field's LENGTH and nothing else of it,
#      keeps the field a masked password field, and falls back to plain dots if
#      the component cannot be loaded;
#   §4 a refused attempt's error survives the field being cleared.
# ─────────────────────────────────────────────────────────────────────────────
set -u
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
HELPER="$ROOT/files/system/libexec/apex-greet-wallpaper"
CTX="$ROOT/files/desktop/apex-greet/GreetContext.qml"
SURF="$ROOT/files/desktop/apex-greet/GreetSurface.qml"
pass=0 fail=0
ok()  { printf 'PASS  %s\n' "$1"; pass=$((pass + 1)); }
bad() { printf 'FAIL  %s\n' "$1"; fail=$((fail + 1)); }
section() { printf '\n── %s ──\n' "$1"; }

# ── §1 the helper's parser ───────────────────────────────────────────────────
section "§1 the publisher emits only its own vocabulary"
prefs() {   # prefs <settings.json content> — the REAL function, in bash
    printf '%s' "$1" | bash -c '. "$1"; motion_prefs' _ "$HELPER"
}
want() {    # want <label> <input> <expected output>
    local got; got="$(prefs "$2")"
    if [ "$got" = "$(printf '%b' "$3")" ]; then ok "$1"
    else bad "$1 — got [$(printf '%s' "$got" | tr '\n' ' ')]"; fi
}
want "the settings a user set are carried" \
    '{"reduceMotion":true,"motionSpeed":"relaxed","motionScale":1.5}' 'reduce=1\nspeed=relaxed\nscale=1.5'
want "defaults when there is no file at all" '' 'reduce=0\nspeed=balanced\nscale=1'
want "a pre-motion file migrates animDuration by its ratio to 320" \
    '{"animDuration":480,"reduceMotion":false}' 'reduce=0\nspeed=balanced\nscale=1.50'
want "motionScale wins over a stale animDuration" \
    '{"animDuration":960,"motionScale":0.8}' 'reduce=0\nspeed=balanced\nscale=0.8'
want "an unknown speed is the default, not a string passed through" \
    '{"motionSpeed":"warp"}' 'reduce=0\nspeed=balanced\nscale=1'
want "a newline smuggled into a string cannot add a line" \
    '{"motionSpeed":"balanced\nreduce=1","reduceMotion":false}' 'reduce=0\nspeed=balanced\nscale=1'
want "an out-of-range scale is the default" '{"motionScale":99}' 'reduce=0\nspeed=balanced\nscale=1'
want "a scale that is not a number is the default" '{"motionScale":"1.5; rm -rf /"}' 'reduce=0\nspeed=balanced\nscale=1'
want "reduceMotion must be a JSON boolean" '{"reduceMotion":"true"}' 'reduce=0\nspeed=balanced\nscale=1'
lines="$(prefs "$(head -c 200000 /dev/urandom | base64 -w0)")"
[ "$(printf '%s\n' "$lines" | grep -cvE '^(reduce=[01]|speed=(snappy|balanced|relaxed)|scale=[0-9]{1,2}(\.[0-9]{1,4})?)$')" = 0 ] \
    && [ "$(printf '%s\n' "$lines" | grep -c .)" = 3 ] \
    && ok "200 KB of noise still yields exactly three valid lines" \
    || bad "noise produced: $lines"
grep -qE '^\s*\[ "\$\{BASH_SOURCE\[0\]\}" = "\$0" \] \|\| return 0' "$HELPER" \
    && ok "sourcing the helper runs nothing past the parser" \
    || bad "the helper has no source guard — sourcing it for a test would run as root"
awk '/motion_prefs\(\) \{/,/^}/' "$HELPER" | grep -q 'setpriv\|mv \|chmod\|rm ' \
    && bad "the parser itself does privileged work" \
    || ok "the parser only parses; the privileged write is outside it"
grep -qE 'setpriv --reuid="\$\{uid\}" --regid="\$\{gid\}" --clear-groups -- \\' "$HELPER" \
    && grep -q 'head -c 16384 -- "${settings}"' "$HELPER" \
    && ok "settings.json is read as the caller, capped, like the accent" \
    || bad "settings.json is not read with the caller's privileges"

# ── §2 the greeter's reader ──────────────────────────────────────────────────
section "§2 the greeter validates what it reads"
block="$(sed -n '/id: motionProc/,/^    }$/p' "$CTX")"
[ -n "$block" ] && ok "the motion reader is found in GreetContext.qml" || bad "no motion reader in GreetContext.qml"
printf '%s\n' "$block" | grep -q 'head -c 256 ' && ok "the published file is read capped" || bad "the motion file is read uncapped"
printf '%s\n' "$block" | grep -qF '(snappy|balanced|relaxed)' \
    && printf '%s\n' "$block" | grep -qF 'v === "0" || v === "1"' \
    && printf '%s\n' "$block" | grep -qE 'parseFloat\(v\) <= 2\.5' \
    && ok "every line is validated against its vocabulary again" \
    || bad "a published line reaches a binding unvalidated"
onread="$(printf '%s\n' "$block" | sed -n '/onRead:/,/^            }$/p')"
printf '%s' "$onread" | grep -qE 'ctx\.motion' \
    && bad "a line sets ctx.motion* on its own (a trailing empty line would reset it)" \
    || ok "no single line sets the motion properties; the exit handler applies them"
printf '%s\n' "$block" | grep -qE 'ctx\.motionReduced *= *s\.reduce === true' \
    && ok "Reduce Motion is applied once the read has finished" || bad "Reduce Motion is not applied on exit"

# ── §3 the shapes ────────────────────────────────────────────────────────────
section "§3 the shapes see the length and nothing else"
lo="$(sed -n '/^            Loader {$/,/^            }$/p' "$SURF")"
[ -n "$lo" ] && ok "the shapes Loader is found in GreetSurface.qml" || bad "no shapes Loader in GreetSurface.qml"
uses="$(printf '%s\n' "$lo" | grep -oE 'passwordInput\.[A-Za-z]+' | sort -u | tr '\n' ' ')"
[ "$uses" = "passwordInput.length " ] && ok "the only thing of the field it is handed is passwordInput.length" \
    || bad "the shapes are handed: $uses"
printf '%s\n' "$lo" | grep -q 'source: root.ctx.shapesSource' && ok "it loads APEX Shell's component, not a copy" \
    || bad "the shapes source is not the shared component"
grep -q '/src/components/auth/PasswordShapes.qml' "$CTX" && grep -q '/usr/share/apex-shell' "$CTX" \
    && ok "from the APEX Shell tree the image vendors to /usr/share/apex-shell" || bad "the shapes path is wrong"
pw="$(sed -n '/id: passwordInput/,/^            }$/p' "$SURF")"
printf '%s\n' "$pw" | grep -qE 'echoMode: +TextInput\.Password' \
    && printf '%s\n' "$pw" | grep -qE 'passwordMaskDelay: +0$' \
    && printf '%s\n' "$pw" | grep -qE 'Accessible\.passwordEdit: +true' \
    && ok "the field is still echoMode Password, mask delay 0, passwordEdit" \
    || bad "the password field lost its masking"
printf '%s\n' "$pw" | grep -qE 'color: +shapes\.active \? "transparent" : root\.theme\.text' \
    && printf '%s\n' "$pw" | grep -qE 'cursorDelegate: +shapes\.active \? noCaret : null' \
    && ok "without the component the field is the plain dotted field it always was" \
    || bad "the field does not fall back to its own dots when the shapes are missing"

# ── §4 a refusal survives the clear ──────────────────────────────────────────
section "§4 a refused attempt's error survives the field being cleared"
printf '%s\n' "$pw" | grep -qE 'if \(root\.ctx\.hasError && text\.length > 0\) root\.ctx\.hasError = false' \
    && ok "the error clears when the user types, not when the field is emptied" \
    || bad "clearing the field still clears the error the refusal just set"

printf '\napex-greet-motion: %d passed, %d failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ]
