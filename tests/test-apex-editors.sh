#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  test-apex-editors.sh — the editors APEX ships can actually be launched.
#
#  ── The failure this exists for ─────────────────────────────────────────────
#  Zed shipped on every APEX image, ran fine from a shell, and was invisible in
#  every launcher, menu and "Open With" list. Two mistakes compounded in the
#  Containerfile:
#
#    1. the desktop entry was installed from `zed.app/share/applications/
#       zed.desktop`, a name the tarball has never shipped — upstream's file is
#       `dev.zed.Zed.desktop`;
#    2. `2>/dev/null || true` on that install swallowed the failure.
#
#  A binary on PATH with no launcher entry is, to the person using the machine,
#  an editor that does not work. Nothing in the build or the suites disagreed
#  with them, which is the actual defect: the image asserted the fetch and never
#  asserted the result.
#
#  ── What this suite checks, and why in two layers ───────────────────────────
#  STRUCTURAL (always): the Containerfile installs the name the tarball really
#  ships, does not swallow that install, installs an icon, and asserts the
#  result. This layer runs in CI where there is no Zed and no /usr/lib/zed.app.
#
#  LIVE (only when the artefact is present): the shipped entry exists, its
#  TryExec resolves on PATH, and its Icon resolves in the hicolor theme. The
#  structural layer alone would pass on a build that installed a correct entry
#  into the wrong prefix.
#
#  Neovim is checked in the same shape for a different reason, and it turned out
#  to be a second live defect rather than a precaution. Its entry is correct and
#  says `Terminal=true`, which is an instruction to the DESKTOP: supply a
#  terminal. freedesktop's mechanism for that is `xdg-terminal-exec`, and no
#  APEX machine had it — while /etc/xdg/xdg-terminals.list, which is that
#  program's config file and names Alacritty.desktop, has shipped since
#  apex-logs 31. The configuration was on every install and the program that
#  reads it was never packaged. So nvim ran fine from a shell everywhere and
#  could not be started from the desktop anywhere, and `apex install neovim`
#  told the user it was already "provided by APEX-OS" — which is true, and is
#  apex-pkg refusing to shadow an image package, and is not the bug.
#
#  The shell half of the repair (routing Terminal=true entries through the
#  helper instead of calling DesktopEntry.execute(), which does not honour the
#  field) is in apex-shell on task/terminal-entries-launchable, measured by its
#  tests/run-terminal-entry-test.sh. THIS file asserts the image's half: that
#  the helper is installed, that the config and the helper and $TERMINAL all
#  name the same terminal, and that the terminal they name is really there.
#
#  Nothing here launches an editor. A suite that opened a window on the
#  developer's session to prove a window opens is not run twice.
#
#  Run from anywhere: ./tests/test-apex-editors.sh
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail
set +e
cd "$(dirname "$0")" || exit 2
REPO=$(cd .. && pwd)
CF="$REPO/Containerfile.core"
CFB="$REPO/Containerfile.base"

pass=0; fail=0; skip=0
ok()   { printf 'PASS  %s\n' "$1"; pass=$((pass+1)); }
bad()  { printf 'FAIL  %s\n' "$1"; fail=$((fail+1)); }
skp()  { printf 'SKIP  %s\n' "$1"; skip=$((skip+1)); }
section() { printf '\n\033[1m── %s ──\033[0m\n' "$1"; }
want() { local d="$1"; shift; if "$@"; then ok "$d"; else bad "$d"; fi; }

# The zed stanza, isolated once into a file so every structural assertion reads
# the same text and nothing below re-derives it. It ends at the next RUN, so a
# later stage's mention of zed cannot satisfy an assertion about this one.
#
# COMMENTS ARE STRIPPED. The stanza's own comment block quotes the wrong
# filename while explaining the bug, and the first version of this suite failed
# on that quotation — a checker that cannot tell an explanation from an
# instruction reports the documentation as the defect.
STANZA=$(mktemp); trap 'rm -f "$STANZA"' EXIT
awk '/^# ── Stage 5a-zed:/{f=1} f{print} f&&/rm -f \/tmp\/zed.tgz/{exit}' "$CF" > "$STANZA"
CODE=$(mktemp); trap 'rm -f "$STANZA" "$CODE"' EXIT
# Comments out, THEN backslash-continuations joined. The second half is not
# tidiness: the install and its `|| true` live on different physical lines, so
# the first version of the swallow assertion below grepped one line for a
# pattern that was only ever on the next one, and passed against a mutant that
# reintroduced the bug. An assertion that cannot fail is worse than none.
grep -v '^[[:space:]]*#' "$STANZA" | sed -e :a -e '/\\$/N; s/\\\n[[:space:]]*//; ta' > "$CODE"

section "the Containerfile installs the file the tarball actually ships"

want "the zed stage is still present and findable" \
    test -s "$STANZA"

# The bug, stated as the assertion that would have caught it on the day.
want "the desktop entry is installed from dev.zed.Zed.desktop" \
    grep -q 'zed.app/share/applications/dev.zed.Zed.desktop' "$CODE"

if grep -q 'share/applications/zed\.desktop' "$CODE"; then
    bad "no install reads share/applications/zed.desktop (the name upstream never shipped)"
else
    ok "no install reads share/applications/zed.desktop (the name upstream never shipped)"
fi

# The swallow is the reason the wrong name survived, so it is asserted
# separately: fixing the name while keeping `|| true` would leave the next
# upstream rename just as silent.
if grep -E 'install .*applications' "$CODE" | grep -q '|| true'; then
    bad "the desktop-entry install is not swallowed by || true"
else
    ok "the desktop-entry install is not swallowed by || true"
fi

want "an icon is installed into the hicolor theme" \
    grep -q 'share/icons/hicolor' "$CODE"

want "the build fails if the entry does not arrive" \
    grep -q 'FATAL: zed unpacked but shipped no desktop entry' "$CODE"

want "the build fails if the entry names a binary that is not there" \
    grep -q 'test -x /usr/bin/zed' "$CODE"

want "the build fails if the icon does not arrive" \
    grep -q 'FATAL: zed shipped no 512x512 icon' "$CODE"

# The fetch must stay non-fatal: a build box with no network should still
# produce an image. Asserting the assertions above without this one would invite
# someone to make the whole stage fatal and break offline builds.
want "the fetch itself stays non-fatal" \
    grep -q 'zed: fetch FAILED' "$CODE"

section "the shipped artefact, where there is one"

if [ -d /usr/lib/zed.app ]; then
    want "zed.app ships the entry this build installs" \
        test -s /usr/lib/zed.app/share/applications/dev.zed.Zed.desktop
    if [ -s /usr/share/applications/dev.zed.Zed.desktop ]; then
        ok "a launcher entry for Zed is installed"
        tryexec=$(sed -n 's/^TryExec=//p' /usr/share/applications/dev.zed.Zed.desktop | head -n1)
        if [ -n "$tryexec" ] && command -v "$tryexec" >/dev/null 2>&1; then
            ok "its TryExec ($tryexec) resolves on PATH"
        else
            bad "its TryExec (${tryexec:-none}) resolves on PATH"
        fi
        icon=$(sed -n 's/^Icon=//p' /usr/share/applications/dev.zed.Zed.desktop | head -n1)
        if [ -n "$icon" ] && [ -n "$(find /usr/share/icons /usr/local/share/icons -name "${icon}.png" -o -name "${icon}.svg" 2>/dev/null | head -1)" ]; then
            ok "its Icon ($icon) resolves in an installed theme"
        else
            bad "its Icon (${icon:-none}) resolves in an installed theme"
        fi
    else
        bad "a launcher entry for Zed is installed"
    fi
else
    skp "zed.app is not on this machine (structural layer above still ran)"
fi

section "neovim ships an entry, and a terminal to honour it"

if [ -s /usr/share/applications/nvim.desktop ]; then
    ok "a launcher entry for Neovim is installed"
    tryexec=$(sed -n 's/^TryExec=//p' /usr/share/applications/nvim.desktop | head -n1)
    if [ -n "$tryexec" ] && command -v "$tryexec" >/dev/null 2>&1; then
        ok "its TryExec ($tryexec) resolves on PATH"
    else
        bad "its TryExec (${tryexec:-none}) resolves on PATH"
    fi
    # Terminal=true is not a defect — it is a fact with a consequence, and the
    # consequence is that SOMETHING has to be there to host it.
    if grep -qx 'Terminal=true' /usr/share/applications/nvim.desktop; then
        ok "the entry declares Terminal=true, so it needs a terminal emulator"
        found=""
        for t in kitty alacritty foot ghostty wezterm xterm; do
            command -v "$t" >/dev/null 2>&1 && { found="$t"; break; }
        done
        if [ -n "$found" ]; then
            ok "the image ships a terminal emulator to host it ($found)"
        else
            bad "the image ships a terminal emulator to host it"
        fi
    else
        skp "the entry does not declare Terminal=true"
    fi
else
    skp "nvim.desktop is not on this machine"
fi

section "the image installs the program that makes Terminal=true mean something"

# Same comment-strip-then-join pipeline as the zed stanza above, and for the
# same reason: the package name and the assertion that it arrived are on
# different physical lines of one `\`-continued RUN, so a per-line grep can be
# satisfied by text that is not in the command it claims to check.
DESKTOP=$(mktemp); TERMBLK=$(mktemp)
trap 'rm -f "$STANZA" "$CODE" "$DESKTOP" "$TERMBLK"' EXIT
awk '/^RUN set -eux; \\$/{buf=""; f=1} f{buf=buf $0 "\n"} f&&/dnf5 clean all|^$/{if (buf ~ /alacritty/) {printf "%s", buf; exit} f=0}' "$CF" \
    | grep -v '^[[:space:]]*#' | sed -e :a -e '/\\$/N; s/\\\n[[:space:]]*//; ta' > "$DESKTOP"
# The Containerfile.base block that checks the three parts agree. It starts at
# the COPY of the list, so a `xdg-terminal-exec` mentioned anywhere else in the
# file cannot satisfy an assertion about this one.
awk '/^COPY files\/system\/xdg\/xdg-terminals.list/{f=1} f{print} f&&/agrees with TERMINAL/{exit}' "$CFB" \
    | grep -v '^[[:space:]]*#' | sed -e :a -e '/\\$/N; s/\\\n[[:space:]]*//; ta' > "$TERMBLK"

want "the desktop-package stanza was found in Containerfile.core" \
    test -s "$DESKTOP"

# The PACKAGE LIST, isolated, not the stanza. The first version of this
# assertion grepped the whole joined stanza for `xdg-terminal-exec` — and the
# stanza also contains `command -v xdg-terminal-exec` and a FATAL message
# naming it, so deleting the package from the install line left it passing.
# It could not fail against the one mutant it exists for. Everything between
# `dnf5 -y install` and the next `;`, split into words, matched whole.
# Split on `;` FIRST. The stanza is one joined line carrying two installs
# (`dnf5 -y install <the desktop set>` and, further down, `dnf5 -y install
# --skip-unavailable unrar`), and `.*dnf5 -y install ` is greedy — it matched
# the SECOND one and reported a two-package image. Every install in the stanza
# now contributes, and `-` flags are dropped rather than counted as packages.
pkgs=$(tr ';' '\n' < "$DESKTOP" \
        | sed -n 's/^[[:space:]]*dnf5 -y install //p' \
        | tr ' ' '\n' | grep -v '^-' | grep .)
have_pkg() { printf '%s\n' "$pkgs" | grep -qx "$1"; }

want "xdg-terminal-exec is in the package list, not merely mentioned nearby" \
    have_pkg xdg-terminal-exec

# The control on the assertion above: if the word-split ever stopped matching
# anything, `have_pkg` would report every package missing and the check would
# be dead in the same silent way. A package that has been in this list for
# years proves the extraction still works.
want "  ...and the extraction that proves it still finds a known package" \
    have_pkg alacritty

# Installing it and never checking it arrived is how the shipped config file
# ended up with no reader for months.
want "the build fails if xdg-terminal-exec did not arrive" \
    grep -q 'FATAL: xdg-terminal-exec is not executable' "$DESKTOP"

want "the agreement block was found in Containerfile.base" \
    test -s "$TERMBLK"

want "the build fails if the list ships with no program to read it" \
    grep -q 'FATAL: /etc/xdg/xdg-terminals.list ships but xdg-terminal-exec does not' "$TERMBLK"

want "the build fails if the list names an entry that is not installed" \
    grep -q 'FATAL: xdg-terminals.list names .* not installed' "$TERMBLK"

# The two settings are written in different files by different stages, and
# nothing but this check stops them drifting apart.
want "the build fails if the list and \$TERMINAL name different terminals" \
    grep -q 'FATAL: xdg-terminals.list opens .* but /etc/environment sets TERMINAL=' "$TERMBLK"

section "the terminal chain on this machine, where it is installed"

# Gated on being an APEX machine, not on the files being there. Gating on the
# files would turn "the image stopped shipping the list" into a silent SKIP,
# which is the same shape as the defect this section exists for: a missing
# piece that nothing complained about. On anything that is not APEX — a CI
# runner, a developer's Ubuntu box — none of it applies and the whole section
# skips.
if grep -q '^PRETTY_NAME="APEX-OS"' /usr/lib/os-release 2>/dev/null; then
    if [ -s /etc/xdg/xdg-terminals.list ]; then
        ok "/etc/xdg/xdg-terminals.list is installed"
        xte_entry=$(grep -m1 -E -v '^[[:space:]]*([#/-]|$)' /etc/xdg/xdg-terminals.list || true)
        if [ -n "$xte_entry" ]; then
            ok "it names a terminal ($xte_entry)"
            if [ -s "/usr/share/applications/$xte_entry" ]; then
                ok "the entry it names is installed"
                xte_bin=$(sed -n 's/^TryExec=//p;s/^Exec=\([^ ]*\).*/\1/p' \
                    "/usr/share/applications/$xte_entry" | head -n1)
                env_bin=$(sed -n 's/^TERMINAL=//p' /etc/environment 2>/dev/null | head -n1)
                if [ -n "$env_bin" ] && [ "${xte_bin##*/}" = "${env_bin##*/}" ]; then
                    ok "it agrees with TERMINAL in /etc/environment (${env_bin##*/})"
                else
                    bad "it agrees with TERMINAL in /etc/environment (list says ${xte_bin:-none}, environment says ${env_bin:-none})"
                fi
            else
                bad "the entry it names is installed"
            fi
        else
            bad "it names a terminal"
        fi
    else
        bad "/etc/xdg/xdg-terminals.list is installed"
    fi

    # The live half of the defect itself. On a machine still running an image
    # built before this branch this FAILS, and the failure IS the report: it is
    # exactly what made clicking Neovim do nothing.
    if command -v xdg-terminal-exec >/dev/null 2>&1; then
        ok "xdg-terminal-exec is on PATH, so a Terminal=true entry has something to go through"
    else
        bad "xdg-terminal-exec is on PATH, so a Terminal=true entry has something to go through"
        echo "        This machine predates the fix: nvim.desktop says Terminal=true"
        echo "        and nothing installed here can honour it. Expected until the"
        echo "        branch reaches an image build; not a reason to hand-install it."
    fi
else
    skp "not an APEX image (the live terminal chain does not apply here)"
fi

printf '\napex-editors: %d passed, %d failed, %d skipped\n' "$pass" "$fail" "$skip"
[ "$fail" -eq 0 ]
