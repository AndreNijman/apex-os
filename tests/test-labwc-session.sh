#!/usr/bin/env bash
#
# APEX-OS — labwc (APEX Floating) session capability matrix.
#
# The roadmap's labwc test matrix, for the part of it a machine can answer.
# Everything here runs a REAL client against a REAL nested labwc using the
# SHIPPED config, because the question is never "does labwc support this
# protocol" in the abstract — it is "does the session we ship actually let a
# client do this". A protocol that is advertised but unusable looks identical to
# one that works, right up until a user tries it.
#
# The application grid from the same section (Firefox, Chromium, Steam,
# gamescope, VS Code, JetBrains, LibreOffice, Blender, Discord, Wine, XWayland
# games) is NOT here and is not claimed. Those need a real GPU session and a
# human looking at the screen; they live in docs/labwc-verification.md as a
# manual checklist. Fabricating them would be worse than leaving them out.
#
# Skips rather than fails when labwc or a probe client is missing, so this is
# safe to run in CI where there is no compositor at all.
#
# Usage: tests/test-labwc-session.sh

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMPL="${ROOT}/files/desktop/labwc"
WORK="$(mktemp -d)"
trap 'rm -rf "${WORK}"' EXIT

pass=0
fail=0
skip=0

ok()   { printf 'PASS  %s\n' "$1"; pass=$((pass + 1)); }
bad()  { printf 'FAIL  %s\n' "$1"; fail=$((fail + 1)); }
skp()  { printf 'SKIP  %s\n' "$1"; skip=$((skip + 1)); }
section() { printf '\n\033[1m── %s ──\033[0m\n' "$1"; }

# ── preconditions ────────────────────────────────────────────────────────────
section "environment"

if ! command -v labwc >/dev/null 2>&1; then
    skp "labwc is not installed; the whole session matrix is unrunnable here"
    printf '\nlabwc-session: %d passed, %d failed, %d skipped\n' "$pass" "$fail" "$skip"
    exit 0
fi
ok "labwc is installed ($(labwc --version 2>&1 | head -1))"

# A nested compositor needs a parent display. Without one there is no session to
# probe and pretending otherwise would report green on nothing.
if [ -z "${WAYLAND_DISPLAY:-}" ] && [ -z "${DISPLAY:-}" ]; then
    skp "no parent display; labwc cannot start nested"
    printf '\nlabwc-session: %d passed, %d failed, %d skipped\n' "$pass" "$fail" "$skip"
    exit 0
fi
ok "a parent display is available to nest inside"

# ── the config under test ────────────────────────────────────────────────────
# The shipped templates, rendered exactly as the provisioner renders them, so
# this tests what users get rather than a convenient variant of it.
CFG="${WORK}/.config"
mkdir -p "${CFG}/labwc"
cp "${TMPL}/rc.xml" "${TMPL}/menu.xml" "${CFG}/labwc/"
sed 's/@ACCENT@/#D9F99D/g' "${TMPL}/themerc-override" > "${CFG}/labwc/themerc-override"

# Run a probe script inside a nested labwc and return its output.
run_nested() {
    local script="$1" seconds="${2:-25}"
    chmod +x "$script"
    XDG_CONFIG_HOME="${CFG}" timeout "$seconds" labwc --startup "$script" 2>/dev/null
}

have() { command -v "$1" >/dev/null 2>&1; }

# ── capability probes ────────────────────────────────────────────────────────
section "session capabilities"

cat > "${WORK}/probe.sh" <<PROBE
#!/bin/bash
out="${WORK}/results"
: > "\$out"
say() { printf '%s=%s\n' "\$1" "\$2" >> "\$out"; }

# wlr-output-management. The display-settings page is built on this, so if it
# does not answer here, monitor arrangement cannot work in this session.
if wlr-randr >/dev/null 2>&1; then say output_management ok; else say output_management fail; fi

# wlr-screencopy: screenshots and the screen recorder.
if grim "${WORK}/shot.png" >/dev/null 2>&1 && [ -s "${WORK}/shot.png" ]; then
    say screencopy ok
else
    say screencopy fail
fi

# Clipboard and primary selection are separate mechanisms; a compositor can
# serve one and not the other, and middle-click paste is the one people notice.
if echo apex-clipboard | wl-copy 2>/dev/null && [ "\$(wl-paste 2>/dev/null)" = apex-clipboard ]; then
    say clipboard ok
else
    say clipboard fail
fi
if echo apex-primary | wl-copy --primary 2>/dev/null && [ "\$(wl-paste --primary 2>/dev/null)" = apex-primary ]; then
    say primary ok
else
    say primary fail
fi

# wlr-layer-shell. APEX Shell is a layer-shell client, so this is the single
# protocol the whole desktop depends on; swaybg is the smallest client that
# exercises it.
if command -v swaybg >/dev/null 2>&1; then
    timeout 3 swaybg -c '#112233' 2>"${WORK}/bg.err"
    if grep -qiE 'layer.shell|not supported|unsupported' "${WORK}/bg.err" 2>/dev/null; then
        say layer_shell fail
    else
        say layer_shell ok
    fi
else
    say layer_shell skip
fi

# ext-session-lock: the lock screen path.
if command -v swaylock >/dev/null 2>&1; then
    timeout 3 swaylock -f -c 000000 2>"${WORK}/lock.err"
    if grep -qiE 'does not support|unsupported|ext-session-lock' "${WORK}/lock.err" 2>/dev/null; then
        say session_lock fail
    else
        say session_lock ok
    fi
else
    say session_lock skip
fi

# ext-idle-notify: idle handling without a compositor-specific daemon.
if command -v swayidle >/dev/null 2>&1; then
    timeout 3 swayidle -w timeout 1 true 2>"${WORK}/idle.err"
    if grep -qiE 'not supported|unsupported' "${WORK}/idle.err" 2>/dev/null; then
        say idle fail
    else
        say idle ok
    fi
else
    say idle skip
fi
PROBE

run_nested "${WORK}/probe.sh" 40 >/dev/null 2>&1

report() {
    local key="$1" label="$2"
    local v
    v="$(sed -n "s/^${key}=//p" "${WORK}/results" 2>/dev/null | head -1)"
    case "${v:-missing}" in
        ok)   ok "$label" ;;
        skip) skp "$label (probe client not installed)" ;;
        fail) bad "$label" ;;
        *)    bad "$label (probe produced no result)" ;;
    esac
}

if [ -s "${WORK}/results" ]; then
    report output_management "wlr-output-management answers (display settings depend on it)"
    report screencopy        "screencopy works (screenshots, recording)"
    report clipboard         "clipboard round-trips"
    report primary           "primary selection round-trips"
    report layer_shell       "layer-shell works (APEX Shell depends on it)"
    report session_lock      "ext-session-lock works (lock screen)"
    report idle              "ext-idle-notify works"
else
    bad "the nested session produced no probe results at all"
fi

# ── config reload and recovery ───────────────────────────────────────────────
# The matrix asks for both. Recovery is the one that matters: labwc falls back
# to defaults SILENTLY on a broken config, which means a bad edit costs the user
# their keybinds and their shell with no error anywhere.
section "config reload and invalid-config recovery"

cat > "${WORK}/reload.sh" <<RELOAD
#!/bin/bash
out="${WORK}/reload-results"
: > "\$out"

# A valid reconfigure must be accepted.
if labwc --reconfigure >/dev/null 2>&1; then echo "reload=ok" >> "\$out"; else echo "reload=fail" >> "\$out"; fi
sleep 1

# Now corrupt the config and reconfigure again. The compositor must SURVIVE:
# an invalid file may not take the session down with it.
printf '<?xml version="1.0"?>\n<labwc_config><core><gap>8</gap>\n' > "${CFG}/labwc/rc.xml"
labwc --reconfigure >/dev/null 2>&1
sleep 1

# Still alive? Ask it to do something that needs a live compositor.
if wlr-randr >/dev/null 2>&1; then echo "survives_bad_config=ok" >> "\$out"; else echo "survives_bad_config=fail" >> "\$out"; fi
RELOAD

run_nested "${WORK}/reload.sh" 30 >/dev/null 2>&1

if [ -s "${WORK}/reload-results" ]; then
    grep -q '^reload=ok' "${WORK}/reload-results" \
        && ok "labwc --reconfigure is accepted" || bad "labwc --reconfigure is accepted"
    grep -q '^survives_bad_config=ok' "${WORK}/reload-results" \
        && ok "the session survives an invalid rc.xml" || bad "the session survives an invalid rc.xml"
else
    bad "the reload probe produced no results"
fi

# ── portal configuration ─────────────────────────────────────────────────────
# Static, but it belongs with the session matrix: screen sharing is the most
# common thing to be quietly broken, and the failure is invisible until someone
# joins a call.
section "portal backends"

PORTAL="${ROOT}/files/system/xdg-desktop-portal/labwc-portals.conf"
if [ ! -f "$PORTAL" ]; then
    bad "a labwc portal config is shipped"
else
    ok "a labwc portal config is shipped"
    grep -qE '^default=gtk$' "$PORTAL" \
        && ok "desktop interfaces are pinned to gtk" || bad "desktop interfaces are pinned to gtk"
    grep -qE '^org\.freedesktop\.impl\.portal\.ScreenCast=wlr$' "$PORTAL" \
        && ok "ScreenCast is pinned to the wlroots backend" || bad "ScreenCast is pinned to the wlroots backend"
    grep -qE '^org\.freedesktop\.impl\.portal\.Screenshot=wlr$' "$PORTAL" \
        && ok "Screenshot is pinned to the wlroots backend" || bad "Screenshot is pinned to the wlroots backend"
    # The packaged labwc config uses `default=wlr;*`, which leaves FileChooser
    # to whichever backend resolves first. Reintroducing that is the regression
    # this guards.
    grep -qE '^\s*default=.*\*' "$PORTAL" \
        && bad "no wildcard backend remains" || ok "no wildcard backend remains"

    if [ -d /usr/share/xdg-desktop-portal/portals ]; then
        [ -f /usr/share/xdg-desktop-portal/portals/wlr.portal ] \
            && ok "the wlroots portal backend is installed" || bad "the wlroots portal backend is installed"
        [ -f /usr/share/xdg-desktop-portal/portals/gtk.portal ] \
            && ok "the gtk portal backend is installed" || bad "the gtk portal backend is installed"
    else
        skp "no portal backends installed here; cannot check the config names real ones"
    fi
fi

# ── what this cannot answer ──────────────────────────────────────────────────
section "not covered here"
cat <<'NOTE'
      Gamma control (night light) cannot be probed nested: a nested backend has
      no gamma-capable output, so gammastep reports "Zero outputs support gamma
      adjustment" regardless of whether labwc implements the protocol. Needs a
      real session.

      Multi-monitor hotplug, scale, refresh and rotation need real outputs.

      Fullscreen/maximise/minimise behaviour, workspace switching and window
      focus need real windows and a human.

      The application grid (Firefox, Chromium, Steam, gamescope, VS Code,
      JetBrains, LibreOffice, Blender, Discord, Qt/GTK/Electron, Wine, XWayland
      games) is a manual checklist in docs/labwc-verification.md.
NOTE

printf '\nlabwc-session: %d passed, %d failed, %d skipped\n' "$pass" "$fail" "$skip"
[ "$fail" -eq 0 ]
