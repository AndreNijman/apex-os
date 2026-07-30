# APEX-OS — starship prompt for interactive bash (apex-logs 14)
#
# zsh gets starship from the seeded ~/.zshrc. bash had nothing at all, so every
# bash shell fell back to the stock "[user@host dir]$": a `bash` started inside a
# zsh session, a root shell, a TTY login, a `podman exec`, a rescue shell. The
# prompt has to come from the IMAGE rather than a per-user dotfile, because a
# dotfile is written once at first login and no later update can reach it.
#
# This is sourced for BOTH login and non-login interactive bash: /etc/profile
# walks /etc/profile.d for login shells, and Fedora's /etc/bashrc walks it again
# for interactive non-login shells.
#
# The guards, and why each one is load-bearing:
#   BASH_VERSION   this file is also sourced by zsh (through /etc/profile) and by
#                  plain sh. `starship init bash` emits bash-only syntax, and zsh
#                  already initialises starship from ~/.zshrc — without this
#                  guard, zsh sessions would error and double-init.
#   $- contains i  never touch a non-interactive shell. Prompt setup written into
#                  a script, or into an scp/rsync/sftp session, corrupts it.
#   STARSHIP_SHELL set by `starship init`, so a bash nested in a bash that has
#                  already initialised does not do it twice.
#   command -v     a minimal or rescue environment may not ship starship.
#
# starship needs no config file to function: with no ~/.config/starship.toml it
# falls back to its own default preset, so root and any un-provisioned account
# still get a good prompt. apex-shell-firstrun writes the edition-accented
# ~/.config/starship.toml for real users.
if [ -n "${BASH_VERSION:-}" ] && [ -z "${STARSHIP_SHELL:-}" ]; then
    case $- in
        *i*)
            if command -v starship >/dev/null 2>&1; then
                eval "$(starship init bash)"
            fi
            ;;
    esac
fi
