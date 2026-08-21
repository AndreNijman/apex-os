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

# The blocks log through the script's own helper and read HOME/HYPR_CONF, so the
# harness supplies exactly those and nothing else.
run_zshrc()   { HOME="$1" bash -c 'set -euo pipefail; log() { :; }; source "$1"' -- "${WORK}/zshrc-block.sh"; }

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

# ─────────────────────────────────────────────────────────────────────────────
#  labwc config seeding
#
#  labwc has no IPC, so everything the compositor does for the shell is declared
#  in these files up front. A seeding bug is therefore not a cosmetic problem: it
#  is the difference between a working session and a bare grey screen with no way
#  to discover why. And because the files belong to the user afterwards, the
#  interesting property is again that re-running leaves their edits alone.
# ─────────────────────────────────────────────────────────────────────────────
extract '# ── 6b\. labwc config' "${WORK}/labwc-block.sh" 'apex-shell-autostart'

# The block hardcodes the INSTALLED template directory, which does not exist in a
# checkout. Redirect that single path at the repo copies so the real logic runs
# against the real templates. (That the install path itself exists is asserted at
# image build time in Containerfile.base, which is where it belongs.)
TMPL="${ROOT}/files/desktop/labwc"
sed -i "s|LABWC_TMPL_DIR=/usr/share/apex/labwc|LABWC_TMPL_DIR=${TMPL}|" "${WORK}/labwc-block.sh"
grep -q "LABWC_TMPL_DIR=${TMPL}" "${WORK}/labwc-block.sh" \
    || { printf 'could not redirect LABWC_TMPL_DIR in the extracted block\n' >&2; exit 1; }
run_labwc() {
    # Note the two different $1s: the outer one is this function's argument (the
    # throwaway HOME), the inner one is the bash -c script's first positional
    # argument (the extracted block). They do not collide because they are in
    # different scopes.
    HOME="$1" bash -c '
        set -euo pipefail
        log() { :; }
        KB_LAYOUT=us
        KB_VARIANT=
        render_hypr_tmpl() {
            sed -e "s|@HOME@|${HOME}|g" \
                -e "s|@KB_LAYOUT@|${KB_LAYOUT}|g" \
                -e "s|@KB_VARIANT@|${KB_VARIANT}|g" "$1"
        }
        # `--` is $0, so the block path is $1. LABWC_TMPL_DIR is set by the
        # block itself, redirected at extraction time.
        source "$1"
    ' -- "${WORK}/labwc-block.sh"
}

section "labwc config seeding"

if ! command -v labwc >/dev/null 2>&1 && [ ! -x /usr/bin/labwc ]; then
    printf 'SKIP  labwc not installed; seeding block is guarded on it\n'
else
    h="${WORK}/labwc-fresh"; mkdir -p "$h"
    run_labwc "$h"
    for f in rc.xml menu.xml autostart environment; do
        [ -s "${h}/.config/labwc/${f}" ] \
            && ok "seeded ${f}" || bad "seeded ${f}"
    done
    grep -q 'apex-shell-autostart' "${h}/.config/labwc/autostart" \
        && ok "autostart starts APEX Shell" || bad "autostart starts APEX Shell"
    # A session that cannot be detected by the shell is a session with a bar that
    # thinks it is on an unknown compositor.
    grep -q '^XDG_CURRENT_DESKTOP=labwc' "${h}/.config/labwc/environment" \
        && ok "environment identifies the compositor" \
        || bad "environment identifies the compositor"
    # Placeholders must be substituted, not shipped literally.
    ! grep -q '@HOME@\|@KB_LAYOUT@' "${h}/.config/labwc/autostart" "${h}/.config/labwc/environment" \
        && ok "no placeholders survive substitution" || bad "no placeholders survive substitution"
    [ -x "${h}/.config/labwc/autostart" ] \
        && ok "autostart is executable" || bad "autostart is executable"

    # Idempotence: a second login must not duplicate the autostart block.
    run_labwc "$h"
    [ "$(grep -c 'apex-shell-autostart' "${h}/.config/labwc/autostart")" = 1 ] \
        && ok "re-running does not duplicate autostarts" \
        || bad "re-running does not duplicate autostarts"

    # A user's own edits must survive.
    h="${WORK}/labwc-edited"; mkdir -p "${h}/.config/labwc"
    printf '%s\n' '# my own config' > "${h}/.config/labwc/rc.xml"
    cp "${h}/.config/labwc/rc.xml" "${WORK}/labwc-rc.orig"
    run_labwc "$h"
    cmp -s "${h}/.config/labwc/rc.xml" "${WORK}/labwc-rc.orig" \
        && ok "a hand-written rc.xml is left alone" || bad "a hand-written rc.xml is left alone"

    # An autostart predating APEX Shell gets repaired rather than replaced.
    h="${WORK}/labwc-legacy"; mkdir -p "${h}/.config/labwc"
    printf '%s\n' '# pre-existing' 'xterm &' > "${h}/.config/labwc/autostart"
    run_labwc "$h"
    grep -q 'apex-shell-autostart' "${h}/.config/labwc/autostart" \
        && ok "a legacy autostart gains the shell" || bad "a legacy autostart gains the shell"
    grep -qxF 'xterm &' "${h}/.config/labwc/autostart" \
        && ok "the user's own autostart lines survive" || bad "the user's own autostart lines survive"
fi

# The shipped XML must be well-formed: labwc has no --verify-config, and a
# malformed rc.xml makes it fall back to defaults SILENTLY — a session that
# starts with no keybinds and no shell.
section "labwc shipped config validity"
if command -v xmllint >/dev/null 2>&1; then
    xmllint --noout "${TMPL}/rc.xml" 2>/dev/null \
        && ok "rc.xml is well-formed XML" || bad "rc.xml is well-formed XML"
    xmllint --noout "${TMPL}/menu.xml" 2>/dev/null \
        && ok "menu.xml is well-formed XML" || bad "menu.xml is well-formed XML"
else
    printf 'SKIP  xmllint unavailable\n'
fi

# labwc itself is the only authority on whether an action name and its arguments
# are valid; xmllint cannot know that `Focus direction=...` is not a thing.
if command -v labwc >/dev/null 2>&1; then
    d="${WORK}/labwc-parse"; mkdir -p "$d"
    cp "${TMPL}/rc.xml" "${TMPL}/menu.xml" "$d/"
    sed -e 's|@KB_LAYOUT@|us|g' -e 's|@KB_VARIANT@||g' "${TMPL}/environment" > "${d}/environment"
    parse_out="$(timeout 6 env -u HYPRLAND_INSTANCE_SIGNATURE -u WAYLAND_DISPLAY \
                     labwc -C "$d" 2>&1 | grep -iE 'error|invalid' || true)"
    if [ -z "$parse_out" ]; then
        ok "labwc parses rc.xml with no errors"
    else
        printf '%s\n' "$parse_out" | head -5
        bad "labwc parses rc.xml with no errors"
    fi
else
    printf 'SKIP  labwc unavailable; cannot validate action names\n'
fi

printf '\napex-shell-firstrun: %d passed, %d failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ]
