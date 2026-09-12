#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  test-installer-keymap.sh — does the layout the user picks actually reach the
#  keys under their hands? (roadmap P2-004, "keyboard layout before password
#  creation")
#
#  ── What this suite is for, and why it is not the locale suite ──────────────
#
#  test-installer-locale.sh proves the choice reaches the INSTALLED system: the
#  answers file, the engine, /etc/vconsole.conf, /etc/X11/xorg.conf.d. All
#  necessary, none of it the criterion. The criterion is about the password the
#  user types INTO THE INSTALLER, minutes before any of those files exist. A
#  layout that is faithfully recorded and not applied leaves exactly the lockout
#  this feature exists to prevent, with the user believing they chose otherwise.
#
#  So this suite asks the running software instead: given a compositor started a
#  particular way, what character does a physical key produce in a GTK client?
#
#  ── The mechanism, measured rather than assumed ─────────────────────────────
#
#  A wlroots compositor compiles its keymap with all-NULL RMLVO, which is the
#  call that makes libxkbcommon consult XKB_DEFAULT_{RULES,MODEL,LAYOUT,VARIANT,
#  OPTIONS}. Those five variables are its ENTIRE configuration surface —
#  libxkbcommon links only libc, and contains no string naming
#  org.freedesktop.locale1, localectl or xorg.conf. Both facts are asserted
#  below rather than left as claims, because the whole design rests on them:
#  they are why `localectl set-x11-keymap` cannot change a running session, and
#  therefore why apex-installer-session restarts the compositor at all.
#
#  ── Why it needs Xvfb, and what happens without it ──────────────────────────
#
#  The end-to-end half needs a seat with a REAL KEYBOARD on it. The wlroots
#  headless backend creates no input device, so the seat has no keyboard
#  capability, GDK falls back to a fixed keymap, and every reading comes back
#  `us` whatever XKB_DEFAULT_LAYOUT says. That is a false green of the most
#  dangerous kind — a probe that answers confidently and is not measuring the
#  thing — so it is not merely avoided here, it is ASSERTED as a known-insensitive
#  configuration (test 3c). If that assertion ever starts failing, the headless
#  backend has grown input devices and the guard can go.
#
#  The wlroots X11 backend DOES create a keyboard, so the real measurement runs
#  under a private Xvfb. No Xvfb, no cage, no GTK: SKIP, never a silent pass.
#
#  ── Headless discipline ─────────────────────────────────────────────────────
#
#  Nothing here touches the ambient session. WAYLAND_DISPLAY, DISPLAY and
#  HYPRLAND_INSTANCE_SIGNATURE are cleared; XDG_RUNTIME_DIR is a private mktemp;
#  the X display number is probed for a free one and the Xvfb serving it is this
#  script's own child, killed on every exit path. It never opens a window on
#  anybody's desktop.
#
#  Run from anywhere: ./installer/test-installer-keymap.sh
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail
# Deliberately +e, like every suite in this tree: CI invokes a suite as
# `bash -e {0}`, and under -e an assignment from a failing command ends the run
# silently, mid-section.
set +e

cd "$(dirname "$0")" || exit 2
SESSION="$PWD/apex-installer-session"
GUI="$PWD/apex-installer-gui"
for f in "$SESSION" "$GUI"; do
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

W="$(mktemp -d "${TMPDIR:-/tmp}/apex-keymap.XXXXXX")" || exit 2
XVFB_PID=""
cleanup() {
    [ -n "$XVFB_PID" ] && kill "$XVFB_PID" 2>/dev/null
    rm -rf "$W"
}
trap cleanup EXIT INT TERM

finish() {
    printf '\ninstaller-keymap: %d passed, %d failed, %d skipped\n' "$pass" "$fail" "$skip"
    [ "$fail" -eq 0 ]
}

# Private runtime dir. Set before anything can create a socket in the real one.
unset WAYLAND_DISPLAY HYPRLAND_INSTANCE_SIGNATURE DISPLAY
export XDG_RUNTIME_DIR="$W/rt"
mkdir -p "$XDG_RUNTIME_DIR"
chmod 700 "$XDG_RUNTIME_DIR"   # wayland refuses group/world-readable runtime dirs

# ═════════════════════════════════════════════════════════════════════════════
section "1. libxkbcommon is the only thing that decides, and it reads only the env"
# ═════════════════════════════════════════════════════════════════════════════
# The design rests on these two facts. If either stops being true, the choice
# between "restart the compositor" and "call localectl" changes, and this suite
# should be the thing that says so.

LIBXKB=""
for c in /usr/lib64/libxkbcommon.so.0 /usr/lib/x86_64-linux-gnu/libxkbcommon.so.0 \
         /usr/lib/libxkbcommon.so.0; do
    [ -e "$c" ] && { LIBXKB="$c"; break; }
done

if [ -z "$LIBXKB" ]; then
    skp "libxkbcommon located" "not found in the usual paths"
else
    ok "libxkbcommon located ($LIBXKB)"

    # (1a) It links no dbus and no systemd. `localectl set-x11-keymap` signals
    # localed over dbus; a library that cannot speak dbus cannot hear it.
    deps="$(ldd "$LIBXKB" 2>/dev/null | grep -Eo 'lib(dbus|systemd)[^ ]*')"
    if [ -z "$deps" ]; then
        ok "libxkbcommon links neither libdbus nor libsystemd (so localed cannot reach it)"
    else
        bad "libxkbcommon links neither libdbus nor libsystemd" "found: $deps"
    fi

    # (1b) The POSITIVE half first, and deliberately in this order. The next
    # assertion reads evidence out of `strings` SILENCE, and silence from a
    # missing or failing `strings` looks identical to silence from a clean
    # library. So prove the tool can find something in this file before any
    # conclusion is drawn from it not finding something else.
    strings "$LIBXKB" > "$W/xkb.strings" 2>/dev/null
    if [ -s "$W/xkb.strings" ] && grep -qx 'XKB_DEFAULT_LAYOUT' "$W/xkb.strings"; then
        ok "libxkbcommon carries the XKB_DEFAULT_LAYOUT name it is configured by"
        xkb_strings_ok=1
    else
        bad "libxkbcommon carries the XKB_DEFAULT_LAYOUT name it is configured by" \
            "$([ -s "$W/xkb.strings" ] && echo "not found in $(wc -l < "$W/xkb.strings") strings" \
                                      || echo "strings produced nothing — is binutils installed?")"
        xkb_strings_ok=0
    fi

    # (1c) It names no config file and no dbus interface. Only meaningful
    # because (1b) just showed this same command CAN find a name in this file.
    if [ "${xkb_strings_ok:-0}" -ne 1 ]; then
        skp "libxkbcommon names no locale1 / localectl / xorg.conf string" \
            "strings could not be trusted here, so its silence proves nothing"
    else
        refs="$(grep -Ei 'org\.freedesktop\.locale1|localectl|xorg\.conf' "$W/xkb.strings")"
        if [ -z "$refs" ]; then
            ok "libxkbcommon names no locale1 / localectl / xorg.conf string"
        else
            bad "libxkbcommon names no locale1 / localectl / xorg.conf string" \
                "found: $(printf '%s' "$refs" | tr '\n' ' ')"
        fi
    fi
fi

# ═════════════════════════════════════════════════════════════════════════════
section "2. the exact call wlroots makes, driven directly"
# ═════════════════════════════════════════════════════════════════════════════
# xkb_keymap_new_from_names(ctx, NULL, 0). All-NULL RMLVO is what sends
# libxkbcommon to the environment. Driven through ctypes against the real
# library — not a reimplementation of the rule, the rule itself.

cat > "$W/xkbprobe.py" <<'PY'
import ctypes, os, sys
x = ctypes.CDLL("libxkbcommon.so.0")
x.xkb_context_new.restype = ctypes.c_void_p
x.xkb_keymap_new_from_names.restype = ctypes.c_void_p
x.xkb_keymap_new_from_names.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_int]
x.xkb_state_new.restype = ctypes.c_void_p
x.xkb_state_new.argtypes = [ctypes.c_void_p]
x.xkb_state_key_get_utf8.argtypes = [ctypes.c_void_p, ctypes.c_uint32,
                                     ctypes.c_char_p, ctypes.c_size_t]
# evdev keycode + 8, the offset xkb uses: KEY_Y=21->29, KEY_Z=44->52,
# KEY_Q=16->24, KEY_SEMICOLON=39->47.
CODES = [29, 52, 24, 47]
def compile_and_read():
    ctx = x.xkb_context_new(0)
    if not ctx: return None
    km = x.xkb_keymap_new_from_names(ctx, None, 0)
    if not km: return None
    st = x.xkb_state_new(km)
    if not st: return None
    buf = ctypes.create_string_buffer(32)
    out = []
    for c in CODES:
        x.xkb_state_key_get_utf8(st, c, buf, 32)
        out.append(buf.value.decode('utf-8', 'replace'))
    return out
first = compile_and_read()
if first is None:
    print("ERROR"); sys.exit(1)
print(" ".join(first))
# Change the variable in this live process and recompile. This is what makes a
# restart necessary rather than an env poke: the value is consumed when the
# keymap is BUILT.
os.environ["XKB_DEFAULT_LAYOUT"] = "fr"
second = compile_and_read()
print(" ".join(second) if second else "ERROR")
PY

if ! command -v python3 >/dev/null 2>&1; then
    skp "the wlroots keymap call is driven directly" "no python3"
else
    out_us="$(XKB_DEFAULT_LAYOUT=us python3 "$W/xkbprobe.py" 2>/dev/null | head -1)"
    out_de="$(XKB_DEFAULT_LAYOUT=de python3 "$W/xkbprobe.py" 2>/dev/null | head -1)"
    out_fr="$(XKB_DEFAULT_LAYOUT=fr python3 "$W/xkbprobe.py" 2>/dev/null | head -1)"

    is "XKB_DEFAULT_LAYOUT=us gives a US keyboard"     "y z q ;" "$out_us"
    is "XKB_DEFAULT_LAYOUT=de gives a German keyboard" "z y q ö" "$out_de"
    is "XKB_DEFAULT_LAYOUT=fr gives a French keyboard" "y w a m" "$out_fr"

    # The sensitivity check. Three readings that differ from each other is the
    # evidence that the probe measures anything at all; without it, three
    # matching expectations could all be the same constant.
    if [ "$out_us" != "$out_de" ] && [ "$out_de" != "$out_fr" ]; then
        ok "the three layouts really do differ (the probe is sensitive, not constant)"
    else
        bad "the three layouts really do differ" \
            "us[$out_us] de[$out_de] fr[$out_fr] — the probe is not measuring the layout"
    fi

    # Set XKB_DEFAULT_LAYOUT=fr inside a process that already compiled `de`, then
    # recompile: the SECOND keymap is French, so the variable is read at compile
    # time. That is the whole reason apex-installer-session restarts cage
    # instead of exporting into a running one.
    second="$(XKB_DEFAULT_LAYOUT=de python3 "$W/xkbprobe.py" 2>/dev/null | sed -n 2p)"
    is "the env var is consumed when the keymap is COMPILED, so a restart is required" \
       "y w a m" "$second"
fi

# ═════════════════════════════════════════════════════════════════════════════
section "3. end to end: cage, a real keyboard, and what a GTK client receives"
# ═════════════════════════════════════════════════════════════════════════════
# This is the criterion itself. Everything above is the mechanism; this is a
# GTK4 client — the toolkit the installer is written in — being asked what
# character the physical Y key produces, under the compositor the installer
# actually runs on.

cat > "$W/gtkprobe.py" <<'PY'
import gi, sys
gi.require_version('Gtk', '4.0'); gi.require_version('Gdk', '4.0')
from gi.repository import Gtk, Gdk, GLib
KEYS = [("Y", 29), ("Z", 52), ("Q", 24), ("SEMI", 47)]
def dump(d):
    seat = d.get_default_seat()
    caps = int(seat.get_capabilities()) if seat else 0
    # GDK_SEAT_CAPABILITY_KEYBOARD is bit 1 (value 2). NOT seat.get_keyboard():
    # that returns the logical "Core Keyboard" device on every seat, present or
    # not, and reading it as presence is what made the first version of this
    # probe report a keyboard under a backend that has none.
    kb = bool(caps & 2)
    vals = []
    for name, code in KEYS:
        ok, _k, kv = d.map_keycode(code)
        vals.append("%s=%s" % (name, Gdk.keyval_name(kv[0]) if (ok and kv) else "UNMAPPED"))
    # GDK_SEAT_CAPABILITY_KEYBOARD is bit 1 (value 2).
    print("PROBE caps=%d keyboard=%s %s" % (caps, "yes" if kb else "no", " ".join(vals)),
          flush=True)
def on_activate(app):
    w = Gtk.ApplicationWindow(application=app); w.present()
    d = w.get_display()
    n = {'i': 0}
    def later():
        n['i'] += 1
        # Only the last reading is used: the seat advertises its keyboard and
        # sends the keymap asynchronously, so an immediate read races it.
        if n['i'] >= 3:
            dump(d); app.quit(); return False
        return True
    GLib.timeout_add(600, later)
app = Gtk.Application(application_id='dev.apex.keymapprobe')
app.connect('activate', on_activate)
sys.exit(app.run([]))
PY

# Adw as well as Gtk: section 6 imports the shipped GUI as a module, and that
# file calls gi.require_version("Adw", "1") at import time. A gate that checks
# only Gtk turns a missing gir1.2-adw-1 into a traceback and five failures
# instead of a skip that names the missing package.
have_gtk=0
python3 -c 'import gi
gi.require_version("Gtk","4.0"); gi.require_version("Adw","1")
from gi.repository import Gtk, Gdk, Adw' >/dev/null 2>&1 && have_gtk=1

if [ "$have_gtk" -ne 1 ] || ! command -v cage >/dev/null 2>&1; then
    skp "cage hands a GTK client the layout it was started with" \
        "needs cage and python3-gi with Gtk 4.0"
    skp "the headless backend is still keyboardless (false-green guard)" "same"
else
    export WLR_RENDERER=pixman GSK_RENDERER=cairo GDK_BACKEND=wayland
    export LIBGL_ALWAYS_SOFTWARE=1 WLR_NO_HARDWARE_CURSORS=1

    run_cage() {   # run_cage <backend> <layout> [display]
        local backend="$1" layout="$2" disp="${3:-}"
        env ${disp:+DISPLAY="$disp"} WLR_BACKENDS="$backend" \
            XKB_DEFAULT_LAYOUT="$layout" \
            timeout 40 cage -- python3 "$W/gtkprobe.py" 2>/dev/null \
            | grep -m1 '^PROBE'
    }

    # ── 3a/3b: the real measurement, on a seat that has a keyboard ───────────
    # The wlroots X11 backend creates one; the headless backend does not. So a
    # private Xvfb is what makes the criterion assertable at all.
    if ! command -v Xvfb >/dev/null 2>&1; then
        skp "cage hands a GTK client the layout it was started with" \
            "no Xvfb: the headless backend has no keyboard, so this cannot be measured here"
    else
        # A free display number, never a fixed one — :99 may be somebody's.
        DISP=""
        for n in $(seq 71 95); do
            if [ ! -e "/tmp/.X11-unix/X$n" ]; then DISP=":$n"; break; fi
        done
        if [ -z "$DISP" ]; then
            skp "cage hands a GTK client the layout it was started with" \
                "no free X display number in :71-:95"
        else
            Xvfb "$DISP" -screen 0 1280x800x24 >/dev/null 2>&1 &
            XVFB_PID=$!
            for _ in $(seq 1 50); do
                [ -e "/tmp/.X11-unix/X${DISP#:}" ] && break
                sleep 0.2
            done
            if [ ! -e "/tmp/.X11-unix/X${DISP#:}" ]; then
                skp "cage hands a GTK client the layout it was started with" \
                    "Xvfb did not come up on $DISP"
            else
                p_us="$(run_cage x11 us "$DISP")"
                p_de="$(run_cage x11 de "$DISP")"

                # Premise first. If the seat has no keyboard the readings below
                # are the fixed fallback and mean nothing, so this is a FAILURE
                # and not a skip — a suite that cannot tell must not report green.
                if printf '%s' "$p_de" | grep -q 'keyboard=yes'; then
                    ok "the X11 backend really does put a keyboard on the seat"
                else
                    bad "the X11 backend really does put a keyboard on the seat" \
                        "got: ${p_de:-<no probe line>} — every reading below would be a fallback"
                fi

                got_us="$(printf '%s' "$p_us" | sed -n 's/.*\(Y=[^ ]*\) \(Z=[^ ]*\) .*\(SEMI=[^ ]*\).*/\1 \2 \3/p')"
                got_de="$(printf '%s' "$p_de" | sed -n 's/.*\(Y=[^ ]*\) \(Z=[^ ]*\) .*\(SEMI=[^ ]*\).*/\1 \2 \3/p')"

                is "cage started us: a GTK client reads the US letters off the physical keys" \
                   "Y=y Z=z SEMI=semicolon" "$got_us"
                # THE CRITERION. Started de, the physical key engraved Y produces
                # z, the key engraved Z produces y, and the semicolon key gives ö
                # — which is what "the user can type their password on their own
                # layout" means in a measurement.
                is "cage started de: the same physical keys now produce the GERMAN letters" \
                   "Y=z Z=y SEMI=odiaeresis" "$got_de"
            fi
        fi
    fi

    # ── 3c: the false-green guard ───────────────────────────────────────────
    # The naive version of this suite ran under the headless backend, read `us`
    # under XKB_DEFAULT_LAYOUT=de, and would have reported a confident green for
    # a measurement that was not happening. Pin that configuration as KNOWN
    # INSENSITIVE so the trap cannot be walked into again, and so the day
    # wlroots' headless backend grows input devices, this says so.
    h_us="$(run_cage headless us)"
    h_de="$(run_cage headless de)"
    if [ -z "$h_de" ]; then
        skp "the headless backend is still keyboardless (false-green guard)" \
            "cage produced no probe line on the headless backend"
    elif printf '%s' "$h_de" | grep -q 'keyboard=yes'; then
        bad "the headless backend is still keyboardless (false-green guard)" \
            "it now reports a keyboard — re-check whether it honours the layout, and retire this guard"
    elif [ "$h_us" = "$h_de" ]; then
        ok "the headless backend ignores the layout, as documented — never measure the criterion there"
    else
        bad "the headless backend ignores the layout, as documented" \
            "us[$h_us] != de[$h_de] — the documented reason for needing Xvfb has changed"
    fi
fi

# ═════════════════════════════════════════════════════════════════════════════
section "4. apex-installer-session: the restart loop that applies the choice"
# ═════════════════════════════════════════════════════════════════════════════
# Driven with a stub cage and a stub GUI, so the loop's behaviour is measured
# without starting a compositor. The stub cage records the environment it was
# given, which is the only thing that matters: the layout has to arrive as
# XKB_DEFAULT_LAYOUT in the compositor's own environment.

mkdir -p "$W/bin"
cat > "$W/bin/cage" <<'STUB'
#!/usr/bin/env bash
# Stub cage: record the keymap environment of this invocation, then run the
# "GUI", which is itself a stub that decides what to exit with.
n=$(( $(cat "$STUB_COUNT" 2>/dev/null || echo 0) + 1 ))
printf '%s' "$n" > "$STUB_COUNT"
printf '%d layout=%s variant=%s resumed=%s\n' \
    "$n" "${XKB_DEFAULT_LAYOUT:-<unset>}" "${XKB_DEFAULT_VARIANT:-<unset>}" \
    "${APEX_INSTALLER_RESUMED:-<unset>}" >> "$STUB_LOG"
# Drop the leading `-s --` that the session script passes through.
while [ $# -gt 0 ]; do case "$1" in -s) shift ;; --) shift; break ;; *) break ;; esac; done
exec "$@"
STUB
chmod +x "$W/bin/cage"

# The stub GUI: asks for a restart the first N times, then exits 0.
cat > "$W/bin/gui" <<'STUB'
#!/usr/bin/env bash
n=$(cat "$STUB_COUNT" 2>/dev/null || echo 1)
want="${STUB_RESTARTS:-1}"
if [ "$n" -le "$want" ]; then
    # STUB_CYCLE asks for a different real layout every round. Without it the
    # session's "already in force" guard ends the run after one restart — which
    # is correct behaviour, and would stop the cap test ever reaching the cap.
    if [ -n "${STUB_CYCLE:-}" ]; then
        set -- de fr es it pl cz hu tr
        eval "l=\${$(( (n - 1) % 8 + 1 ))}"
        printf 'layout=%s\nvariant=\n' "$l" > "$STUB_STATE"
    else
        printf 'layout=%s\nvariant=%s\n' "${STUB_LAYOUT:-de}" "${STUB_VARIANT:-}" > "$STUB_STATE"
    fi
    exit 75
fi
exit 0
STUB
chmod +x "$W/bin/gui"

run_session() {   # run_session <restarts-wanted> [layout] [variant] [max]
    : > "$W/stub.log"; printf '0' > "$W/stub.count"; rm -f "$W/state"
    STUB_COUNT="$W/stub.count" STUB_LOG="$W/stub.log" STUB_STATE="$W/state" \
    STUB_RESTARTS="$1" STUB_LAYOUT="${2:-de}" STUB_VARIANT="${3:-}" \
    STUB_CYCLE="${5:-}" \
    APEX_INSTALLER_CAGE="$W/bin/cage" \
    APEX_INSTALLER_GUI="$W/bin/gui" \
    APEX_INSTALLER_STATE="$W/state" \
    APEX_INSTALLER_LOG="$W/session.log" \
    APEX_INSTALLER_MAX_RESTARTS="${4:-3}" \
    bash "$SESSION" >/dev/null 2>&1
    printf '%s' "$?"
}

# (4a) One restart: cage is started twice, and the SECOND start carries the
# layout. This is the entire feature in one assertion.
rc="$(run_session 1 de)"
is "a layout choice restarts the session (exit 0 after the restart)" "0" "$rc"
first="$(sed -n '1p' "$W/stub.log")"
second="$(sed -n '2p' "$W/stub.log")"
is "the first compositor start has no layout forced on it" \
   "1 layout=<unset> variant=<unset> resumed=<unset>" "$first"
is "the second start carries the chosen layout into the compositor's environment" \
   "2 layout=de variant=<unset> resumed=1" "$second"

# (4b) A variant travels too — a German user on `nodeadkeys` who is given plain
# `de` has a different keyboard from the one they picked.
run_session 1 de nodeadkeys >/dev/null
is "a chosen variant reaches the compositor as XKB_DEFAULT_VARIANT" \
   "2 layout=de variant=nodeadkeys resumed=1" "$(sed -n '2p' "$W/stub.log")"

# (4c) The cap. An unbounded restart loop behind a compositor is a black screen
# with a busy CPU — the exact outcome apex-installer-launch exists to rule out.
rc="$(run_session 99 de "" 2 cycle)"
is "a GUI that asks forever is capped, and reports a defect rather than looping" "70" "$rc"
starts="$(wc -l < "$W/stub.log" | tr -d ' ')"
is "the cap really bounds the number of compositor starts (2 restarts -> 3 starts)" \
   "3" "$starts"

# (4d) A layout xkb does not know must be refused. A well-shaped unknown name
# does not fail loudly — it silently degrades to `us`, which is the lockout this
# feature exists to end, with the user believing they chose otherwise.
rc="$(run_session 1 zzznotalayout)"
is "a layout xkb does not know is refused rather than compiled into a us fallback" "70" "$rc"
is "and the compositor is NOT restarted into it" "1" "$(wc -l < "$W/stub.log" | tr -d ' ')"

# (4e) Asking for the layout already in force must not restart. Besides being
# pointless it is how a GUI bug would burn the whole restart budget.
: > "$W/stub.log"; printf '0' > "$W/stub.count"; rm -f "$W/state"
rc=$(STUB_COUNT="$W/stub.count" STUB_LOG="$W/stub.log" STUB_STATE="$W/state" \
    STUB_RESTARTS=1 STUB_LAYOUT=de STUB_VARIANT="" \
    APEX_INSTALLER_CAGE="$W/bin/cage" APEX_INSTALLER_GUI="$W/bin/gui" \
    APEX_INSTALLER_STATE="$W/state" APEX_INSTALLER_LOG="$W/session.log" \
    XKB_DEFAULT_LAYOUT=de \
    bash "$SESSION" >/dev/null 2>&1; printf '%s' "$?")
is "asking for the layout already in force exits cleanly instead of restarting" "0" "$rc"
is "…and starts the compositor exactly once" "1" "$(wc -l < "$W/stub.log" | tr -d ' ')"

# (4f) A stale state file from an earlier boot must not choose a layout for
# somebody who never picked one.
#
# The first version of this asserted that the FIRST compositor start carried no
# layout, with a stub GUI that exited 0 immediately. That could not fail: the
# session only reads the state file after a restart request, so with no request
# the file was never consulted and the assertion held whether or not the clear
# happened. A mutant that deleted the clear outright survived it — the same
# shape as this unit's earlier N8, an assertion about a value that had never
# been shown capable of moving.
#
# The stale file only matters when the GUI ASKS for a restart and does not write
# one — a failed write, or a crash between the two. Then an uncleared file is a
# layout chosen by whoever used this machine last. So: seed a stale file, ask
# for a restart, write nothing.
cat > "$W/bin/gui-silent" <<'STUB'
#!/usr/bin/env bash
n=$(cat "$STUB_COUNT" 2>/dev/null || echo 1)
[ "$n" -le 1 ] && exit 75     # asks for a restart, deliberately writes no state
exit 0
STUB
chmod +x "$W/bin/gui-silent"

: > "$W/stub.log"; printf '0' > "$W/stub.count"
printf 'layout=ru\nvariant=\n' > "$W/state"
rc=$(STUB_COUNT="$W/stub.count" STUB_LOG="$W/stub.log" STUB_STATE="$W/state" \
    APEX_INSTALLER_CAGE="$W/bin/cage" APEX_INSTALLER_GUI="$W/bin/gui-silent" \
    APEX_INSTALLER_STATE="$W/state" APEX_INSTALLER_LOG="$W/session.log" \
    bash "$SESSION" >/dev/null 2>&1; printf '%s' "$?")
is "a restart request with no fresh state is refused, not served from a stale file" "70" "$rc"
is "…and the compositor is never restarted into the stale layout" \
   "1" "$(wc -l < "$W/stub.log" | tr -d ' ')"

# ═════════════════════════════════════════════════════════════════════════════
section "5. the GUI's half of the contract"
# ═════════════════════════════════════════════════════════════════════════════
# The session script and the GUI agree on three things: an exit code, a file
# path, and a file format. A mismatch in any of them is a feature that silently
# does nothing, so each is asserted against BOTH files rather than one.

if grep -q 'RESTART_RC = 75' "$GUI" && grep -q 'RESTART_RC=75' "$SESSION"; then
    ok "both halves use the same restart exit code (75)"
else
    bad "both halves use the same restart exit code (75)"
fi

# The format the session parses is layout=/variant=; the GUI must write exactly
# that. Checked by RUNNING the session against a file the GUI's own writer
# produced, rather than by grepping both for a string.
python3 - "$GUI" "$W/gui-format" <<'PY' 2>/dev/null
import re, sys
src = open(sys.argv[1]).read()
# The writer is an f-string that may be split across several adjacent literals.
# Take the whole f.write(...) call and join every fragment in it, rather than
# assuming a single literal — assuming one is what broke this check when a
# third key was added to the file.
call = re.search(r'f\.write\((.*?)\)\n', src, re.S)
out = ""
if call:
    frags = re.findall(r'f"([^"]*)"', call.group(1))
    out = "".join(frags)
    if "layout=" not in out:
        out = ""
out = (out.replace("{code}", "de")
          .replace("{variant}", "")
          .replace("{self.answers.get('timezone', '')}", "Europe/Berlin")
          .replace("\\n", "\n"))
open(sys.argv[2], "w").write(out)
PY
if [ -s "$W/gui-format" ]; then
    : > "$W/stub.log"; printf '0' > "$W/stub.count"
    cp "$W/gui-format" "$W/state"
    cat > "$W/bin/gui-once" <<'STUB'
#!/usr/bin/env bash
n=$(cat "$STUB_COUNT" 2>/dev/null || echo 1)
[ "$n" -le 1 ] && exit 75
exit 0
STUB
    chmod +x "$W/bin/gui-once"
    # The session clears the state file at startup, so the GUI-shaped file has
    # to be (re)written by the stub at the moment of the request.
    cat > "$W/bin/gui-fmt" <<STUB
#!/usr/bin/env bash
n=\$(cat "\$STUB_COUNT" 2>/dev/null || echo 1)
if [ "\$n" -le 1 ]; then cp "$W/gui-format" "\$STUB_STATE"; exit 75; fi
exit 0
STUB
    chmod +x "$W/bin/gui-fmt"
    STUB_COUNT="$W/stub.count" STUB_LOG="$W/stub.log" STUB_STATE="$W/state" \
        APEX_INSTALLER_CAGE="$W/bin/cage" APEX_INSTALLER_GUI="$W/bin/gui-fmt" \
        APEX_INSTALLER_STATE="$W/state" APEX_INSTALLER_LOG="$W/session.log" \
        bash "$SESSION" >/dev/null 2>&1
    if printf '%s' "$(sed -n '2p' "$W/stub.log")" | grep -q 'layout=de'; then
        ok "the exact line the GUI writes is the line the session parses"
    else
        bad "the exact line the GUI writes is the line the session parses" \
            "second start was: $(sed -n '2p' "$W/stub.log")"
    fi
    # The GUI writes `timezone=` into the same file so the choice survives the
    # restart. The session has no use for it — but its parser must IGNORE an
    # unknown key rather than refuse the restart over it, or carrying the time
    # zone would break the layout it rode along with.
    if grep -q 'timezone=' "$W/gui-format"; then
        ok "the GUI's state file carries the time zone across the restart"
    else
        bad "the GUI's state file carries the time zone across the restart" \
            "no timezone= in what the writer produces"
    fi
else
    bad "the exact line the GUI writes is the line the session parses" \
        "could not find the GUI's state-file writer"
fi

# Both halves must agree on WHERE the file lives, or the GUI writes a choice
# into a path nothing reads.
gui_path="$(grep -oE '"/run/apex-installer/session-keymap"' "$GUI" | head -1)"
ses_path="$(grep -oE '/run/apex-installer/session-keymap' "$SESSION" | head -1)"
if [ -n "$gui_path" ] && [ -n "$ses_path" ]; then
    ok "both halves default to the same state-file path"
else
    bad "both halves default to the same state-file path" \
        "gui[$gui_path] session[$ses_path]"
fi

# The launcher must actually run the session script. Wiring the loop and leaving
# the launcher on bare cage is a feature that exists and never runs.
if grep -qE '^GUI_CMD=\(/usr/bin/apex-installer-session\)' "$PWD/apex-installer-launch"; then
    ok "apex-installer-launch runs the session script, not cage directly"
else
    bad "apex-installer-launch runs the session script, not cage directly"
fi

# …and the thing it execs has to be IN THE IMAGE. This was a real defect, found
# only because it was looked for: the launcher was repointed at
# /usr/bin/apex-installer-session and nothing copied that file into the image, so
# the ISO would have had no installer at all — cage never starts, every boot
# lands on the diagnostic screen. Every assertion above passed while that was
# true, because they all read the source tree, where the file plainly exists.
#
# The target is resolved out of the launcher rather than hardcoded, so renaming
# the script cannot quietly slip past this.
_target="$(grep -oE '^GUI_CMD=\(([^ )]+)' "$PWD/apex-installer-launch" | cut -d'(' -f2)"
_base="$(basename "${_target:-none}")"
if [ -z "$_target" ]; then
    bad "the launcher's exec target is installed into the image" "could not read GUI_CMD"
else
    _missing=""
    grep -q "COPY $_base /usr/bin/$_base" "$PWD/Containerfile.installer" \
        || _missing="$_missing Containerfile.installer"
    grep -q "$_base" "$PWD/build-live-iso.sh" \
        || _missing="$_missing build-live-iso.sh"
    if [ -z "$_missing" ]; then
        ok "the launcher's exec target ($_base) is installed by both the Containerfile and the ISO build"
    else
        bad "the launcher's exec target ($_base) is installed into the image" \
            "absent from:$_missing — the ISO would boot with no installer"
    fi
fi

# ═════════════════════════════════════════════════════════════════════════════
section "6. the GUI page itself, driven for real"
# ═════════════════════════════════════════════════════════════════════════════
# Everything in section 5 reads the GUI's SOURCE — a grep for the exit code, a
# regex for the line it writes. That is the "greps a stanza that also names the
# thing" trap in another costume: the page could build, the constant could be
# right, the writer could be spelled correctly, and Continue could still do
# nothing at all. The one path that matters — pick a layout, press Continue,
# write the state file, exit 75 — had never been executed by anything.
#
# So run the SHIPPED apex-installer-gui under cage, on the keyboard page, and
# press its button. APEX_GUI_PAGE is the test affordance the GUI already carries.

if [ "$have_gtk" -ne 1 ] || ! command -v cage >/dev/null 2>&1; then
    skp "pressing Continue writes the layout and asks for a restart" \
        "needs cage + python3-gi with Gtk 4.0 AND Adw 1"
else
    cat > "$W/drive.py" <<'PY2'
# Driver: import the shipped GUI as a module, let it build, then operate the
# keyboard page the way a person would. Imported rather than re-implemented so
# that what is measured is the file that ships.
import importlib.util, os, sys, gi
gi.require_version('Gtk', '4.0'); gi.require_version('Adw', '1')
from gi.repository import GLib

spec = importlib.util.spec_from_loader(
    "apexgui", importlib.machinery.SourceFileLoader("apexgui", sys.argv[1]))
mod = importlib.util.module_from_spec(spec)
mod.__name__ = "apexgui"          # so its `if __name__ == "__main__"` stays quiet
spec.loader.exec_module(mod)

app = mod.Installer()
result = {"rc": None}

def drive():
    # Find the layout dropdown the page built, select `de`, and click the
    # primary button. Nothing here reaches into internals the page does not
    # already expose to itself.
    try:
        codes = [c for c, _ in mod.xkb_layouts()]
        idx = codes.index("de")
        app.kb_drop.set_selected(idx)
        # The action row is the last child of the page; find the apex-go button.
        page = app.stack.get_visible_child()
        def walk(w, out):
            c = w.get_first_child()
            while c is not None:
                if isinstance(c, mod.Gtk.Button):
                    out.append(c)
                walk(c, out)
                c = c.get_next_sibling()
        btns = []
        walk(page, btns)
        go = [b for b in btns if b.has_css_class("apex-go")]
        if not go:
            print("DRIVE no-primary-button", flush=True); app.quit(); return False
        go[0].emit("clicked")
        # Print what the page actually RECORDED, not merely that it did not
        # raise. An assertion that a driver survived is not an assertion about
        # the value it was supposed to collect.
        print("DRIVE clicked exit_code=%s keymap=%s variant=%s timezone=%s" % (
            app.exit_code, app.answers.get("keymap"),
            app.answers.get("keyvariant"), app.answers.get("timezone")), flush=True)
    except Exception as e:
        print("DRIVE error %s" % e, flush=True)
        app.quit()
    return False

def on_act(_a):
    GLib.timeout_add(900, drive)
app.connect("activate", on_act)
rc = app.run([])
print("DRIVE rc=%s" % (app.exit_code if app.exit_code is not None else rc), flush=True)
sys.exit(app.exit_code if app.exit_code is not None else rc)
PY2

    rm -f "$W/gui-state"
    drive_out="$(env WLR_BACKENDS=headless WLR_RENDERER=pixman GSK_RENDERER=cairo \
        GDK_BACKEND=wayland LIBGL_ALWAYS_SOFTWARE=1 \
        APEX_GUI_PAGE=keyboard APEX_INSTALLER_STATE="$W/gui-state" \
        XKB_DEFAULT_LAYOUT=us \
        timeout 90 cage -- python3 "$W/drive.py" "$GUI" 2>&1)"
    drive_rc=$?

    if printf '%s' "$drive_out" | grep -q 'DRIVE clicked'; then
        ok "the shipped keyboard page builds and its Continue button is reachable"
    else
        bad "the shipped keyboard page builds and its Continue button is reachable" \
            "$(printf '%s' "$drive_out" | grep -E '^DRIVE' | head -2 | tr '\n' ' ')"
    fi

    # THE UNTESTED PATH. Pressing Continue on a layout that is not the active one
    # must write the state file the session reads, and must ask for a restart by
    # exiting 75. Neither had ever run.
    if [ -s "$W/gui-state" ] && grep -q '^layout=de$' "$W/gui-state"; then
        ok "pressing Continue writes the chosen layout to the session state file"
    else
        bad "pressing Continue writes the chosen layout to the session state file" \
            "state file: [$(cat "$W/gui-state" 2>/dev/null | tr '\n' ' ')]"
    fi

    if printf '%s' "$drive_out" | grep -q 'DRIVE rc=75'; then
        ok "…and the process exits 75, which is what asks for the restart"
    else
        bad "…and the process exits 75, which is what asks for the restart" \
            "got rc=$drive_rc, driver said: $(printf '%s' "$drive_out" | grep -m1 'DRIVE rc=')"
    fi

    # The timezone travels in the ANSWERS file, not the state file, so it is
    # asserted on its own rather than inferred from the layout having worked.
    # The value is read back out of the live app, and it has to be a zone that
    # actually exists — "it did not crash" is not a measurement.
    got_tz="$(printf '%s' "$drive_out" | sed -n 's/.*timezone=\([^ ]*\).*/\1/p' | head -1)"
    if [ -n "$got_tz" ] && [ "$got_tz" != "None" ] \
       && [ -e "/usr/share/zoneinfo/$got_tz" ]; then
        ok "pressing Continue records a real time zone in the answers ($got_tz)"
    else
        bad "pressing Continue records a real time zone in the answers" \
            "got [${got_tz:-<nothing>}], which is not a zone this system has"
    fi

    got_km="$(printf '%s' "$drive_out" | sed -n 's/.*keymap=\([^ ]*\).*/\1/p' | head -1)"
    is "…and the layout the dropdown was set to, not some default" "de" "$got_km"
fi

# ═════════════════════════════════════════════════════════════════════════════
section "6b. the resume path — the branch that stops the restart repeating"
# ═════════════════════════════════════════════════════════════════════════════
# After a restart the GUI is a NEW process: self.answers is empty and only
# XKB_DEFAULT_* and APEX_INSTALLER_RESUMED crossed. Two things depend on that
# and neither had a test.
#
# The first is a silent data loss. The time zone is not an environment variable,
# so nothing carried it; the dropdown reset to whatever the live ISO resolved,
# and the resume note tells the user to check the KEYBOARD, so nobody looks at
# the time zone again and the wrong one reaches the installed system. A choice
# that does not survive is worse than one never offered.
#
# The second is the loop guard. On resume the chosen layout EQUALS the active
# one, and that equality is the only thing that makes `nxt` continue to wifi
# instead of asking for another restart. Delete it and the GUI exits 75 forever;
# the session's "already in force" check then exits 0 and the launcher reports a
# clean session end — so the installer simply quits after the keyboard page,
# with every log line looking normal.

if [ "$have_gtk" -ne 1 ] || ! command -v cage >/dev/null 2>&1; then
    skp "a resumed session restores the time zone and continues" "needs cage + python3-gi"
else
    cat > "$W/drive-resume.py" <<'PY2'
import importlib.util, os, sys, gi
gi.require_version('Gtk', '4.0'); gi.require_version('Adw', '1')
from gi.repository import GLib

spec = importlib.util.spec_from_loader(
    "apexgui", importlib.machinery.SourceFileLoader("apexgui", sys.argv[1]))
mod = importlib.util.module_from_spec(spec)
mod.__name__ = "apexgui"
spec.loader.exec_module(mod)

app = mod.Installer()

def drive():
    try:
        codes = [c for c, _ in mod.xkb_layouts()]
        sel = app.kb_drop.get_selected()
        picked = codes[sel] if 0 <= sel < len(codes) else "?"
        tsel = app.tz_drop.get_selected()
        tz = app._tz_names[tsel] if 0 <= tsel < len(app._tz_names) else "?"
        print("RESUME preselect layout=%s timezone=%s" % (picked, tz), flush=True)
        page = app.stack.get_visible_child()
        btns = []
        def walk(w):
            c = w.get_first_child()
            while c is not None:
                if isinstance(c, mod.Gtk.Button):
                    btns.append(c)
                walk(c); c = c.get_next_sibling()
        walk(page)
        go = [b for b in btns if b.has_css_class("apex-go")]
        if not go:
            print("RESUME no-primary-button", flush=True); app.quit(); return False
        go[0].emit("clicked")
        print("RESUME after-click page=%s exit_code=%s timezone=%s" % (
            app.stack.get_visible_child_name(), app.exit_code,
            app.answers.get("timezone")), flush=True)
    except Exception as e:
        print("RESUME error %s" % e, flush=True)
    app.quit()
    return False

app.connect("activate", lambda _a: GLib.timeout_add(900, drive))
rc = app.run([])
sys.exit(app.exit_code if app.exit_code is not None else rc)
PY2

    # The state file the pre-restart process would have left behind. Seeded
    # deterministically with a zone that is NOT this machine's, so a reset to the
    # host default is visible.
    printf 'layout=de\nvariant=\ntimezone=Europe/Berlin\n' > "$W/resume-state"

    r_out="$(env WLR_BACKENDS=headless WLR_RENDERER=pixman GSK_RENDERER=cairo \
        GDK_BACKEND=wayland LIBGL_ALWAYS_SOFTWARE=1 \
        APEX_INSTALLER_RESUMED=1 APEX_INSTALLER_STATE="$W/resume-state" \
        XKB_DEFAULT_LAYOUT=de \
        timeout 90 cage -- python3 "$W/drive-resume.py" "$GUI" 2>&1)"
    r_rc=$?

    if printf '%s' "$r_out" | grep -q 'RESUME preselect'; then
        ok "a resumed session rebuilds the keyboard page"
    else
        bad "a resumed session rebuilds the keyboard page" \
            "$(printf '%s' "$r_out" | grep -E '^RESUME' | head -2 | tr '\n' ' ')"
    fi

    pre_l="$(printf '%s' "$r_out" | sed -n 's/.*RESUME preselect layout=\([^ ]*\).*/\1/p' | head -1)"
    pre_tz="$(printf '%s' "$r_out" | sed -n 's/.*RESUME preselect .*timezone=\([^ ]*\).*/\1/p' | head -1)"
    is "…with the layout that is now in force preselected" "de" "$pre_l"
    # THE DATA-LOSS ASSERTION. Nothing in the environment carries this; it comes
    # back only because nxt wrote it to the state file and the page reads it.
    is "…and the time zone the user picked BEFORE the restart, not the ISO's" \
       "Europe/Berlin" "$pre_tz"

    post_page="$(printf '%s' "$r_out" | sed -n 's/.*after-click page=\([^ ]*\).*/\1/p' | head -1)"
    post_ec="$(printf '%s' "$r_out" | sed -n 's/.*after-click.*exit_code=\([^ ]*\).*/\1/p' | head -1)"
    post_tz="$(printf '%s' "$r_out" | sed -n 's/.*after-click.*timezone=\([^ ]*\).*/\1/p' | head -1)"

    # THE LOOP GUARD. Continuing to wifi, rather than asking for another restart,
    # is the whole reason the restart terminates.
    is "pressing Continue on a resumed session moves ON to wifi" "wifi" "$post_page"
    is "…and does NOT ask for another restart" "None" "$post_ec"
    is "…carrying the pre-restart time zone into the answers" "Europe/Berlin" "$post_tz"

    # ── the ROUND TRIP, on the file the GUI itself wrote ────────────────────
    # Everything above resumes from a hand-seeded file, which measures the READ
    # half only. A mutant that stopped `nxt` writing `timezone=` survived all of
    # it — the write half is asserted in §5, but nothing joined the two, and a
    # feature that writes one spelling and reads another would pass both halves
    # separately. $W/gui-state is what the real GUI produced in §6; resume from
    # exactly that.
    if [ -s "$W/gui-state" ]; then
        rt_out="$(env WLR_BACKENDS=headless WLR_RENDERER=pixman GSK_RENDERER=cairo \
            GDK_BACKEND=wayland LIBGL_ALWAYS_SOFTWARE=1 \
            APEX_INSTALLER_RESUMED=1 APEX_INSTALLER_STATE="$W/gui-state" \
            XKB_DEFAULT_LAYOUT=de \
            timeout 90 cage -- python3 "$W/drive-resume.py" "$GUI" 2>&1)"
        rt_tz="$(printf '%s' "$rt_out" | sed -n 's/.*RESUME preselect .*timezone=\([^ ]*\).*/\1/p' | head -1)"
        want_tz="$(sed -n 's/^timezone=//p' "$W/gui-state" | head -1)"
        if [ -n "$want_tz" ] && [ "$rt_tz" = "$want_tz" ]; then
            ok "round trip: the zone the GUI WROTE in §6 is the zone it READS back ($want_tz)"
        else
            bad "round trip: the zone the GUI WROTE in §6 is the zone it READS back" \
                "wrote [${want_tz:-<nothing>}] read [${rt_tz:-<nothing>}]"
        fi
    else
        bad "round trip: the zone the GUI WROTE in §6 is the zone it READS back" \
            "§6 left no state file to resume from"
    fi
fi

finish
