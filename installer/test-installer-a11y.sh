#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  test-installer-a11y.sh — the installer, audited with a screen reader and a
#  keyboard, and nothing else (roadmap P2-003: "screen reader … / keyboard-only
#  installer validated").
#
#  ── The criterion names the installer, and nothing was measuring it ─────────
#
#  P2-003's acceptance line ends "keyboard-only installer validated". Every
#  other suite in this repository answers questions about the installer by
#  reading its source or by driving it through Python — clicking buttons by
#  calling `emit("clicked")`. Neither can see the two things that decide whether
#  a blind user or a user with no mouse can install APEX at all:
#
#    * whether a control ANNOUNCES itself, and
#    * whether the Tab key can reach it.
#
#  Both are measured here against the shipped GUI: real X key events delivered
#  by xdotool into the real GTK4 toolkit, and the resulting focus read back over
#  AT-SPI on a private accessibility bus — the same protocol Orca speaks.
#
#  ── What the audit found when it was first run ──────────────────────────────
#
#  TEN focusable `text box` nodes on the bus with an empty name AND an empty
#  description: the keyboard test field, the Wi-Fi password and hidden-network
#  name, all four account fields, both Secure Boot enrolment passwords, and the
#  "type ERASE to confirm" field. Every one of them has a visible
#  `placeholder-text`, which is why the defect is invisible to a person looking
#  at the screen and to any grep for a missing label: GTK does not map a
#  placeholder to the accessible name, and a placeholder disappears the moment
#  the user types. A screen reader announced "text box" four times in a row on
#  the page where the account password is set.
#
#  That is the house defect this unit was warned about, in reverse: a sweep for
#  missing labels would have found nothing, because the labels are all there —
#  they are simply not attached to anything. It took asking the bus.
#
#  ── Headless, and cage is deliberately NOT in this loop ─────────────────────
#
#  A private Xvfb on a high display number, a private session bus with an empty
#  service directory, a private a11y bus inside a directory this suite created.
#  Nothing reaches the display or the accessibility bus of whoever is sitting at
#  the machine, and the harness aborts rather than continues if it ever does.
#
#  test-installer-keymap.sh runs the GUI under cage because the thing it
#  measures is what a COMPOSITOR hands a client. This suite measures what the
#  TOOLKIT publishes and how the toolkit routes Tab, which is the same code on
#  either backend — so it runs GTK straight onto X11, where xdotool can deliver
#  real key events. Adding cage here would measure wlroots, not the installer.
#
#  Run from anywhere: ./installer/test-installer-a11y.sh
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail
set +e

cd "$(dirname "$0")" || exit 2
HERE="$(pwd)"
ROOT="$(cd .. && pwd)"
GUI="$HERE/apex-installer-gui"
WALK="$ROOT/tests/atspi-walk.py"

pass=0; fail=0; skip=0
ok()  { printf 'PASS  %s\n' "$1"; pass=$((pass + 1)); }
bad() { printf 'FAIL  %s%s\n' "$1" "${2:+  — $2}"; fail=$((fail + 1)); }
skp() { printf 'SKIP  %s%s\n' "$1" "${2:+  — $2}"; skip=$((skip + 1)); }
section() { printf '\n── %s ──\n' "$1"; }
finish() {
    printf '\ninstaller-a11y: %d passed, %d failed, %d skipped\n' "$pass" "$fail" "$skip"
    [ "$fail" -eq 0 ]
}

for f in "$GUI" "$WALK"; do
    [ -f "$f" ] || { echo "FATAL: cannot find $f" >&2; exit 2; }
done

# ── what this needs, named one by one ───────────────────────────────────────
# A missing tool is a COULD-NOT-RUN with the tool's name in it. Never a pass:
# an accessibility audit that silently measures nothing is worse than no audit,
# because it produces a green line somebody will quote.
missing=""
command -v Xvfb    >/dev/null 2>&1 || missing="$missing Xvfb(xorg-x11-server-Xvfb)"
command -v xdotool >/dev/null 2>&1 || missing="$missing xdotool"
python3 -c 'import gi; gi.require_version("Gtk","4.0"); gi.require_version("Adw","1"); from gi.repository import Gtk, Adw' \
    >/dev/null 2>&1 || missing="$missing python3-gi(Gtk4+Adw)"
if [ -n "$missing" ]; then
    echo "SKIP: not installed here:$missing"
    echo "      This is a COULD-NOT-RUN. No assertion below was evaluated."
    exit 0
fi

# shellcheck source=tests/lib/atspi.sh
. "$ROOT/tests/lib/atspi.sh"
atspi_require || exit 0

XPID=""; GPID=""; DISP=""
cleanup() {
    [ -n "$GPID" ] && kill "$GPID" 2>/dev/null
    [ -n "$XPID" ] && kill "$XPID" 2>/dev/null
    atspi_cleanup
}
trap cleanup EXIT INT TERM

# ── a private X display ─────────────────────────────────────────────────────
# Probed for a free number rather than hardcoded, and high, so it cannot
# collide with the :0 somebody is working on.
for n in 91 92 93 94 95 96 97 98; do
    [ -e "/tmp/.X11-unix/X$n" ] && continue
    DISP=":$n"; break
done
if [ -z "$DISP" ]; then
    echo "SKIP: no free X display number in 91-98. COULD-NOT-RUN."
    exit 0
fi
Xvfb "$DISP" -screen 0 1280x900x24 -nolisten tcp >/dev/null 2>&1 &
XPID=$!
for i in $(seq 1 60); do [ -e "/tmp/.X11-unix/X${DISP#:}" ] && break; sleep 0.25; done
if [ ! -e "/tmp/.X11-unix/X${DISP#:}" ]; then
    echo "SKIP: Xvfb did not come up on $DISP. COULD-NOT-RUN."
    exit 0
fi
export DISPLAY="$DISP"

atspi_start || { echo "SKIP: no private accessibility bus. COULD-NOT-RUN."; exit 0; }

section "the world this runs in is private"
case "$ATSPI_BUS" in
    *"unix:path=$ATSPI_RUNTIME/"*)
        ok "the accessibility bus is inside this suite's own runtime directory" ;;
    *)  bad "the accessibility bus is inside this suite's own runtime directory" "$ATSPI_BUS"
        finish; exit 1 ;;
esac
if [ "$DISPLAY" != ":0" ] && [ -n "$DISP" ]; then
    ok "the X display is this suite's own ($DISP)"
else
    bad "the X display is this suite's own" "refusing to drive $DISPLAY"
    finish; exit 1
fi

# ── the shipped GUI, ONE PAGE PER PROCESS ──────────────────────────────
# Each page gets its own installer process, and the previous one is required to
# have left the registry before the next is judged.
#
# This is not tidiness. The first version of this audit ran the pages in a loop
# and killed each GUI with `kill $!` -- which reaped a SUBSHELL, because
# backgrounding a shell function backgrounds the function, not the program it
# runs. Six installer processes ended up alive at once, every walk saw all of
# their trees merged, and the per-page counts it produced were wrong in the
# direction that hides defects: later pages looked as though they already
# contained the earlier pages' named controls. atspi_run_app now execs, and the
# loop below waits for the registry to go back to zero before trusting a count.
export GDK_BACKEND=x11 GSK_RENDERER=cairo

audit_page() {   # audit_page <page> <sentinel accessible name> <floor>
    local page="$1" sentinel="$2" floor="$3" n i

    for i in $(seq 1 40); do
        n="$(python3 "$WALK" --count 2>/dev/null || echo 0)"
        [ "$n" = "0" ] && break
        sleep 0.3
    done
    if [ "$n" != "0" ]; then
        bad "page '$page': the previous page's process left the registry" \
            "$n still registered; this page's counts would include them"
        return
    fi

    APEX_GUI_PAGE="$page" atspi_run_app python3 "$GUI" \
        >"$ATSPI_W/gui-$page.out" 2>"$ATSPI_W/gui-$page.err" &
    GPID=$!

    # Wait for a NAMED thing this page is about to be asserted on, not for the
    # node count to stop changing. A stability poll was tried first and it caught
    # a pause mid-construction, declaring a 45-node tree settled when the
    # finished page has far more -- the audit then ran over a page that did not
    # exist yet, and would have reported "every control has a name" about almost
    # nothing.
    local built=0
    for i in $(seq 1 100); do
        python3 "$WALK" --dump >"$ATSPI_W/dump-$page.txt" 2>/dev/null
        grep -q "| name=$sentinel |" "$ATSPI_W/dump-$page.txt" && { built=1; break; }
        kill -0 "$GPID" 2>/dev/null || break
        sleep 0.4
    done
    if [ "$built" != "1" ]; then
        bad "page '$page' builds and reaches the accessibility bus" \
            "never saw a node named '$sentinel'"
        grep -v 'libEGL\|DRI3\|Adwaita-WARNING' "$ATSPI_W/gui-$page.err" 2>/dev/null \
            | sed 's/^/      /' | head -8
        kill "$GPID" 2>/dev/null
        return
    fi
    ok "page '$page' builds and reaches the accessibility bus"

    python3 "$WALK" --json >"$ATSPI_W/tree-$page.json" 2>/dev/null
    python3 - "$ATSPI_W/tree-$page.json" >"$ATSPI_W/audit-$page.txt" 2>&1 <<'PYAUDIT'
import json, sys

# Scoped to the roles the installer BUILDS. GTK realises a pile of `generic`
# containers -- a dropdown's popup scaffolding, list-item wrappers -- that are
# focusable in the toolkit's own internal sense and are not controls this
# installer creates. Asserting over those would be asserting about GTK, and red
# for reasons nobody in this repository can fix.
INTERACTIVE = {
    "text box", "entry", "password text", "push button", "button",
    "toggle button", "combo box", "check box", "radio button",
    "slider", "spin button", "link",
}
roots = json.load(open(sys.argv[1]))


def flat(n, out=None):
    out = [] if out is None else out
    out.append(n)
    for c in n["children"]:
        flat(c, out)
    return out


named, unnamed = [], []
for r in roots:
    for n in flat(r):
        if n["role"] in INTERACTIVE and "focusable" in n["states"]:
            (named if (n["name"] or n["description"]) else unnamed).append(n)
print("NAMED=%d" % len(named))
print("UNNAMED=%d" % len(unnamed))
for n in unnamed:
    print("UNNAMED-ROLE %s" % n["role"])
for n in named:
    print("NAMED-CTRL %s|%s" % (n["role"], n["name"] or n["description"]))
PYAUDIT

    local n_named n_unnamed
    n_named="$(grep -m1 '^NAMED=' "$ATSPI_W/audit-$page.txt" | cut -d= -f2)"
    n_unnamed="$(grep -m1 '^UNNAMED=' "$ATSPI_W/audit-$page.txt" | cut -d= -f2)"

    # The floor first, per page. "No unnamed controls" is trivially true of a
    # page with no controls on it, and that is the false green a half-built tree
    # produces.
    if [ "${n_named:-0}" -ge "$floor" ]; then
        ok "page '$page': the audit found controls to audit ($n_named named)"
    else
        bad "page '$page': the audit found controls to audit" \
            "only ${n_named:-0} named interactive controls, expected at least $floor"
    fi

    if [ "${n_unnamed:-1}" -eq 0 ]; then
        ok "page '$page': every focusable control it builds announces itself"
    else
        bad "page '$page': every focusable control it builds announces itself" \
            "$n_unnamed unnamed"
        grep '^UNNAMED-ROLE' "$ATSPI_W/audit-$page.txt" | sed 's/^/      /'
    fi

    cat "$ATSPI_W/audit-$page.txt" >>"$ATSPI_W/audit-all.txt"
}

section "every control the installer builds announces itself, page by page"
: >"$ATSPI_W/audit-all.txt"

# Sentinel + floor per page. The floors are what each page actually builds, so a
# page that silently loses a control fails here rather than quietly shrinking.
audit_page welcome    "Begin"                          2
kill "$GPID" 2>/dev/null
audit_page keyboard   "Keyboard test"                  6
kill "$GPID" 2>/dev/null
audit_page wifi       "Network password"               4
kill "$GPID" 2>/dev/null
audit_page secureboot "Repeat the enrolment password"  4
kill "$GPID" 2>/dev/null
# account goes LAST and is deliberately left running: the keyboard-only section
# below drives it.
audit_page account    "Computer name"                  6

# ── the fields that matter, by name ───────────────────────────────────
section "the fields a user must be told about, by name"

named_has() {   # named_has <printable> <exact accessible name>
    if grep -qxF "NAMED-CTRL text box|$2" "$ATSPI_W/audit-all.txt" \
    || grep -qxF "NAMED-CTRL entry|$2" "$ATSPI_W/audit-all.txt" \
    || grep -qxF "NAMED-CTRL password text|$2" "$ATSPI_W/audit-all.txt"; then
        ok "$1"
    else
        bad "$1" "no named field '$2' anywhere in the installer"
    fi
}
named_has "the username field announces itself"                 "Username"
named_has "the password field announces itself"                 "Password"
named_has "the repeat-password field announces itself"          "Repeat password"
named_has "the computer-name field announces itself"            "Computer name"
named_has "the keyboard test field announces itself"            "Keyboard test"
named_has "the Wi-Fi password field announces itself"           "Network password"
named_has "the hidden-network field announces itself"           "Hidden network name"
named_has "the Secure Boot enrolment password announces itself" "One-time enrolment password"

# ── keyboard only ───────────────────────────────────────────────────────────
section "the installer can be driven with the keyboard alone"

# The account page's process is still up -- audit_page leaves the last one
# running precisely so the keyboard walk has something real to drive.
WID="$(xdotool search --name "APEX-OS Installer" 2>/dev/null | head -1)"
if [ -z "$WID" ]; then
    bad "the installer window can be found on the private display" ""
    finish; exit 1
fi
ok "the installer window can be found on the private display"
xdotool windowactivate --sync "$WID" >/dev/null 2>&1
xdotool windowfocus "$WID" >/dev/null 2>&1

focused_name() {
    python3 "$WALK" --json 2>/dev/null | python3 -c '
import json,sys
def flat(n,o=None):
    o=[] if o is None else o
    o.append(n)
    for c in n["children"]: flat(c,o)
    return o
for r in json.load(sys.stdin):
    for n in flat(r):
        if "focused" in n["states"]:
            print("%s|%s" % (n["role"], n["name"] or n["description"] or ""))
            sys.exit(0)
'
}

first="$(focused_name)"
if [ -n "$first" ]; then
    ok "something has keyboard focus when the page opens (${first#*|})"
else
    bad "something has keyboard focus when the page opens" \
        "nothing reports the focused state — a keyboard user starts nowhere"
fi

# Walk the ring with real Tab presses. Bounded: an unbounded walk on a page with
# a focus trap never returns.
RING=""
TAPS=14
for i in $(seq 1 "$TAPS"); do
    xdotool key --window "$WID" --clearmodifiers Tab >/dev/null 2>&1
    sleep 0.35
    f="$(focused_name)"
    RING="$RING
$f"
done
printf '%s\n' "$RING" | grep -v '^$' | sed 's/^/      tab → /'

moved="$(printf '%s\n' "$RING" | grep -v '^$' | sort -u | wc -l)"
if [ "$moved" -ge 4 ]; then
    ok "Tab really moves focus around the page ($moved distinct controls)"
else
    bad "Tab really moves focus around the page" \
        "only $moved distinct control(s) were ever focused — a keyboard trap"
fi

# Every control the Tab ring visits must be one a reader can name. This is the
# join between the two halves of the criterion: reachable AND announceable. A
# page can pass the audit above and still tab into an unnamed GTK internal.
blank="$(printf '%s\n' "$RING" | grep -v '^$' | grep -c '|$')"
if [ "$blank" -eq 0 ]; then
    ok "every control the Tab ring reaches has a name"
else
    bad "every control the Tab ring reaches has a name" \
        "$blank of $TAPS stops announced nothing"
fi

for want in "Username" "Password" "Repeat password" "Computer name" "Continue"; do
    if printf '%s\n' "$RING" | grep -qF "|$want"; then
        ok "Tab reaches '$want'"
    else
        bad "Tab reaches '$want'" "not in $TAPS Tab presses"
    fi
done

# The ring must close. A ring that never returns to its first member is one a
# keyboard user can fall out of.
firstname="${first#*|}"
if [ -n "$firstname" ] && printf '%s\n' "$RING" | grep -qF "|$firstname"; then
    ok "the Tab ring comes back round to where it started"
else
    bad "the Tab ring comes back round to where it started" \
        "'$firstname' never returned within $TAPS presses"
fi

# ── the keyboard alone can complete the page ────────────────────────────────
section "the keyboard alone can fill the page in and move on"

# Typed, not set. xdotool delivers real X key events to the real toolkit, so
# this exercises the same path a person's fingers do.
type_into() {   # type_into <accessible name> <text>
    for i in $(seq 1 30); do
        f="$(focused_name)"
        [ "${f#*|}" = "$1" ] && { xdotool type --window "$WID" --clearmodifiers "$2" >/dev/null 2>&1; return 0; }
        xdotool key --window "$WID" --clearmodifiers Tab >/dev/null 2>&1
        sleep 0.3
    done
    return 1
}

typed=0
type_into "Username"        "tester"   && typed=$((typed+1))
type_into "Password"        "s3cret-pw" && typed=$((typed+1))
type_into "Repeat password" "s3cret-pw" && typed=$((typed+1))
if [ "$typed" -eq 3 ]; then
    ok "the account fields can all be reached and typed into with the keyboard"
else
    bad "the account fields can all be reached and typed into with the keyboard" \
        "only $typed of 3 were reachable"
fi

# And the values really landed -- typing that goes nowhere would otherwise look
# identical to typing that works.
python3 "$WALK" --json >"$ATSPI_W/tree2.json" 2>/dev/null
uname_text="$(python3 "$WALK" --get-text "Username" 2>/dev/null)"
if [ "$uname_text" = "tester" ]; then
    ok "what was typed on the keyboard is what the field now contains"
else
    bad "what was typed on the keyboard is what the field now contains" \
        "the username field reads '$uname_text'"
fi

# The password must STILL be masked on the bus after being typed there.
pw_text="$(python3 "$WALK" --get-text "Password" 2>/dev/null)"
if printf '%s' "$pw_text" | grep -qF "s3cret-pw"; then
    bad "the password the user typed never crosses the accessibility bus" \
        "GetText returned it"
else
    ok "the password the user typed never crosses the accessibility bus"
fi

finish
