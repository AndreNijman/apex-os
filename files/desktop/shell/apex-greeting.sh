# APEX-OS — fastfetch greeting for interactive terminals (apex-logs 33, 14)
#
# ONE file, sourced by BOTH shells: /etc/bashrc for bash and the seeded ~/.zshrc
# for zsh. It used to be an inline block appended only to /etc/bashrc — but zsh is
# the DEFAULT LOGIN SHELL on APEX-OS (see /etc/default/useradd), so in practice no
# user ever saw the greeting in their own terminal. It only ever appeared if you
# explicitly started bash. Keeping the logic in one place is also why the colour
# handling below cannot drift between the two shells.
#
# Written in POSIX sh on purpose: it is sourced by bash and zsh, and must not
# depend on the syntax of either — so no bash-only test brackets, no arrays, no
# `local`. The build asserts this file is free of bash-only test syntax, which is
# also why that syntax is described here in words rather than written out.
#
# Opt out with APEX_NO_GREETING=1 — set it in ~/.zshrc.local or ~/.bashrc.d, which
# are both read BEFORE this runs.

_apex_greet=1

# Once per shell session. APEX_GREETED is exported, so a nested shell (or a tmux
# pane, or a subshell) does not greet you a second time.
[ -n "${APEX_GREETED:-}" ] && _apex_greet=0

# Explicit user opt-out.
[ -n "${APEX_NO_GREETING:-}" ] && _apex_greet=0

# Interactive only. Printing a 20-line logo into a non-interactive shell corrupts
# scp/sftp/rsync sessions and anything parsing command output.
case $- in
    *i*) ;;
    *)   _apex_greet=0 ;;
esac

# stdout must be a terminal. An interactive shell can still have its output
# redirected, and a captured greeting is just garbage in a file.
[ -t 1 ] || _apex_greet=0

# Dumb or unset TERM means something like Emacs tramp or a bare pipe on the far
# end of a login: no cursor control, so the logo renders as line noise.
case "${TERM:-}" in
    ""|dumb) _apex_greet=0 ;;
esac

command -v fastfetch >/dev/null 2>&1 || _apex_greet=0

if [ "${_apex_greet}" = 1 ]; then
    APEX_GREETED=1
    export APEX_GREETED
    # The logo is drawn in the foreground colour, so it has to invert with the
    # desktop theme or it disappears against the background on a light scheme.
    _apex_logo_color=white
    case "$(gsettings get org.gnome.desktop.interface color-scheme 2>/dev/null)" in
        *light*) _apex_logo_color=black ;;
    esac
    if [ -r /etc/fastfetch/config.jsonc ]; then
        fastfetch --config /etc/fastfetch/config.jsonc --logo-color-1 "${_apex_logo_color}"
    else
        # Config missing (a partial image, or a user deleted it): still greet
        # rather than silently printing nothing at all.
        fastfetch --logo-color-1 "${_apex_logo_color}"
    fi
    unset _apex_logo_color
fi

unset _apex_greet
