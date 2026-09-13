#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  test-apex-ai-apps.sh — the desktop AI apps ship WITH the system, and bring
#  no update channel of their own.
#
#  ── What this exists for ────────────────────────────────────────────────────
#  The 2026-09-11 product decision made the ChatGPT desktop app and the Claude
#  desktop app part of APEX-OS: baked into the image, picked up by existing
#  machines through `sudo apex update`, and updated ONLY by an image rebuild.
#  Three things can go wrong with that, and each has already happened once on a
#  real machine:
#
#    1. The app is installed somewhere that is not the image. Claude Desktop was
#       unpacked by hand into /usr/local/lib/claude-desktop, and /usr/local on
#       APEX is a symlink to /var/usrlocal — machine-local, not in the image, on
#       no other laptop, and gone on a reinstall. It also SHADOWS the image:
#       /usr/local/bin comes before /usr/bin on PATH, so on that one machine
#       `command -v claude-desktop` answers with the hand-install no matter what
#       the image ships. A test that only asked "is claude-desktop on PATH?"
#       would pass while measuring the wrong thing entirely, so the live section
#       below asks each prefix separately and prints which one answered.
#
#    2. The app brings its own updater. Both vendors package for mutable
#       distributions, where installing the app also subscribes the machine to
#       the vendor's repository: OpenAI's rpm SHIPS /etc/yum.repos.d/chatgpt.repo
#       with enabled=1, and Anthropic's deb writes an apt source plus an
#       unattended-upgrades snippet from its maintainer script. On APEX that is
#       not merely useless (/usr is read-only) but harmful: `apex-pkg` builds
#       user system extensions with dnf against the HOST's repo set, so an
#       enabled vendor repo turns `apex install chatgpt` into a newer build
#       layered into an extension that shadows the image's own /usr. Self-update
#       through a side channel. Both halves are asserted structurally, because
#       by the time it is visible on a machine it has already happened.
#
#    3. The app ships and no launcher can see it. This is the Zed defect
#       (tests/test-apex-editors.sh), and ChatGPT arrives one step from it:
#       upstream ships its icon ONLY at /usr/share/pixmaps/chatgpt.png and
#       nothing under hicolor. So the entry, the binary it names, the icon it
#       names, and — the part a file check cannot see — the SCHEME HANDLER are
#       each asserted against the built artefact.
#
#  ── Structural and live, for the reason the editors suite gives ─────────────
#  STRUCTURAL (always): the Containerfile really installs these packages,
#  really deletes the update channels, really verifies the signatures, and does
#  not swallow any of it. Runs in CI, where neither app is present.
#
#  LIVE (only where the artefacts are): the shipped entries, binaries, icons and
#  registered schemes on this machine. The structural layer alone would pass a
#  build that did all of it into the wrong prefix — which is exactly the mistake
#  item 1 describes.
#
#  Nothing here launches either app. A suite that opened a window on the
#  developer's session to prove a window opens is not run twice.
#
#  Run from anywhere: ./tests/test-apex-ai-apps.sh
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
note() { printf '      %s\n' "$1"; }
section() { printf '\n\033[1m── %s ──\033[0m\n' "$1"; }
want() { local d="$1"; shift; if "$@"; then ok "$d"; else bad "$d"; fi; }

# The 5a-aiapps stanza, isolated once so every structural assertion reads the
# same text. It ends at the stanza's own final echo, so a later stage that
# mentions either app cannot satisfy an assertion about this one.
#
# COMMENTS ARE STRIPPED, then backslash-continuations joined — the same
# pipeline, for the same two reasons, as test-apex-editors.sh. The stanza's
# header explains the bugs it prevents and therefore quotes the very paths
# being asserted; a checker that cannot tell an explanation from an instruction
# reports the documentation as the defect. And the command and its guard live
# on different physical lines, so a per-line grep can be satisfied by text that
# is not in the command it claims to check.
STANZA=$(mktemp); CODE=$(mktemp); CMDS=$(mktemp); ENVL=$(mktemp)
trap 'rm -f "$STANZA" "$CODE" "$CMDS" "$ENVL"' EXIT
awk '/^# ── Stage 5a-aiapps:/{f=1} f{print} f&&/ai-apps: desktop entries and scheme handlers registered/{exit}' \
    "$CF" > "$STANZA"
grep -v '^[[:space:]]*#' "$STANZA" | sed -e :a -e '/\\$/N; s/\\\n[[:space:]]*//; ta' > "$CODE"
# The joined stanza is effectively ONE line. Splitting it back into individual
# commands on `;` is what lets an assertion name a command rather than a
# substring that happens to appear in a FATAL message — the trap that left the
# xdg-terminal-exec check in test-apex-editors.sh unable to fail.
tr ';' '\n' < "$CODE" | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//' > "$CMDS"
is_cmd() { grep -qE "$1" "$CMDS"; }

section "the stanza exists and is readable"

want "the 5a-aiapps stage is present and findable" \
    test -s "$STANZA"
want "  ...and the comment-strip/join pipeline produced commands to check" \
    test "$(grep -c . "$CMDS")" -gt 20

section "both apps are installed from the vendors' own signed packages"

# The install TARGET, isolated from the stanza. The stanza contains FATAL text
# naming chatgpt several times over, so a stanza-wide grep for the name would
# stay green with the install deleted — the exact mutant this extraction exists
# to catch.
pkgs=$(sed -n 's/^dnf5 -y install //p' "$CMDS" | tr ' ' '\n' | grep -v '^-' | grep .)
have_pkg() { printf '%s\n' "$pkgs" | grep -qx "$1"; }

want "the chatgpt rpm is what dnf5 installs, not merely a name mentioned nearby" \
    have_pkg /tmp/chatgpt.rpm

# The control on the assertion above: if the extraction ever stopped matching,
# have_pkg would report everything missing and the check would be dead in the
# same silent way the defect is. Exactly one install target is expected here.
want "  ...and the extraction that proves it still finds exactly one target" \
    test "$(printf '%s\n' "$pkgs" | grep -c .)" = 1

# ChatGPT goes through dnf rather than being unpacked, and that is load-bearing
# rather than stylistic: apex-pkg asks the system rpmdb whether a package is
# image-owned, so an unpacked ChatGPT would leave `apex install chatgpt` free to
# shadow 442 MB of image with an extension.
want "the claude deb is UNPACKED rather than installed by dpkg" \
    is_cmd '^tar -xJf /tmp/cl-deb/data\.tar\.xz -C /tmp/cl-root$'

# Running the maintainer script is the one thing that would re-create every
# update channel this stage removes, so the absence is asserted, not assumed.
if grep -q 'dpkg' "$CODE"; then
    bad "nothing in the stanza runs dpkg (which would run the deb's maintainer script)"
else
    ok "nothing in the stanza runs dpkg (which would run the deb's maintainer script)"
fi

section "the signatures are verified against fingerprints pinned in THIS repository"

# A key taken out of the package it is meant to authenticate proves nothing on
# its own. These two constants are the independent half, and they are the only
# thing standing between the image and a substituted package.
want "the chatgpt signing key is pinned by fingerprint" \
    grep -q 'CHATGPT_KEY_FPR="3BFA0E4AE8B8CC16A2D9BA684A3B4A566C4660E4"' "$CMDS"
want "the claude signing key is pinned by fingerprint" \
    grep -q 'CLAUDE_KEY_FPR="31DDDE24DDFAB679F42D7BD2BAA929FF1A7ECACE"' "$CMDS"

# Declaring a constant and never comparing anything to it is the shape of a
# check that cannot fail, so the comparison itself is a separate assertion.
want "the chatgpt key is actually COMPARED to its pinned fingerprint" \
    is_cmd '^test "\$cgfpr" = "\$CHATGPT_KEY_FPR"'
want "the claude key is actually COMPARED to its pinned fingerprint" \
    is_cmd '^test "\$clfpr" = "\$CLAUDE_KEY_FPR"'

want "the chatgpt rpm's signature is checked with that key" \
    is_cmd '^rpm --dbpath /tmp/cg-rpmdb -K /tmp/chatgpt\.rpm'

# The verification must not widen the image's own trust store: a key imported
# into the real rpmdb is honoured by every later rpm and dnf call on the
# machine — including apex-pkg's extension builds — on behalf of a repository
# this very stanza deletes.
if sed -n 's/^rpm .*--import.*/&/p' "$CMDS" | grep -qv -- '--dbpath'; then
    bad "every rpm --import goes to a throwaway rpmdb, never the image's"
else
    ok "every rpm --import goes to a throwaway rpmdb, never the image's"
fi

# The deb's sha256 is only as good as the index it came from, so the chain is
# closed: signature on the index, and the index we read must be the one the
# signature covers.
want "the claude deb is checked against the sha256 the apt index publishes" \
    is_cmd '\| sha256sum -c -'
want "the apt index itself is signature-verified with gpgv" \
    is_cmd '^gpgv --keyring /tmp/cl-key\.gpg /tmp/cl-InRelease'
want "the Packages file read for that sha256 must be named by the signed InRelease" \
    is_cmd '^grep -q "\$clpsha" /tmp/cl-InRelease'

section "neither app keeps an update channel of its own"

# The mutation this is written for: delete the rm and watch THIS go red. The
# stanza also carries a FATAL naming chatgpt.repo and a `test ! -e` on the same
# path, so the assertion names the REMOVAL COMMAND, not the path.
want "the vendor rpm's /etc/yum.repos.d/chatgpt.repo is removed by an actual rm" \
    is_cmd '^rm -f /etc/yum\.repos\.d/chatgpt\.repo$'

want "the build fails if that repo file survives the install" \
    grep -q 'FATAL: chatgpt.repo survived' "$CODE"
want "the build fails if an apt source for claude-desktop ever appears" \
    grep -q "FATAL: an apt source for claude-desktop exists" "$CODE"

# No per-app timer may ride alongside the OS update workflow. Nothing in the
# stanza should be creating units at all; this catches a future edit that does.
if grep -qE '(systemctl enable|\.timer|/usr/lib/systemd/system)' "$CODE"; then
    bad "the stanza installs or enables no per-app systemd unit or timer"
else
    ok "the stanza installs or enables no per-app systemd unit or timer"
fi

section "what ships is launchable: entry, binary, icon, and the scheme"

want "the build fails if chatgpt's binary is not executable" \
    grep -q 'FATAL: chatgpt installed but /usr/bin/chatgpt is not executable' "$CODE"
want "the build fails if chatgpt ships no desktop entry" \
    grep -q 'FATAL: chatgpt installed but shipped no desktop entry' "$CODE"
want "the build fails if claude-desktop's binary is not executable" \
    grep -q 'FATAL: claude-desktop unpacked but /usr/bin/claude-desktop is not executable' "$CODE"
want "the build fails if claude-desktop ships no desktop entry" \
    grep -q 'FATAL: claude-desktop unpacked but shipped no desktop entry' "$CODE"

# Upstream puts ChatGPT's icon only in /usr/share/pixmaps. That is spec-legal
# and it is also one lookup-path change from the invisible-launcher bug, so the
# image installs it where APEX's own assertions look.
want "chatgpt's pixmap is also installed into the hicolor theme" \
    is_cmd '^install -Dm644 /usr/share/pixmaps/chatgpt\.png /usr/share/icons/hicolor/1024x1024/apps/chatgpt\.png$'
want "the build fails if that hicolor icon does not arrive" \
    grep -q 'FATAL: chatgpt shipped no hicolor icon' "$CODE"
want "the build fails if claude-desktop's hicolor icon does not arrive" \
    grep -q 'FATAL: claude-desktop shipped no hicolor icon' "$CODE"

# Chromium exits at launch when it finds an unprivileged chrome-sandbox, so
# this is not hardening — it is whether the app opens at all.
want "claude-desktop's chrome-sandbox is made setuid" \
    is_cmd '^chmod 4755 /usr/lib/claude-desktop/chrome-sandbox$'
want "the build fails if it is not setuid" \
    grep -q 'FATAL: claude-desktop.s chrome-sandbox is not setuid' "$CODE"

# A desktop entry on disk is not a registered handler. The cache the desktop
# actually reads is mimeinfo.cache, and asserting the EFFECTIVE result there is
# the only honest test that `claude://` and `codex://` will open these apps.
want "the desktop database is rebuilt after the entries are installed" \
    is_cmd '^update-desktop-database /usr/share/applications$'
want "the build fails if claude:// is not registered in mimeinfo.cache" \
    grep -q 'FATAL: claude:// is not registered to com.anthropic.Claude.desktop' "$CODE"
want "the build fails if codex:// is not registered in mimeinfo.cache" \
    grep -q 'FATAL: codex:// is not registered to chatgpt.desktop' "$CODE"

# The fetch must stay non-fatal, or a build box with no network stops producing
# an image. Asserting all of the above without this invites someone to make the
# whole stage fatal.
want "the chatgpt fetch itself stays non-fatal" \
    grep -q 'chatgpt: fetch FAILED' "$CODE"
want "the claude-desktop fetch itself stays non-fatal" \
    grep -q 'claude-desktop: fetch FAILED' "$CODE"

# The swallow is how Zed's wrong filename survived for months.
if grep -E 'install .*applications' "$CMDS" | grep -q '|| true'; then
    bad "no desktop-entry install is swallowed by || true"
else
    ok "no desktop-entry install is swallowed by || true"
fi

section "Electron apps get Wayland, not XWayland"

# Both apps are Electron, and Electron defaults to X11 unless told otherwise —
# on a Wayland-first image that means XWayland, with the scaling and input
# behaviour that follows. The hint belongs with the other session-wide settings
# in /etc/environment, beside DISABLE_AUTOUPDATER, not in the app stanza.
grep -n "printf 'QT_QPA_PLATFORMTHEME" "$CF" | head -n1 | cut -d: -f1 \
    | while read -r n; do sed -n "${n}p" "$CF"; done > "$ENVL"

want "the /etc/environment line was found" \
    test -s "$ENVL"
want "it sets ELECTRON_OZONE_PLATFORM_HINT so Electron picks Wayland when there is one" \
    grep -q 'ELECTRON_OZONE_PLATFORM_HINT=auto' "$ENVL"
# The control: the settings that have always been on that line must still be
# there, or the extraction is reading something else.
want "  ...and the line still carries the settings it always had" \
    grep -q 'DISABLE_AUTOUPDATER=1' "$ENVL"

section "the shipped artefacts on this machine, where there are any"

# /usr/local/bin precedes /usr/bin on PATH and /usr/local is /var/usrlocal —
# machine-local, not the image. So each prefix is asked separately and the
# answer is printed: a PASS here must never be a hand-install being measured.
img_claude=""; loc_claude=""
[ -x /usr/bin/claude-desktop ]       && img_claude=yes
[ -e /usr/local/bin/claude-desktop ] && loc_claude=yes
resolved=$(command -v claude-desktop 2>/dev/null)
note "command -v claude-desktop -> ${resolved:-(nothing)}"
note "image prefix  /usr/bin/claude-desktop        : ${img_claude:-absent}"
note "local prefix  /usr/local/bin/claude-desktop  : ${loc_claude:-absent}"

if grep -q '^PRETTY_NAME="APEX-OS"' /usr/lib/os-release 2>/dev/null; then
    if [ -n "$img_claude" ]; then
        ok "Claude Desktop is installed by the IMAGE at /usr/bin/claude-desktop"
    else
        bad "Claude Desktop is installed by the IMAGE at /usr/bin/claude-desktop"
        note "This machine predates the fix: the app here (if any) is the"
        note "hand-unpacked /usr/local tree, which is machine-local and in no"
        note "image. Expected until this branch reaches an image build."
    fi
    if [ -n "$loc_claude" ]; then
        note "NOTE: the /usr/local stopgap is present and SHADOWS the image copy"
        note "on PATH. It stays until an in-image version lands — removing it"
        note "today would leave this machine with no Claude Desktop updates."
    fi

    # ChatGPT can be present from the image OR from a user system extension
    # merged over /usr, and those are not the same claim. The rpmdb settles it.
    if [ -e /usr/bin/chatgpt ]; then
        if rpm -qf /usr/bin/chatgpt >/dev/null 2>&1; then
            ok "ChatGPT is owned by an image package ($(rpm -qf /usr/bin/chatgpt 2>/dev/null))"
        else
            bad "ChatGPT is owned by an image package"
            note "/usr/bin/chatgpt exists but the system rpmdb does not own it,"
            note "so it is coming from a system extension merged over /usr, not"
            note "from the image. apex-pkg's image-owner guard cannot see it."
        fi
    else
        bad "ChatGPT is installed at /usr/bin/chatgpt"
        note "Expected until this branch reaches an image build."
    fi

    # The update channel, live. This is the assertion that would have caught
    # the state the developer's laptop is in today.
    if [ -e /etc/yum.repos.d/chatgpt.repo ]; then
        bad "no vendor repo for ChatGPT is configured on this machine"
        note "/etc/yum.repos.d/chatgpt.repo is present. apex-pkg builds"
        note "extensions with dnf against the host's repo set, so this is a"
        note "path by which the app can be updated outside \`apex update\`."
    else
        ok "no vendor repo for ChatGPT is configured on this machine"
    fi

    # No per-app updater timer may run beside the OS update workflow. The
    # user-level stopgap is reported rather than failed: it is deliberately
    # kept until an in-image Claude Desktop exists to replace it.
    if systemctl --user list-unit-files 'claude-desktop-update.timer' 2>/dev/null \
         | grep -q 'claude-desktop-update.timer'; then
        note "NOTE: the user-level claude-desktop-update.timer exists. That is"
        note "the documented stopgap and must NOT be retired before an in-image"
        note "Claude Desktop lands, or this machine stops getting the app at all."
    fi
    # `find -iname`, not `ls | grep`: a unit file whose name contained a
    # newline would reach the emptiness test below as two names, and the
    # assertion is that there are NONE.
    sys_timers=$(find /usr/lib/systemd/system/ -maxdepth 1 \
        \( -iname 'chatgpt*.timer'  -o -iname 'chatgpt*.service' \
        -o -iname 'claude*.timer'   -o -iname 'claude*.service' \) \
        -printf '%f\n' 2>/dev/null)
    if [ -z "$sys_timers" ]; then
        ok "the image ships no per-app updater unit for either app"
    else
        bad "the image ships no per-app updater unit for either app"
        note "found: $sys_timers"
    fi

    # Firefox must still own the web handlers. ChatGPT's entry registers
    # x-scheme-handler/http and https for itself, so it joins the "Open With"
    # list for every link — but it must never become the default browser.
    if [ -s /etc/xdg/mimeapps.list ]; then
        if grep -q '^x-scheme-handler/https=firefox.desktop$' /etc/xdg/mimeapps.list; then
            ok "firefox still owns https, despite ChatGPT registering for it"
        else
            bad "firefox still owns https, despite ChatGPT registering for it"
        fi
    else
        skp "/etc/xdg/mimeapps.list is not on this machine"
    fi
else
    skp "not an APEX image (the live half does not apply here)"
fi

# Entry-level checks run wherever the entries are, APEX or not: they are about
# the files, not the distribution.
check_entry() {
    local entry="$1" label="$2"
    if [ ! -s "$entry" ]; then
        skp "$label entry is not on this machine"
        return
    fi
    ok "$label ships a launcher entry"
    local execbin
    execbin=$(sed -n 's/^Exec=\([^ ]*\).*/\1/p' "$entry" | head -n1)
    if [ -n "$execbin" ] && command -v "$execbin" >/dev/null 2>&1; then
        ok "  its Exec ($execbin) resolves on PATH"
    else
        bad "  its Exec (${execbin:-none}) resolves on PATH"
    fi
    local icon
    icon=$(sed -n 's/^Icon=//p' "$entry" | head -n1)
    if [ -n "$icon" ] && [ -n "$(find /usr/share/icons /usr/share/pixmaps \
            /usr/local/share/icons -name "${icon}.png" -o -name "${icon}.svg" \
            2>/dev/null | head -1)" ]; then
        ok "  its Icon ($icon) resolves in an installed theme"
    else
        bad "  its Icon (${icon:-none}) resolves in an installed theme"
    fi
}
check_entry /usr/share/applications/chatgpt.desktop "ChatGPT"
if [ -s /usr/share/applications/com.anthropic.Claude.desktop ]; then
    check_entry /usr/share/applications/com.anthropic.Claude.desktop "Claude Desktop"
else
    check_entry /usr/local/share/applications/com.anthropic.Claude.desktop \
        "Claude Desktop (/usr/local stopgap)"
fi

# The schemes, as the desktop resolves them. A handler that never reached
# mimeinfo.cache does not exist to xdg-open, whatever the entry says.
# Gated on the ENTRY being installed here, not on a cache file existing: on a
# machine that ships neither app — a CI runner, a developer's Ubuntu box —
# "claude:// is not registered" is not a finding, it is the absence of the app.
# Where the app IS installed, a missing registration is exactly the invisible
# half-failure this suite exists for.
IMG_CACHE=/usr/share/applications/mimeinfo.cache
LOC_CACHE=/usr/local/share/applications/mimeinfo.cache
for pair in "claude:com.anthropic.Claude.desktop" "codex:chatgpt.desktop"; do
    scheme=${pair%%:*}; want_entry=${pair#*:}
    if [ ! -s "/usr/share/applications/${want_entry}" ] \
       && [ ! -s "/usr/local/share/applications/${want_entry}" ]; then
        skp "${scheme}:// — ${want_entry} is not installed on this machine"
        continue
    fi
    pat="^x-scheme-handler/${scheme}=.*${want_entry}"
        if [ -s "$IMG_CACHE" ] && grep -q "$pat" "$IMG_CACHE"; then
            ok "${scheme}:// is registered to ${want_entry} by the image"
        elif [ -s "$LOC_CACHE" ] && grep -q "$pat" "$LOC_CACHE"; then
            bad "${scheme}:// is registered to ${want_entry} by the image"
            note "It IS registered — but from $LOC_CACHE, i.e. the machine-local"
            note "/usr/local stopgap, which is on XDG_DATA_DIRS and so really"
            note "works here and on no other machine. Expected until this"
            note "branch reaches an image build."
        else
            bad "${scheme}:// is registered to ${want_entry} by the image"
        fi
done

printf '\napex-ai-apps: %d passed, %d failed, %d skipped\n' "$pass" "$fail" "$skip"
[ "$fail" -eq 0 ]
