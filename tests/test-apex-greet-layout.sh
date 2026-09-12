#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  test-apex-greet-layout.sh — the greeter's keyboard-layout readout, proved by
#  running the greeter's own extraction against sway's own JSON shape
#  (roadmap P2-004, "keyboard layout before password").
#
#  ── The defect this closes ──────────────────────────────────────────────────
#
#  Three files in this repository already carry the same note — the keymap
#  generator (files/system/systemd/apex-keymap-env), sway-greet.conf and
#  labwc-greet/environment: on a QWERTZ or AZERTY machine a password typed on a
#  US map does not match, and a user who has just installed APEX cannot log in
#  to it. All three fixed which layout the greeter STARTS on.
#
#  None of them tells the user what that layout IS. So a wrong one still
#  presents as an ordinary "wrong password", with nothing on the login screen
#  to diagnose it and no way to change it. GreetContext.qml now reads the live
#  layout out of sway and GreetSurface.qml shows it above the password field;
#  this is the half of that which can be measured without a running sway.
#
#  ── Why it extracts rather than restates ────────────────────────────────────
#
#  A test that re-implements the extraction proves only that the test author
#  and the greeter author agree today. tests/test-apex-greet-sessions.sh
#  established the alternative for this same file: lift the `sh -c` script out
#  of the shipped QML and RUN it, with only the machine it interrogates
#  stubbed. That is what happens here — the script under test is the string
#  GreetContext.qml actually hands to sh, reconstructed from the QML source,
#  and a stub `swaymsg` on a private PATH plays sway.
#
#  The extraction is checked for plausibility before it is used. An extraction
#  that silently returned an empty string would make every assertion below pass
#  for no reason, so a short or shapeless extraction is a FAILURE, never a skip.
#
#  ── What it will not do ─────────────────────────────────────────────────────
#
#  Nothing here starts sway, a compositor, a greeter or a session. It opens no
#  window and asks for no password. The only binary stubbed is swaymsg, on a
#  PATH private to this run.
#
#  Run from anywhere: ./tests/test-apex-greet-layout.sh
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail
# Deliberately +e, like the other suites in this directory: CI invokes a suite
# as `bash -e {0}`, and under -e an assignment from a failing command ends the
# run silently, mid-section.
set +e

cd "$(dirname "$0")" || exit 2
ROOT="$(cd .. && pwd)"
CTX="$ROOT/files/desktop/apex-greet/GreetContext.qml"
SURFACE="$ROOT/files/desktop/apex-greet/GreetSurface.qml"
for f in "$CTX" "$SURFACE"; do
    [ -f "$f" ] || { echo "FATAL: cannot find $f" >&2; exit 2; }
done

pass=0; fail=0; skip=0
ok()  { printf 'PASS  %s\n' "$1"; pass=$((pass + 1)); }
bad() { printf 'FAIL  %s%s\n' "$1" "${2:+  — $2}"; fail=$((fail + 1)); }
skp() { printf 'SKIP  %s%s\n' "$1" "${2:+  — $2}"; skip=$((skip + 1)); }
section() { printf '\n── %s ──\n' "$1"; }
is() {
    local name=$1 want=$2 got=$3
    if [ "$got" = "$want" ]; then ok "$name"
    else bad "$name" "want [$want] got [$got]"; fi
}
finish() {
    printf '\napex-greet-layout: %d passed, %d failed, %d skipped\n' "$pass" "$fail" "$skip"
    [ "$fail" -eq 0 ]
}

WORK="$(mktemp -d "${TMPDIR:-/tmp}/apex-greet-layout.XXXXXX")" || exit 2
cleanup() { rm -rf "$WORK"; }
trap cleanup EXIT INT TERM

# ── The greeter still parses ─────────────────────────────────────────────────
# This is the cheapest assertion in the file and the one with the worst failure
# mode behind it. GreetSurface.qml is built by tests/test-apex-greet-a11y.sh, so
# a syntax error there is caught. GreetContext.qml imports Quickshell, which no
# test can load, so NOTHING parsed this file until this section existed — and it
# is the greeter: a stray brace in it means the next image boots to a login
# screen that never paints, on every machine, with no way in.
#
# qmllint is a parser, not a linter, for this purpose: warnings are ignored (the
# shipped file already emits some, e.g. Process.onExited's QProcess::ExitStatus
# parameter type) and only the exit status is read.
section "the greeter parses"
linter=""
for c in qmllint-qt6 qmllint /usr/lib64/qt6/bin/qmllint /usr/lib/qt6/bin/qmllint; do
    if command -v "$c" >/dev/null 2>&1; then linter="$c"; break; fi
done
if [ -z "$linter" ]; then
    skp "GreetContext.qml and GreetSurface.qml parse" "qmllint not installed"
else
    # Asked, not hardcoded: Fedora puts the QML modules under
    # /usr/lib64/qt6/qml and Debian under
    # /usr/lib/x86_64-linux-gnu/qt6/qml, and a wrong -I turns the Quickshell
    # imports into "not found" noise on one of the two distributions.
    qmldir_inc=""
    for q in qtpaths6 qtpaths /usr/lib64/qt6/bin/qtpaths6 /usr/lib/qt6/bin/qtpaths6; do
        if command -v "$q" >/dev/null 2>&1; then
            d="$("$q" --query QT_INSTALL_QML 2>/dev/null)"
            [ -n "$d" ] && [ -d "$d" ] && qmldir_inc="-I $d"
            break
        fi
    done
    for f in "$CTX" "$SURFACE"; do
        # shellcheck disable=SC2086  # qmldir_inc is a flag pair or empty
        if "$linter" $qmldir_inc "$f" >/dev/null 2>&1; then
            ok "$(basename "$f") parses"
        else
            bad "$(basename "$f") parses" "qmllint rejected it — the greeter would not start"
            # shellcheck disable=SC2086
            "$linter" $qmldir_inc "$f" 2>&1 | grep -i "error" | head -5
        fi
    done
fi

# ── Extraction ───────────────────────────────────────────────────────────────
# The `readonly property string layoutScript:` value is a QML string built by
# concatenating double-quoted segments. QML's escapes are JSON's, so each
# segment is decoded as a JSON string and the pieces joined.
extract_script() {   # extract_script <GreetContext.qml>
    python3 - "$1" <<'PY'
import json, re, sys
src = open(sys.argv[1]).read()
m = re.search(r'readonly property string layoutScript:\s*(.*?)\n\s*\n', src, re.S)
if not m:
    sys.exit(1)
body = m.group(1)
parts = re.findall(r'"((?:[^"\\]|\\.)*)"', body)
if not parts:
    sys.exit(1)
sys.stdout.write("".join(json.loads('"' + p + '"') for p in parts))
PY
}

SCRIPT="$WORK/layout.sh"
extract_script "$CTX" > "$SCRIPT" 2>/dev/null
xstatus=$?

section "the extraction is real"
if [ "$xstatus" -ne 0 ] || [ ! -s "$SCRIPT" ]; then
    bad "layoutScript extracted from GreetContext.qml" "extraction produced nothing"
    finish; exit 1
fi
ok "layoutScript extracted from GreetContext.qml"

n_chars=$(wc -c < "$SCRIPT")
if [ "$n_chars" -ge 200 ]; then
    ok "the extracted script is a whole script ($n_chars chars)"
else
    bad "the extracted script is a whole script" "only $n_chars chars; the regex lost most of it"
    finish; exit 1
fi

for token in swaymsg xkb_active_layout_name xkb_layout_names; do
    if grep -q "$token" "$SCRIPT"; then
        ok "the extracted script mentions $token"
    else
        bad "the extracted script mentions $token" "extraction produced the wrong string"
        finish; exit 1
    fi
done

# ── The stub sway ────────────────────────────────────────────────────────────
mkdir -p "$WORK/bin"
cat > "$WORK/bin/swaymsg" <<'STUB'
#!/usr/bin/env bash
# Plays sway's `-t get_inputs -r`. The payload is whatever the test put in
# $SWAY_FIXTURE; an absent fixture means "sway answered nothing", which is the
# labwc-host case.
if [ -n "${SWAY_FIXTURE:-}" ] && [ -f "$SWAY_FIXTURE" ]; then
    cat "$SWAY_FIXTURE"
    exit 0
fi
exit 1
STUB
chmod +x "$WORK/bin/swaymsg"

run_script() {   # run_script <fixture-file-or-empty>
    SWAY_FIXTURE="$1" PATH="$WORK/bin:$PATH" sh "$SCRIPT" 2>/dev/null
}

# sway's own shape. Real `swaymsg -t get_inputs` returns an array of input
# devices; only keyboards carry the xkb fields, and a machine has several
# non-keyboard devices before them.
mk_fixture() {   # mk_fixture <file> <active> <names-json>
    cat > "$1" <<EOF
[
  {
    "identifier": "1739:0:Synaptics_TouchPad",
    "name": "Synaptics TouchPad",
    "type": "touchpad"
  },
  {
    "identifier": "1:1:AT_Translated_Set_2_keyboard",
    "name": "AT Translated Set 2 keyboard",
    "type": "keyboard",
    "xkb_active_layout_name": "$2",
    "xkb_layout_names": $3
  }
]
EOF
}

section "one layout"
mk_fixture "$WORK/one.json" "English (US)" '["English (US)"]'
got="$(run_script "$WORK/one.json")"
is "a single-layout machine reports that layout, active and configured" \
   "$(printf 'English (US)\tEnglish (US)')" "$got"

section "two layouts — the case a comma-split loses"
# This is the assertion that matters. sway renders the list as
# ["English (US)", "French"], and any extraction that splits the JSON on commas
# keeps only the first entry — which reads as a single-layout machine and hides
# the switch on exactly the machines that need it.
mk_fixture "$WORK/two.json" "French" '["English (US)", "French"]'
got2="$(run_script "$WORK/two.json")"
is "both configured layouts survive extraction" \
   "$(printf 'French\tEnglish (US),French')" "$got2"

section "three layouts, and a layout name containing a comma-free space"
mk_fixture "$WORK/three.json" "German" '["English (US)", "German", "Arabic"]'
got3="$(run_script "$WORK/three.json")"
is "a three-layout machine reports all three in order" \
   "$(printf 'German\tEnglish (US),German,Arabic')" "$got3"

section "the host that has no sway IPC"
# Under the documented labwc fallback host there is no swaymsg to answer. The
# script must print NOTHING and exit cleanly, so GreetContext's environment
# fallback is what supplies the readout rather than an empty indicator.
got4="$(run_script "")"
is "no sway IPC produces no line at all" "" "$got4"
run_script "" >/dev/null 2>&1
is "…and exits 0, so the fallback Process is what answers" "0" "$?"

section "a keyboard with no xkb fields"
cat > "$WORK/nokb.json" <<'EOF'
[ { "identifier": "1739:0:pad", "name": "pad", "type": "touchpad" } ]
EOF
got5="$(run_script "$WORK/nokb.json")"
is "a machine sway reports no keyboard for produces no line" "" "$got5"

# ── The QML side: _setLayouts ────────────────────────────────────────────────
section "_setLayouts turns that line into the model"
if ! command -v node >/dev/null 2>&1; then
    skp "_setLayouts parses the extracted line" "node is not installed"
else
    SET_JS="$WORK/setlayouts.js"
    python3 - "$CTX" "$SET_JS" <<'PY'
import re, sys
src = open(sys.argv[1]).read()
m = re.search(r'function _setLayouts\(line\) \{(.*?)\n    \}', src, re.S)
if not m:
    sys.exit(1)
body = m.group(1)
open(sys.argv[2], "w").write("""
var ctx = { layouts: [], layoutName: "us" };
function _setLayouts(line) {%s
}
_setLayouts(process.argv[2]);
console.log(JSON.stringify([ctx.layoutName, ctx.layouts]));
""" % body)
PY
    if [ ! -s "$SET_JS" ]; then
        bad "_setLayouts extracted from GreetContext.qml" "extraction produced nothing"
    else
        ok "_setLayouts extracted from GreetContext.qml"

        r1="$(node "$SET_JS" "$(printf 'French\tEnglish (US),French')" 2>/dev/null)"
        is "two layouts become a two-element model with the active one selected" \
           '["French",["English (US)","French"]]' "$r1"

        r2="$(node "$SET_JS" "$(printf 'English (US)\tEnglish (US)')" 2>/dev/null)"
        is "one layout becomes a one-element model" \
           '["English (US)",["English (US)"]]' "$r2"

        # An empty active field must not blank the readout — the indicator would
        # then show nothing at all, which is worse than showing the first layout.
        r3="$(node "$SET_JS" "$(printf '\tEnglish (US),French')" 2>/dev/null)"
        is "a missing active name falls back to the first configured layout" \
           '["English (US)",["English (US)","French"]]' "$r3"

        # A malformed line must leave the previous model alone rather than
        # replacing it with an empty one.
        r4="$(node "$SET_JS" "garbage-with-no-tab" 2>/dev/null)"
        is "a line with no tab changes nothing" '["us",[]]' "$r4"
    fi
fi

# ── The surface honours the model ────────────────────────────────────────────
section "the surface's switch is gated on there being something to switch to"
# GreetSurface hides the switch affordance unless more than one layout is
# configured. Asserted here as well as in greet-a11y-test.qml because this is
# the file that knows what the model looks like: a `switchable` that read
# `layouts.length > 0` would satisfy the QML suite's two-layout case and still
# offer a dead control on every single-layout machine.
# NOTE: the surface's own gate is NOT asserted here by grepping for
# `layouts.length > 1`. greet-a11y-test.qml's test_034 presses Space on the
# focused indicator with one layout configured and requires that nothing
# happens, which is the same claim measured instead of matched.
# NOT a grep. Replacing cycleLayout's guard with `if (false) return` was a
# mutation this suite SURVIVED while this was two `grep -q` calls: the stanza
# still CONTAINED both `canSwitchLayout` and `ctx.layouts.length > 1` — the
# first in the readonly property that defines it, the second in that property's
# own expression — so the grep stayed green with the guard gone. That is the
# failure mode where an assertion greps a block that also names the thing it is
# looking for. So the function is extracted and RUN, with the shipped
# canSwitchLayout expression supplying the predicate.
if ! command -v node >/dev/null 2>&1; then
    skp "GreetContext refuses to cycle a single-layout keyboard" "node is not installed"
else
    CYC_JS="$WORK/cyclelayout.js"
    python3 - "$CTX" "$CYC_JS" <<'PY'
import re, sys
src = open(sys.argv[1]).read()
fn = re.search(r'function cycleLayout\(dir\) \{(.*?)\n    \}', src, re.S)
pred = re.search(r'readonly property bool canSwitchLayout:\s*(.+)', src)
if not fn or not pred:
    sys.exit(1)
open(sys.argv[2], "w").write("""
var layoutSwitchProc = { command: null, running: false };
var ctx = {
  layouts: JSON.parse(process.argv[2]),
  get canSwitchLayout() { return %s; }
};
function cycleLayout(dir) {%s
}
cycleLayout(parseInt(process.argv[3], 10));
console.log(JSON.stringify([layoutSwitchProc.running,
                            (layoutSwitchProc.command || [])[2] || ""]));
""" % (pred.group(1).strip(), fn.group(1)))
PY
    if [ ! -s "$CYC_JS" ]; then
        bad "cycleLayout extracted from GreetContext.qml" "extraction produced nothing"
    else
        ok "cycleLayout extracted from GreetContext.qml"

        c1="$(node "$CYC_JS" '["English (US)"]' 1 2>/dev/null)"
        is "one configured layout: cycleLayout runs nothing at all" \
           '[false,""]' "$c1"

        c2="$(node "$CYC_JS" '["English (US)","French"]' 1 2>/dev/null | \
              sed -E 's/.*(xkb_switch_layout [a-z]+).*/\1/')"
        is "two layouts, forwards: sway is asked for the next layout" \
           "xkb_switch_layout next" "$c2"

        c3="$(node "$CYC_JS" '["English (US)","French"]' -1 2>/dev/null | \
              sed -E 's/.*(xkb_switch_layout [a-z]+).*/\1/')"
        is "two layouts, backwards: sway is asked for the previous layout" \
           "xkb_switch_layout prev" "$c3"

        c4="$(node "$CYC_JS" '[]' 1 2>/dev/null)"
        is "no layouts known yet: cycleLayout runs nothing" '[false,""]' "$c4"
    fi
fi

# ── Self-test ────────────────────────────────────────────────────────────────
# Two mutants, each changing ONE arm, each of which must be caught. The repo
# idiom (check-scale-tokens.sh, check-agent-help.sh): apply the mutation, VERIFY
# THE FILE ACTUALLY CHANGED before believing the verdict — a mutant that failed
# to apply must be reported as such, never as caught, because this tree has
# produced exactly that false verdict before — then re-run the rule.
#
# Both of these SURVIVED when they were first written, and both survivals were
# real defects in the assertions rather than bad mutants. SM2 in particular is
# why the cycleLayout check above runs the function instead of grepping for it.
section "self-test"

mutate_ctx() {   # mutate_ctx <out.qml> <python-expr-over-s>
    python3 - "$CTX" "$1" "$2" <<'PY'
import sys
src, out, expr = sys.argv[1], sys.argv[2], sys.argv[3]
s = open(src).read()
before = s
s = eval(expr, {"s": s, "chr": chr})
if s == before:
    sys.exit(3)
open(out, "w").write(s)
PY
}

# SM1 — the extraction splits sway's JSON array on commas, keeping only the
# first layout. A two-layout machine then reads as single-layout and the switch
# is hidden on exactly the machines that need it.
if mutate_ctx "$WORK/m1.qml" 's.replace(chr(92)+chr(92)+"[[^]]*"+chr(92)+chr(92)+"]", chr(92)+chr(92)+"[[^],]*", 1)'; then
    extract_script "$WORK/m1.qml" > "$WORK/m1.sh" 2>/dev/null
    m1_out="$(SWAY_FIXTURE="$WORK/two.json" PATH="$WORK/bin:$PATH" sh "$WORK/m1.sh" 2>/dev/null)"
    if [ "$m1_out" != "$(printf 'French\tEnglish (US),French')" ]; then
        ok "self-test SM1 (comma-split extraction): caught"
    else
        bad "self-test SM1 (comma-split extraction): SURVIVED" \
            "a mutated extraction still produced both layouts"
    fi
else
    bad "self-test SM1 (comma-split extraction): MUTANT DID NOT APPLY" \
        "the extraction no longer contains the bracket pattern this mutant edits"
fi

# SM2 — cycleLayout loses its single-layout guard, so Space on a one-layout
# machine asks sway to switch to a layout that does not exist.
if ! command -v node >/dev/null 2>&1; then
    skp "self-test SM2 (cycleLayout guard removed)" "node is not installed"
elif mutate_ctx "$WORK/m2.qml" 's.replace("if (!ctx.canSwitchLayout) return", "if (false) return", 1)'; then
    M2_JS="$WORK/m2.js"
    python3 - "$WORK/m2.qml" "$M2_JS" <<'PY'
import re, sys
src = open(sys.argv[1]).read()
fn = re.search(r'function cycleLayout\(dir\) \{(.*?)\n    \}', src, re.S)
pred = re.search(r'readonly property bool canSwitchLayout:\s*(.+)', src)
if not fn or not pred:
    sys.exit(1)
open(sys.argv[2], "w").write("""
var layoutSwitchProc = { command: null, running: false };
var ctx = { layouts: JSON.parse(process.argv[2]),
            get canSwitchLayout() { return %s; } };
function cycleLayout(dir) {%s
}
cycleLayout(1);
console.log(JSON.stringify(layoutSwitchProc.running));
""" % (pred.group(1).strip(), fn.group(1)))
PY
    m2_out="$(node "$M2_JS" '["English (US)"]' 2>/dev/null)"
    if [ "$m2_out" = "true" ]; then
        ok "self-test SM2 (cycleLayout guard removed): caught"
    else
        bad "self-test SM2 (cycleLayout guard removed): SURVIVED" \
            "a guardless cycleLayout still refused to run; got [$m2_out]"
    fi
else
    bad "self-test SM2 (cycleLayout guard removed): MUTANT DID NOT APPLY" \
        "cycleLayout no longer contains the guard this mutant edits"
fi

finish
