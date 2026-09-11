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
#  Neovim is checked in the same shape for a different reason: it ships an entry
#  with `Terminal=true`, which is only launchable if the machine has a terminal
#  emulator for the desktop to hand it to. The entry existing proves nothing on
#  its own.
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

printf '\napex-editors: %d passed, %d failed, %d skipped\n' "$pass" "$fail" "$skip"
[ "$fail" -eq 0 ]
