#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  Assertions for the self-healing blocks of /usr/libexec/apex-shell-firstrun.
#
#  That script runs at every login and edits files that belong to the user, so
#  the interesting question is never "does it write the new value" but "does it
#  leave everything else exactly as it was, every time it runs". The blocks are
#  extracted from the shipped script by their own comment markers and executed
#  verbatim against throwaway HOMEs — nothing here re-implements them, and a
#  renamed or deleted block fails the test instead of silently skipping it.
#
#  Needs neither root nor network. Run from the repository root:
#      ./tests/test-apex-firstrun.sh
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SRC="${ROOT}/files/system/libexec/apex-shell-firstrun"
[ -f "$SRC" ] || { printf 'missing %s\n' "$SRC" >&2; exit 1; }

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

pass=0
fail=0
ok()  { printf 'PASS  %s\n' "$1"; pass=$((pass + 1)); }
bad() { printf 'FAIL  %s\n' "$1"; fail=$((fail + 1)); }
section() { printf '\n── %s ──\n' "$1"; }

extract() {
    local first="$1" out="$2" must="$3"
    sed -n "/^${first}/,/^fi$/p" "$SRC" > "$out"
    grep -q "$must" "$out" \
        || { printf 'could not extract the block starting %s\n' "$first" >&2; exit 1; }
}

extract '# Repair an already-seeded ~\/.zshrc' "${WORK}/zshrc-block.sh" ZSH_AUTOSUGGEST_HIGHLIGHT_STYLE
extract '# Touchpad tap behaviour for a hyprland.conf' "${WORK}/touchpad-block.sh" tap_button_map

# The blocks log through the script's own helper and read HOME/HYPR_CONF, so the
# harness supplies exactly those and nothing else.
run_zshrc()   { HOME="$1" bash -c 'set -euo pipefail; log() { :; }; source "$1"' -- "${WORK}/zshrc-block.sh"; }
run_touchpad() {
    HOME="$1" HYPR_CONF="$1/.config/hypr/hyprland.conf" \
        bash -c 'set -euo pipefail; log() { :; }; source "$1"' -- "${WORK}/touchpad-block.sh"
}

section "zsh autosuggestion colour"
# The bug: zle lower-cases a colour spec when it stores the highlight, and
# zsh-autosuggestions removes its highlight by exact string match, so the
# upper-case value APEX used to ship left accepted text grey.
h="${WORK}/zsh-seeded"; mkdir -p "$h"
printf '%s\n' '# user header' \
               "ZSH_AUTOSUGGEST_HIGHLIGHT_STYLE='fg=#4A5162'" \
               "alias ll='eza -l'" > "${h}/.zshrc"
chmod 0640 "${h}/.zshrc"
inode="$(stat -c %i "${h}/.zshrc")"
mode="$(stat -c %a "${h}/.zshrc")"
run_zshrc "$h"
grep -qxF "ZSH_AUTOSUGGEST_HIGHLIGHT_STYLE='fg=#4a5162'" "${h}/.zshrc" \
    && ok "the upper-case colour is rewritten" || bad "the upper-case colour is rewritten"
grep -qxF "alias ll='eza -l'" "${h}/.zshrc" \
    && ok "the user's own lines survive" || bad "the user's own lines survive"
[ "$(stat -c %i "${h}/.zshrc")" = "$inode" ] \
    && ok "the file keeps its inode" || bad "the file keeps its inode"
[ "$(stat -c %a "${h}/.zshrc")" = "$mode" ] \
    && ok "the file keeps its mode" || bad "the file keeps its mode"
[ -z "$(find "$h" -maxdepth 1 -name '.zshrc.apex.*')" ] \
    && ok "no temporary file is left behind" || bad "no temporary file is left behind"
run_zshrc "$h"
[ "$(grep -c ZSH_AUTOSUGGEST_HIGHLIGHT_STYLE "${h}/.zshrc")" = 1 ] \
    && ok "re-running changes nothing" || bad "re-running changes nothing"

h="${WORK}/zsh-custom"; mkdir -p "$h"
printf '%s\n' "ZSH_AUTOSUGGEST_HIGHLIGHT_STYLE='fg=#123456'" > "${h}/.zshrc"
cp "${h}/.zshrc" "${WORK}/zsh-custom.orig"
run_zshrc "$h"
cmp -s "${h}/.zshrc" "${WORK}/zsh-custom.orig" \
    && ok "a colour the user chose is left alone" || bad "a colour the user chose is left alone"

h="${WORK}/zsh-none"; mkdir -p "$h"
run_zshrc "$h"
[ ! -e "${h}/.zshrc" ] \
    && ok "no ~/.zshrc is invented" || bad "no ~/.zshrc is invented"

section "touchpad tap defaults"
# Appended only to a config that states no tap preference at all: in Hyprland the
# last assignment wins, so appending over a user's setting would silently
# override it.
h="${WORK}/hypr-bare"; mkdir -p "${h}/.config/hypr"
printf '%s\n' 'monitor = , preferred, auto, 1' > "${h}/.config/hypr/hyprland.conf"
run_touchpad "$h"
grep -q 'tap_button_map = lrm' "${h}/.config/hypr/hyprland.conf" \
    && ok "tap defaults are seeded when the config states none" \
    || bad "tap defaults are seeded when the config states none"
grep -q 'tap-to-click   = true' "${h}/.config/hypr/hyprland.conf" \
    && ok "tap-to-click is stated with the map" || bad "tap-to-click is stated with the map"
run_touchpad "$h"
[ "$(grep -c 'tap_button_map = lrm' "${h}/.config/hypr/hyprland.conf")" = 1 ] \
    && ok "re-running appends nothing" || bad "re-running appends nothing"

h="${WORK}/hypr-user"; mkdir -p "${h}/.config/hypr"
printf '%s\n' 'input {' '    touchpad {' '        tap-to-click = false' '    }' '}' \
    > "${h}/.config/hypr/hyprland.conf"
cp "${h}/.config/hypr/hyprland.conf" "${WORK}/hypr-user.orig"
run_touchpad "$h"
cmp -s "${h}/.config/hypr/hyprland.conf" "${WORK}/hypr-user.orig" \
    && ok "a user who disabled tapping keeps it disabled" \
    || bad "a user who disabled tapping keeps it disabled"

h="${WORK}/hypr-mapped"; mkdir -p "${h}/.config/hypr"
printf '%s\n' 'input { touchpad { tap_button_map = lmr } }' > "${h}/.config/hypr/hyprland.conf"
cp "${h}/.config/hypr/hyprland.conf" "${WORK}/hypr-mapped.orig"
run_touchpad "$h"
cmp -s "${h}/.config/hypr/hyprland.conf" "${WORK}/hypr-mapped.orig" \
    && ok "a user's own tap map is left alone" || bad "a user's own tap map is left alone"

h="${WORK}/hypr-commented"; mkdir -p "${h}/.config/hypr"
printf '%s\n' '# tap-to-click = true' > "${h}/.config/hypr/hyprland.conf"
run_touchpad "$h"
grep -q 'tap_button_map = lrm' "${h}/.config/hypr/hyprland.conf" \
    && ok "a commented-out setting does not count as a preference" \
    || bad "a commented-out setting does not count as a preference"

h="${WORK}/hypr-none"; mkdir -p "${h}/.config/hypr"
run_touchpad "$h"
[ ! -e "${h}/.config/hypr/hyprland.conf" ] \
    && ok "no hyprland.conf is invented" || bad "no hyprland.conf is invented"

printf '\napex-shell-firstrun: %d passed, %d failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ]
