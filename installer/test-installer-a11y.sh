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
#  ── What is covered, and what is not ────────────────────────────────────────
#
#  The installer has eleven pages. SIX are audited here: welcome, keyboard,
#  wifi, secureboot, confirm and account. The name audit runs on all six; the
#  Tab ring is walked on `account`, which has the most fields and both
#  passwords; the keyboard-only page advance is measured on welcome → keyboard,
#  where the button is gated on nothing.
#
#  The `wifi` page has two shapes and the one the installer builds depends on
#  the machine: with no Wi-Fi adapter it returns an explanation and two buttons,
#  with no password field on it at all. Both shapes are audited; the two wifi
#  field-name checks SKIP with the reason on a machine with no adapter, because
#  a field that is not built is not a field that is unnamed.
#
#  Not audited, and named rather than left to look covered: `disk`, `mode` and
#  `part` enumerate real block devices and would assert about this machine's
#  hardware; `run` starts an install; `done` follows one. `confirm` IS audited,
#  using the GUI's own test affordance with a disk that does not exist, because
#  its one text field is the last thing between a user and an irreversible
#  erase.
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
for _ in $(seq 1 60); do [ -e "/tmp/.X11-unix/X${DISP#:}" ] && break; sleep 0.25; done
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

audit_page() {   # audit_page <page> <sentinel accessible name> <floor> [KEY=VAL ...]
    local page="$1" sentinel="$2" floor="$3" n
    shift 3

    for _ in $(seq 1 40); do
        n="$(python3 "$WALK" --count 2>/dev/null || echo 0)"
        [ "$n" = "0" ] && break
        sleep 0.3
    done
    if [ "$n" != "0" ]; then
        bad "page '$page': the previous page's process left the registry" \
            "$n still registered; this page's counts would include them"
        return
    fi

    # Extra KEY=VAL arguments are the state a later page needs before it can be
    # built at all — the disk, the install mode. They go through `env` rather
    # than being exported, so one page's stand-in state cannot leak into the
    # next page's process.
    APEX_GUI_PAGE="$page" atspi_run_app env "$@" python3 "$GUI" \
        >"$ATSPI_W/gui-$page.out" 2>"$ATSPI_W/gui-$page.err" &
    GPID=$!

    # Wait for a NAMED thing this page is about to be asserted on, not for the
    # node count to stop changing. A stability poll was tried first and it caught
    # a pause mid-construction, declaring a 45-node tree settled when the
    # finished page has far more -- the audit then ran over a page that did not
    # exist yet, and would have reported "every control has a name" about almost
    # nothing.
    local built=0
    for _ in $(seq 1 100); do
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

# ── the Tab ring, on whichever page is up ───────────────────────────────────
#
# Rounds 18/18b walked the ring on `account` only. Every other page was
# name-audited, which answers "can a reader say what this is" and not "can a
# keyboard user get to it" — so a focus trap on any other page was invisible
# to the whole suite. The walk is a function now and runs on four pages.
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

walk_ring() {   # walk_ring <page> <wid> <taps> <distinct floor> [required name ...]
    local page="$1" wid="$2" taps="$3" floor="$4"
    shift 4
    local first ring f moved blank firstname want

    xdotool windowactivate --sync "$wid" >/dev/null 2>&1
    xdotool windowfocus "$wid" >/dev/null 2>&1

    # Focused AND named. This assertion used to accept any focused node, and on
    # the wifi page it passed while printing an empty name: focus opened on the
    # ScrolledWindow around the network list, which GTK makes focusable so it
    # can be scrolled from the keyboard. "Something has focus" is not the
    # property — the property is that a reader user is told where they are.
    first="$(focused_name)"
    firstname="${first#*|}"
    if [ -n "$firstname" ]; then
        ok "page '$page': the control focused when the page opens says what it is ($firstname)"
    elif [ -n "$first" ]; then
        bad "page '$page': the control focused when the page opens says what it is" \
            "focus is on a '${first%%|*}' with no name and no description — a reader user is told nothing about where they are"
    else
        bad "page '$page': the control focused when the page opens says what it is" \
            "nothing reports the focused state — a keyboard user starts nowhere"
    fi

    # Bounded: an unbounded walk on a page with a focus trap never returns.
    ring=""
    local _i
    for _i in $(seq 1 "$taps"); do
        xdotool key --window "$wid" --clearmodifiers Tab >/dev/null 2>&1
        sleep 0.35
        f="$(focused_name)"
        ring="$ring
$f"
    done
    printf '%s\n' "$ring" | grep -v '^$' | sed "s/^/      $page tab → /"

    RING_OUT="$ring"
    moved="$(printf '%s\n' "$ring" | grep -v '^$' | sort -u | wc -l)"
    if [ "$moved" -ge "$floor" ]; then
        ok "page '$page': Tab really moves focus around it ($moved distinct controls)"
    else
        bad "page '$page': Tab really moves focus around it" \
            "only $moved distinct control(s) were ever focused, wanted $floor — a keyboard trap"
    fi

    # The join between the two halves of the criterion: reachable AND
    # announceable. A page can pass the audit and still tab into an unnamed
    # GTK internal.
    blank="$(printf '%s\n' "$ring" | grep -v '^$' | grep -c '|$')"
    if [ "$blank" -eq 0 ]; then
        ok "page '$page': every control the Tab ring reaches has a name"
    else
        bad "page '$page': every control the Tab ring reaches has a name" \
            "$blank of $taps stops announced nothing"
    fi

    for want in "$@"; do
        if printf '%s\n' "$ring" | grep -qF "|$want"; then
            ok "page '$page': Tab reaches '$want'"
        else
            bad "page '$page': Tab reaches '$want'" "not in $taps Tab presses"
        fi
    done

    # The ring must close. One that never returns to its first member is one a
    # keyboard user can fall out of.
    firstname="${first#*|}"
    if [ -n "$firstname" ] && printf '%s\n' "$ring" | grep -qF "|$firstname"; then
        ok "page '$page': the Tab ring comes back round to where it started"
    else
        bad "page '$page': the Tab ring comes back round to where it started" \
            "'$firstname' never returned within $taps presses"
    fi
}

ring_now() {   # ring_now <page> <a name the ring must reach> — walk the page that is up
    local page="$1" sentinel="$2" wid
    if ! kill -0 "$GPID" 2>/dev/null; then
        skp "page '$page': its Tab ring can be walked" \
            "the page's process is not running — see the audit above"
        return
    fi
    wid="$(xdotool search --name "APEX-OS Installer" 2>/dev/null | head -1)"
    if [ -z "$wid" ]; then
        skp "page '$page': its Tab ring can be walked" \
            "no installer window on the private display to send Tab to"
        return
    fi
    walk_ring "$page" "$wid" 12 3 "$sentinel"
}

# Typing, reaching and reading back. Defined here rather than beside the
# account page that used to own them, because the confirmation page's walk
# needs them too and a function must exist before the first call.
type_text() {   # type_text <text> — returns 0 if either mechanism was attempted
    xdotool windowfocus "$WID" >/dev/null 2>&1
    xdotool type --clearmodifiers --delay 40 "$1" >/dev/null 2>&1
}
type_text_fallback() {
    xdotool type --window "$WID" --clearmodifiers --delay 40 "$1" >/dev/null 2>&1
}

reach() {   # reach <accessible name> — leave focus on it, bounded
    for _ in $(seq 1 30); do
        f="$(focused_name)"
        [ "${f#*|}" = "$1" ] && return 0
        xdotool key --window "$WID" --clearmodifiers Tab >/dev/null 2>&1
        sleep 0.3
    done
    return 1
}

type_into() {   # type_into <accessible name> <text>
    reach "$1" || return 1
    type_text "$2"
    return 0
}

# The confirmation page needs its own walk, and the reason is the finding that
# produced it. A generic "Tab reaches at least three controls" went RED here:
# the ring has exactly two stops, Back and the ERASE field. That is not a
# keyboard trap, it is the page working as designed — `go` is built with
# sensitive=False and only `e_erase`'s changed handler turns it on, when the
# text is exactly ERASE. An insensitive button is not a tab stop in GTK.
#
# So the floor was the wrong assertion and the right one is stronger, in both
# directions: the destructive button must NOT be reachable before the words are
# typed, and it MUST be reachable after. That is the whole keyboard-only path
# through the last screen before an irreversible erase.
ring_confirm() {
    local wid found
    if ! kill -0 "$GPID" 2>/dev/null; then
        skp "page 'confirm': its Tab ring can be walked" \
            "the page's process is not running — see the audit above"
        return
    fi
    wid="$(xdotool search --name "APEX-OS Installer" 2>/dev/null | head -1)"
    if [ -z "$wid" ]; then
        skp "page 'confirm': its Tab ring can be walked" \
            "no installer window on the private display to send Tab to"
        return
    fi
    WID="$wid"

    walk_ring confirm "$wid" 12 2 "Type ERASE to confirm"

    if printf '%s\n' "$RING_OUT" | grep -qF "|Erase and install"; then
        bad "page 'confirm': the erase button is out of reach until ERASE is typed" \
            "it is already a tab stop with the field empty — a keyboard user can reach an irreversible action without confirming it"
    else
        ok "page 'confirm': the erase button is out of reach until ERASE is typed"
    fi

    if ! reach "Type ERASE to confirm"; then
        skp "page 'confirm': typing ERASE puts the erase button in the Tab ring" \
            "Tab never returned to the confirmation field"
        return
    fi
    type_text "ERASE"
    local got=""
    for _ in $(seq 1 12); do
        got="$(python3 "$WALK" --get-text "Type ERASE to confirm" 2>/dev/null)"
        [ "$got" = "ERASE" ] && break
        sleep 0.3
    done
    if [ "$got" != "ERASE" ]; then
        # CI has already shown one X server that accepts Tab and Return and
        # drops synthesised text. A COULD-NOT-RUN, not a pass and not a defect.
        skp "page 'confirm': typing ERASE puts the erase button in the Tab ring" \
            "the field reads [$got] after typing, so no text was delivered on this display"
        return
    fi

    found=0
    for _ in $(seq 1 12); do
        xdotool key --window "$wid" --clearmodifiers Tab >/dev/null 2>&1
        sleep 0.35
        f="$(focused_name)"
        [ "${f#*|}" = "Erase and install" ] && { found=1; break; }
    done
    if [ "$found" = 1 ]; then
        ok "page 'confirm': typing ERASE puts the erase button in the Tab ring, so the whole page can be completed with the keyboard"
    else
        bad "page 'confirm': typing ERASE puts the erase button in the Tab ring" \
            "the field reads ERASE and 12 more Tab presses never reached 'Erase and install'"
    fi
}

section "every control the installer builds announces itself, and Tab can reach it"
: >"$ATSPI_W/audit-all.txt"

# Sentinel + floor per page. The floors are what each page actually builds, so a
# page that silently loses a control fails here rather than quietly shrinking.
audit_page welcome    "Begin"                          2
kill "$GPID" 2>/dev/null
audit_page keyboard   "Keyboard test"                  6
ring_now keyboard "Keyboard test"
kill "$GPID" 2>/dev/null
# The wifi page has TWO shapes, and which one the installer builds depends on
# the MACHINE rather than on the code. `wifi_available()` asks
# `nmcli -t -f TYPE device` for a line reading exactly `wifi`; with no adapter --
# which is every CI runner -- the page it returns is a different page: an
# explanation and two buttons, with no password field and no hidden-network
# field on it at all.
#
# CI found this, and the way it failed is the reason it is worth fixing rather
# than pinning. The audit waited the full 40 seconds for a sentinel that could
# never appear and then reported `page 'wifi' builds and reaches the
# accessibility bus` as FAILED -- which reads as a defect in the installer, on a
# page that was working perfectly. Wrong in the most expensive direction.
#
# The same probe the GUI uses is asked here, and the shape that was audited is
# then CONFIRMED against the tree rather than assumed, so a wrong probe cannot
# silently downgrade the audit to a page with almost nothing on it.
WIFI_SHAPE=no-adapter
if command -v nmcli >/dev/null 2>&1 \
   && nmcli -t -f TYPE device 2>/dev/null | grep -qx 'wifi'; then
    WIFI_SHAPE=adapter
fi
if [ "$WIFI_SHAPE" = adapter ]; then
    audit_page wifi   "Network password"               4
else
    audit_page wifi   "Continue"                       2
fi
if [ "$WIFI_SHAPE" = no-adapter ]; then
    if grep -qE '\| name=(No Wi-Fi adapter detected\.|Wi-Fi is switched off by a hardware switch\.) \|' \
            "$ATSPI_W/dump-wifi.txt" 2>/dev/null; then
        ok "the wifi page built its no-adapter shape, and that is the shape audited"
    else
        bad "the wifi page built its no-adapter shape, and that is the shape audited" \
            "nmcli reported no wifi device, but the page says otherwise — the audit measured an unknown shape"
    fi
else
    ok "the wifi page built its adapter shape, and that is the shape audited"
fi
kill "$GPID" 2>/dev/null
audit_page secureboot "Repeat the enrolment password"  4
ring_now secureboot "Repeat the enrolment password"
kill "$GPID" 2>/dev/null
# The confirmation page is the one that matters most and it was not audited at
# all until now: its single text field is the last thing between the user and an
# irreversible erase. It needs state an earlier page would have chosen, so the
# GUI's own test affordance supplies it — with a disk that does not exist, so
# the page builds and nothing real is ever named as a target.
audit_page confirm    "Type ERASE to confirm"         3 \
    APEX_GUI_DISK=/dev/zzz-not-a-disk APEX_GUI_MODE=disk
ring_confirm
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
if [ "$WIFI_SHAPE" = adapter ]; then
    named_has "the Wi-Fi password field announces itself"       "Network password"
    named_has "the hidden-network field announces itself"       "Hidden network name"
else
    skp "the Wi-Fi password field announces itself" \
        "this machine has no Wi-Fi adapter, so the installer builds its no-adapter page and that field does not exist here — COULD-NOT-RUN, not a pass"
    skp "the hidden-network field announces itself" \
        "same: the no-adapter page does not build it"
fi
named_has "the Secure Boot enrolment password announces itself" "One-time enrolment password"
named_has "the ERASE confirmation field announces itself"       "Type ERASE to confirm"

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
walk_ring account "$WID" 14 4 \
    "Username" "Password" "Repeat password" "Computer name" "Continue"

section "the keyboard alone can fill the page in"

# Typed, not set. xdotool delivers real X key events to the real toolkit, so
# this exercises the same path a person's fingers do.
# Two delivery mechanisms, because they are not equivalent and CI proved it.
# `xdotool ... --window` sends XSendEvent; without --window it uses XTEST, which
# injects at the server the way a real keyboard does. On an ubuntu runner
# `key --window Tab` and `key --window Return` worked perfectly while
# `type --window` delivered NOTHING — the field read back empty after a type
# that reported success. `type` has to remap scratch keycodes for characters the
# server's keymap lacks, and a remap followed by XSendEvent races the client's
# own keymap cache. So text goes through XTEST first and falls back.
typed=0
type_into "Username"        "tester"   && typed=$((typed+1))
type_into "Password"        "s3cret-pw" && typed=$((typed+1))
type_into "Repeat password" "s3cret-pw" && typed=$((typed+1))
# Named for what it actually proves. It used to say "and typed into", which it
# has never measured: it only shows Tab reached each field. Whether the
# characters landed is the NEXT assertion, and CI caught the two disagreeing.
if [ "$typed" -eq 3 ]; then
    ok "the account fields can all be reached with the keyboard alone"
else
    bad "the account fields can all be reached with the keyboard alone" \
        "only $typed of 3 were reachable"
fi

# And the values really landed -- typing that goes nowhere would otherwise look
# identical to typing that works.
# Bounded retry, not a single read: the toolkit commits typed text on its own
# schedule and a runner under load is slower than this laptop. An unbounded wait
# would hang; a single read reported EMPTY on CI for text that had gone in.
read_username() {
    local t=""
    for _ in $(seq 1 12); do
        t="$(python3 "$WALK" --get-text "Username" 2>/dev/null)"
        [ "$t" = "tester" ] && break
        sleep 0.3
    done
    printf '%s' "$t"
}
uname_text="$(read_username)"

# If XTEST delivered nothing, try the other mechanism before concluding
# anything. Only if BOTH fail is this a property of the X server rather than of
# the installer.
TYPED_BY="XTEST"
if [ "$uname_text" != "tester" ]; then
    if reach "Username"; then
        # Select-all first: a PARTIAL XTEST delivery would otherwise be appended
        # to, and "tetester" is a wrong answer that looks like a different bug.
        xdotool key --window "$WID" --clearmodifiers ctrl+a >/dev/null 2>&1
        type_text_fallback "tester"
        uname_text="$(read_username)"
        [ "$uname_text" = "tester" ] && TYPED_BY="XSendEvent"
    fi
fi

TYPING_LANDED=0
if [ "$uname_text" = "tester" ]; then
    ok "what was typed on the keyboard is what the field now contains (via $TYPED_BY)"
    TYPING_LANDED=1
else
    # NOT a failure of the installer, and saying so is the whole point. Tab,
    # Return and space all reach this toolkit on this display — proved by every
    # assertion above and below — so what is missing is the harness's ability to
    # SYNTHESISE TEXT here, not the installer's ability to receive it. A red line
    # naming the installer would be a lie about which thing is broken.
    skp "what was typed on the keyboard is what the field now contains" \
        "neither XTEST nor XSendEvent delivered text to this X server (the field reads [$uname_text]), while Tab and Return both work — COULD-NOT-RUN, not a pass, and not an installer defect"
fi

# The password must STILL be masked on the bus after being typed there.
#
# Gated on the assertion above, and that gate is the point. If nothing was ever
# typed, "the bus does not hand out the password" is true of an empty field and
# says nothing at all -- and that is exactly the state CI was in when it found
# the read-back defect: this assertion passed while the username field was
# empty. An unmeasurable result is said out loud, never counted as a pass.
if [ "$TYPING_LANDED" = 1 ]; then
    pw_text="$(python3 "$WALK" --get-text "Password" 2>/dev/null)"
    if printf '%s' "$pw_text" | grep -qF "s3cret-pw"; then
        bad "the password the user typed never crosses the accessibility bus" \
            "GetText returned it"
    else
        ok "the password the user typed never crosses the accessibility bus"
    fi
else
    skp "the password the user typed never crosses the accessibility bus" \
        "nothing was typed, so an empty field would satisfy this — COULD-NOT-RUN, not a pass"
fi

# ── the keyboard alone can LEAVE a page ─────────────────────────────────────
section "the keyboard alone moves the installer from one page to the next"

# Everything above proves a keyboard user can REACH a control and fill it in.
# None of it proves they can leave the page. A primary button can be reachable,
# correctly named, announced perfectly — and completely inert to the keyboard.
# The user is then stuck on step 1 of 7 with a mouse the criterion says they do
# not have, and every assertion above is still green. The section this one
# follows used to be titled "…and move on", which nothing in it did.
#
# This is K11 from the installer keyboard work, one layer lower. There the
# forward edge existed in the page graph and could still have left the flow;
# here the edge exists and the KEY PRESS may not travel it.
#
# The edge measured is welcome → keyboard, deliberately. `Begin` is gated on
# nothing, so a red line here means the key did not activate the button and
# means nothing else. `Continue` on the account page is gated on the fields
# validating, which would make a failure ambiguous between the two.

ADV_BUILT=0; ADV_REACHED=0; ADV_MOVED=0; ADV_LEFT=0
advance_with() {   # advance_with <xdotool key name>
    local key="$1" f wid
    ADV_BUILT=0; ADV_REACHED=0; ADV_MOVED=0; ADV_LEFT=0

    # A fresh process on the welcome page. The account page's process has to be
    # gone from the registry first, or the walk below reads two trees at once
    # and `Begin` is "reachable" on a page that is not on screen.
    kill "$GPID" 2>/dev/null
    for _ in $(seq 1 40); do
        [ "$(python3 "$WALK" --count 2>/dev/null || echo 0)" = "0" ] && break
        sleep 0.3
    done

    APEX_GUI_PAGE=welcome atspi_run_app python3 "$GUI" \
        >"$ATSPI_W/gui-adv-$key.out" 2>"$ATSPI_W/gui-adv-$key.err" &
    GPID=$!
    for _ in $(seq 1 100); do
        python3 "$WALK" --dump >"$ATSPI_W/dump-adv-$key.txt" 2>/dev/null
        grep -q '| name=Begin |' "$ATSPI_W/dump-adv-$key.txt" && { ADV_BUILT=1; break; }
        kill -0 "$GPID" 2>/dev/null || break
        sleep 0.4
    done
    [ "$ADV_BUILT" = 1 ] || return 0

    wid="$(xdotool search --name "APEX-OS Installer" 2>/dev/null | head -1)"
    [ -n "$wid" ] || return 0
    xdotool windowactivate --sync "$wid" >/dev/null 2>&1
    xdotool windowfocus "$wid" >/dev/null 2>&1

    for _ in $(seq 1 20); do
        f="$(focused_name)"
        [ "${f#*|}" = "Begin" ] && { ADV_REACHED=1; break; }
        xdotool key --window "$wid" --clearmodifiers Tab >/dev/null 2>&1
        sleep 0.3
    done
    [ "$ADV_REACHED" = 1 ] || return 0

    xdotool key --window "$wid" --clearmodifiers "$key" >/dev/null 2>&1
    for _ in $(seq 1 40); do
        python3 "$WALK" --dump >"$ATSPI_W/dump-adv-$key.txt" 2>/dev/null
        grep -q '| name=Keyboard test |' "$ATSPI_W/dump-adv-$key.txt" && { ADV_MOVED=1; break; }
        sleep 0.4
    done
    # The page must be GONE, not merely overlaid. A dialog on top of the welcome
    # page would satisfy "the next page's sentinel appeared" on its own.
    #
    # Bounded settle window, and it is not a "wait until it passes": the dump
    # above is taken the instant the NEXT page's sentinel first appears, and GTK
    # tears the old page's widgets off the accessibility bus a beat later. CI
    # went red here on 2026-09-12 with no installer change on either side since
    # this assertion was written — `Begin` was still on the bus on the runner
    # and gone on this laptop, which is a race in the assertion rather than a
    # page that stayed. If `Begin` is still there after the window it is still a
    # FAILURE, and the mutant for an inert button (B7) never reaches this line
    # at all, because the next page's sentinel never appears.
    for _ in $(seq 1 25); do
        grep -q '| name=Begin |' "$ATSPI_W/dump-adv-$key.txt" || { ADV_LEFT=1; break; }
        sleep 0.4
        python3 "$WALK" --dump >"$ATSPI_W/dump-adv-$key.txt" 2>/dev/null
    done
    return 0
}

for key in Return space; do
    advance_with "$key"

    # Floor first: each assertion below is about what a key press did, and all
    # of them are vacuous if the page never built or the button was never
    # focused. Those two are reported as their own failures rather than folded
    # into a misleading "the key did not work".
    if [ "$ADV_BUILT" = 1 ]; then
        ok "[$key] the welcome page builds in its own process"
    else
        bad "[$key] the welcome page builds in its own process" \
            "$(grep -v 'libEGL\|DRI3\|Adwaita-WARNING' "$ATSPI_W/gui-adv-$key.err" 2>/dev/null | tail -3 | tr '\n' ' ')"
        continue
    fi

    if [ "$ADV_REACHED" = 1 ]; then
        ok "[$key] Tab alone puts focus on the welcome page's primary button"
    else
        bad "[$key] Tab alone puts focus on the welcome page's primary button" \
            "'Begin' never took focus in 20 Tab presses — a keyboard user cannot press it"
        continue
    fi

    if [ "$ADV_MOVED" = 1 ]; then
        ok "[$key] pressing it with the keyboard alone opens the next page"
    else
        bad "[$key] pressing it with the keyboard alone opens the next page" \
            "the keyboard page never appeared; the installer is keyboard-navigable but not keyboard-COMPLETABLE"
    fi

    if [ "$ADV_LEFT" = 1 ]; then
        ok "[$key] and the page it came from is gone, so the flow really advanced"
    else
        bad "[$key] and the page it came from is gone, so the flow really advanced" \
            "'Begin' is still on the bus — the next page appeared over the old one rather than replacing it"
    fi
done

finish
