# shellcheck shell=bash disable=SC2034
# (SC2034: ZSH_HIGHLIGHT_STYLES is read by the plugin, sourced later from ~/.zshrc.)
# APEX-OS — zsh-syntax-highlighting follows the wallpaper accent.
#
# Installed to /etc/profile.d, which Fedora's /etc/zshrc sources for every
# interactive zsh BEFORE ~/.zshrc. The plugin fills its styles with
# `: ${ZSH_HIGHLIGHT_STYLES[arg0]:=fg=green}`, so a value set here first wins —
# and the seeded ~/.zshrc, which belongs to the user once written, needs no
# edit for an existing account to pick this up.
#
# The colour is APEX Shell's matugen accent, which it writes on every wallpaper
# change as an SGR parameter string (38;2;R;G;B). No file, or anything that is
# not exactly that shape, leaves the plugin's own defaults untouched.
[ -n "${ZSH_VERSION:-}" ] || return 0
case $- in *i*) ;; *) return 0 ;; esac

_apex_hl_file="${XDG_CACHE_HOME:-$HOME/.cache}/apex-shell/accent-ansi"
_apex_hl=""
[ -r "$_apex_hl_file" ] && read -r _apex_hl < "$_apex_hl_file"
case "$_apex_hl" in
    38\;2\;*) ;;
    *) unset _apex_hl _apex_hl_file; return 0 ;;
esac
_apex_hl="${_apex_hl#38;2;}"
_apex_r="${_apex_hl%%;*}"; _apex_hl="${_apex_hl#*;}"
_apex_g="${_apex_hl%%;*}"; _apex_b="${_apex_hl#*;}"
case "$_apex_r$_apex_g$_apex_b" in
    ''|*[!0-9]*) unset _apex_hl _apex_hl_file _apex_r _apex_g _apex_b; return 0 ;;
esac
_apex_hex="$(printf '#%02x%02x%02x' "$_apex_r" "$_apex_g" "$_apex_b")"
typeset -gA ZSH_HIGHLIGHT_STYLES
ZSH_HIGHLIGHT_STYLES[arg0]="fg=$_apex_hex"
ZSH_HIGHLIGHT_STYLES[precommand]="fg=$_apex_hex,underline"
ZSH_HIGHLIGHT_STYLES[suffix-alias]="fg=$_apex_hex,underline"
unset _apex_hl _apex_hl_file _apex_r _apex_g _apex_b _apex_hex
