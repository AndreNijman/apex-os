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

# The Hyprland rule migration, extracted so the assertion below drives the real
# sed rather than a copy of it that can drift.
sed -n '/^# Hyprland 0\.54+ removed syntax/,/^done$/p' "$SRC" > "${WORK}/hypr-mig-block.sh"
grep -q 'suppress_event maximize' "${WORK}/hypr-mig-block.sh" \
    || { printf 'could not extract the Hyprland migration block\n' >&2; exit 1; }

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
        # The real script derives this from VARIANT_ID before reaching the
        # labwc block (chartreuse on Daily, gold on Gaming). The block only
        # consumes it, so the harness supplies the Daily value.
        APEX_ACCENT="#D9F99D"
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
    # The keybinds must not hardcode the shell's install path: `apex shell`
    # exists so a renamed IPC target is fixed once in the CLI rather than in
    # every seeded config on every machine.
    grep -q 'command="apex shell' "${h}/.config/labwc/rc.xml" \
        && ok "keybinds go through apex shell" || bad "keybinds go through apex shell"
    ! grep -q 'command="qs -p' "${h}/.config/labwc/rc.xml" \
        && ok "no raw qs invocation in keybinds" || bad "no raw qs invocation in keybinds"
    grep -q '<layout>icon:iconify,max,close</layout>' "${h}/.config/labwc/rc.xml" \
        && grep -q '<maximizedDecoration>titlebar</maximizedDecoration>' "${h}/.config/labwc/rc.xml" \
        && ok "native window controls remain available" \
        || bad "native window controls remain available"
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
    # Grep for CONFIG diagnostics specifically, not any error. labwc will also
    # fail to open a backend here (no seat in CI, and X11 fallback complains in a
    # Wayland session), which is expected and irrelevant: config parsing happens
    # first and is the only thing under test.
    parse_out="$(timeout 6 env -u HYPRLAND_INSTANCE_SIGNATURE \
                     labwc -C "$d" 2>&1 \
                 | grep -iE 'invalid argument for action|invalid action|unexpected element' \
                 || true)"
    if [ -z "$parse_out" ]; then
        ok "labwc parses rc.xml with no errors"
    else
        printf '%s\n' "$parse_out" | head -5
        bad "labwc parses rc.xml with no errors"
    fi
else
    printf 'SKIP  labwc unavailable; cannot validate action names\n'
fi

# ─────────────────────────────────────────────────────────────────────────────
#  labwc window chrome (APEX Floating)
#
#  themerc-override is the first-boot default; matugen overwrites this exact
#  path from the live palette once the user changes wallpaper. labwc IGNORES an
#  unrecognised theme key in silence, so a typo here does not fail a session, it
#  just quietly leaves that element on the built-in grey — which is exactly the
#  Openbox-fallback look the Floating pass exists to remove. Hence a key-name
#  check rather than a parse check.
# ─────────────────────────────────────────────────────────────────────────────
# ─────────────────────────────────────────────────────────────────────────────
#  labwc input settings
#
#  labwc ignores an unrecognised element in SILENCE — no warning, no error, the
#  session starts fine and the setting simply does not exist. That is how
#  `<tapToClick>yes</tapToClick>` shipped: not a labwc element (it is `<tap>`),
#  so the line did nothing, and it looked correct because tap-to-click is on by
#  default anyway.
#
#  So the check is against the NAMES labwc actually implements, taken from the
#  package's own exhaustive reference rather than from memory.
# ─────────────────────────────────────────────────────────────────────────────
# ─────────────────────────────────────────────────────────────────────────────
#  Hyprland window-rule migration
#
#  The migration and the shipped template must agree, because they configure the
#  same compositor. They did not, twice, in opposite directions: the template was
#  once "corrected" to Hyprland 0.51.1 syntax after checking the PUBLISHED core
#  image, which was stale — Containerfile.core builds 0.56.2, where only the
#  `match:` forms parse.
#
#  So this asserts the migration's OUTPUT equals what the template ships. That is
#  the invariant; which syntax is currently right is the Containerfile's
#  `Hyprland --verify-config` assertion to decide.
# ─────────────────────────────────────────────────────────────────────────────
section "Hyprland rule migration agrees with the template"

HYPR_TMPL="${ROOT}/files/desktop/hypr/hyprland.conf"
if [ ! -f "$HYPR_TMPL" ]; then
    bad "the Hyprland template is present"
else
    mig="${WORK}/mig.conf"
    # The pre-0.54 spellings an upgrading user would still have on disk.
    {
        printf 'windowrule = suppressevent maximize, class:.*\n'
        printf 'windowrule = nofocus, class:^$, title:^$, xwayland:1, floating:1, fullscreen:0, pinned:0\n'
    } > "$mig"

    # Run the real block against it, not a copy of the sed.
    HOME="${WORK}/mighome" bash -c '
        set -euo pipefail
        log() { :; }
        KB_LAYOUT=us; KB_VARIANT=; APEX_ACCENT="#D9F99D"
        render_hypr_tmpl() { cat "$1"; }
        HYPR_CONF="$2"
        mkdir -p "$(dirname "$2")"
        source "$1"
    ' -- "${WORK}/hypr-mig-block.sh" "$mig" >/dev/null 2>&1 || true

    for want in 'suppress_event maximize, match:class' 'no_focus on'; do
        grep -qF "$want" "$mig" \
            && ok "migration produces: ${want}" || bad "migration produces: ${want}"
    done
    # And the produced spelling is the one the template actually ships.
    for want in 'suppress_event maximize, match:class' 'no_focus on'; do
        grep -qF "$want" "$HYPR_TMPL" \
            && ok "the template ships the same: ${want}" \
            || bad "the template ships the same: ${want}"
    done
    # Nothing may still carry the pre-0.54 spelling after migrating.
    grep -qE 'suppressevent|nofocus,' "$mig" \
        && bad "no pre-0.54 spelling survives migration" \
        || ok "no pre-0.54 spelling survives migration"
fi

section "labwc input settings"

# labwc ships rc.xml.all as its complete annotated reference. Preferring it over
# a hardcoded list means the check tracks the installed labwc rather than
# whatever was true when this test was written.
LABWC_REF=""
for cand in /usr/share/doc/labwc/rc.xml.all /usr/share/doc/labwc-*/rc.xml.all; do
    [ -f "$cand" ] && { LABWC_REF="$cand"; break; }
done

if [ -z "$LABWC_REF" ]; then
    printf 'SKIP  labwc rc.xml.all unavailable; cannot check libinput element names\n'
else
    ok "labwc's own element reference is available"

    # Every element name labwc documents inside <libinput>, commented or not.
    sed -n '/<libinput>/,/<\/libinput>/p' "$LABWC_REF" \
        | grep -oE '<[a-zA-Z]+>' | tr -d '<>' | sort -u > "${WORK}/labwc-input-known"

    # Every element name the shipped config actually uses.
    sed -n '/<libinput>/,/<\/libinput>/p' "${TMPL}/rc.xml" \
        | grep -vE '^\s*<!--' \
        | grep -oE '<[a-zA-Z]+>' | tr -d '<>' | sort -u > "${WORK}/labwc-input-used"

    unknown=""
    while IFS= read -r el; do
        case "$el" in libinput|device) continue ;; esac
        grep -qxF "$el" "${WORK}/labwc-input-known" || unknown="${unknown} ${el}"
    done < "${WORK}/labwc-input-used"

    if [ -z "$unknown" ]; then
        ok "every <libinput> element is one labwc implements"
    else
        printf '  not implemented by labwc:%s\n' "$unknown"
        printf '  (labwc ignores these silently, so the setting does nothing)\n'
        bad "every <libinput> element is one labwc implements"
    fi

    # The specific regression, named, so it cannot come back quietly.
    #
    # Non-comment lines only: the comment above the block explains what went
    # wrong and therefore contains the string "<tapToClick>". Grepping the whole
    # file made the documentation trip the check on the fixed config.
    live_libinput() {
        sed -n '/<libinput>/,/<\/libinput>/p' "${TMPL}/rc.xml" | grep -vE '^\s*<!--|^\s*[a-zA-Z]'
    }
    live_libinput | grep -q '<tapToClick>' \
        && bad "tap-to-click uses labwc's own element name" \
        || ok "tap-to-click uses labwc's own element name"
    live_libinput | grep -q '<tap>yes</tap>' \
        && ok "tap-to-click is enabled with <tap>" \
        || bad "tap-to-click is enabled with <tap>"
fi

section "labwc window chrome"
THEMERC="${TMPL}/themerc-override"
if [ ! -f "$THEMERC" ]; then
    bad "themerc-override is shipped"
else
    ok "themerc-override is shipped"

    # The accent placeholder must be present in the template and gone after
    # seeding: an unsubstituted @ACCENT@ is not a colour, and labwc drops the
    # line, leaving the active border on the default.
    grep -q '@ACCENT@' "$THEMERC" \
        && ok "themerc-override carries the @ACCENT@ placeholder" \
        || bad "themerc-override carries the @ACCENT@ placeholder"

    # The seeding half only has an answer where the block actually ran. The
    # seeding block is guarded on labwc being installed, so on a runner without
    # it there is no seeded file to inspect and asserting one would be checking
    # the guard, not the behaviour.
    if ! command -v labwc >/dev/null 2>&1 && [ ! -x /usr/bin/labwc ]; then
        printf 'SKIP  labwc not installed; nothing was seeded to inspect\n'
    elif [ -f "${h}/.config/labwc/themerc-override" ]; then
        ok "themerc-override is seeded into the user config"
        grep -q '@ACCENT@' "${h}/.config/labwc/themerc-override" \
            && bad "seeded themerc-override has no unsubstituted placeholder" \
            || ok "seeded themerc-override has no unsubstituted placeholder"
        grep -qE '^window\.active\.border\.color: #[0-9A-Fa-f]{6}$' \
             "${h}/.config/labwc/themerc-override" \
            && ok "the seeded accent is a real hex colour" \
            || bad "the seeded accent is a real hex colour"
    else
        bad "themerc-override is seeded into the user config"
    fi

    # Every key must be one labwc actually knows. The man page is the only
    # authority; without it this is unverifiable and skipping is honest.
    if man 5 labwc-theme >/dev/null 2>&1; then
        man 5 labwc-theme 2>/dev/null | col -b \
            | grep -oE "^ {3,7}[a-z][a-z0-9.*-]+" | tr -d ' ' | sort -u \
            > "${WORK}/labwc-theme-keys"
        unknown=""
        while IFS= read -r key; do
            grep -qxF "$key" "${WORK}/labwc-theme-keys" || unknown="${unknown} ${key}"
        done <<EOF
$(grep -vE '^\s*#|^\s*$' "$THEMERC" | sed 's/:.*//' | tr -d ' ' | sort -u)
EOF
        if [ -z "$unknown" ]; then
            ok "every themerc-override key is one labwc documents"
        else
            printf '  unknown keys:%s\n' "$unknown"
            bad "every themerc-override key is one labwc documents"
        fi
    else
        printf 'SKIP  labwc-theme(5) unavailable; cannot validate theme key names\n'
    fi

    # Geometry has to stay in step with rc.xml: labwc derives titlebar height
    # from the font, so the 36-40 px target only holds for this pairing.
    grep -q '<name>Noto Sans</name>' "${TMPL}/rc.xml" \
        && ok "rc.xml uses a proportional face for window chrome" \
        || bad "rc.xml uses a proportional face for window chrome"
    grep -qE '<cornerRadius>1[0-2]</cornerRadius>' "${TMPL}/rc.xml" \
        && ok "rc.xml sets a 10-12 px corner radius" \
        || bad "rc.xml sets a 10-12 px corner radius"
    grep -qE '^window\.titlebar\.padding\.height: 11$' "$THEMERC" \
        && ok "titlebar padding matches the 36-40 px target" \
        || bad "titlebar padding matches the 36-40 px target"
    # Both dimensions, and the hover radius that has to be half of them for the
    # highlight to be a circle. An `(width|height)` alternation here would pass
    # with one of the two wrong.
    if grep -qE '^window\.button\.width: 30$' "$THEMERC" \
       && grep -qE '^window\.button\.height: 30$' "$THEMERC" \
       && grep -qE '^window\.button\.hover\.bg\.corner-radius: 15$' "$THEMERC"; then
        ok "window buttons are 30x30 with a circular hover"
    else
        bad "window buttons are 30x30 with a circular hover"
    fi
fi

# ─────────────────────────────────────────────────────────────────────────────
#  labwc keybinds vs the shell's own defaults
#
#  Maintained by hand (labwc has no IPC to push bindings over and no include
#  mechanism), so they can drift silently. Only runnable where a shell tree is
#  available; the image build runs the same script against the vendored copy.
# ─────────────────────────────────────────────────────────────────────────────
section "labwc keybinds match the shell defaults"
CHECK="${ROOT}/files/scripts/check-labwc-keybinds"
SHELL_TREE=""
# The CHECKOUT first, then the installed copy. The image build calls the checker
# with an explicit /usr/share/apex-shell path (Containerfile.base), so this
# ordering only affects a local run — and there, the vendored copy on the machine
# is whatever the last image shipped, which lags the tree being tested. Checking
# a change against a stale source of truth fails for a reason that has nothing
# to do with the change.
for cand in "${ROOT}/../apex-shell" /usr/share/apex-shell; do
    [ -f "${cand}/src/services/config_tab/KeybindService.qml" ] && { SHELL_TREE="$cand"; break; }
done

if [ -z "$SHELL_TREE" ]; then
    printf 'SKIP  no apex-shell tree available to compare against\n'
elif ! command -v python3 >/dev/null 2>&1; then
    printf 'SKIP  python3 unavailable\n'
else
    if python3 "$CHECK" "$SHELL_TREE" "${TMPL}/rc.xml" >/dev/null 2>&1; then
        ok "every shell popup bind matches KeybindService"
    else
        python3 "$CHECK" "$SHELL_TREE" "${TMPL}/rc.xml" 2>&1 | head -12
        bad "every shell popup bind matches KeybindService"
    fi
fi

printf '\napex-shell-firstrun: %d passed, %d failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ]
