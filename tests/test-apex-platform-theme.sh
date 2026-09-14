#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  test-apex-platform-theme.sh — the Qt platform theme is not a palette
#  preference. It is the only reason any APEX surface mirrors for a
#  right-to-left language, and nothing used to assert its VALUE.
#
#  ── What this exists for ────────────────────────────────────────────────────
#  Measured 2026-09-13 (p2-b round 23) with `qmltestrunner -platform offscreen`
#  under `env -i`, one variable at a time:
#
#      LANG=ar_EG.UTF-8  QT_QPA_PLATFORMTHEME=qt6ct   layoutDirection=RTL
#      LANG=he_IL.UTF-8  QT_QPA_PLATFORMTHEME=qt6ct   layoutDirection=RTL
#      LANG=ar_EG.UTF-8  QT_QPA_PLATFORMTHEME unset   layoutDirection=LTR
#      LANG=ar_EG.UTF-8  QT_QPA_PLATFORMTHEME=bogus   layoutDirection=LTR
#      LANG=ur_PK.UTF-8  QT_QPA_PLATFORMTHEME=qt6ct   layoutDirection=LTR  <- control
#
#  Qt does not decide `Qt.application.layoutDirection` from the locale. It
#  decides it by TRANSLATING the string `QT_LAYOUT_DIRECTION` and comparing the
#  answer to `RTL`, so it needs a Qt translation catalogue for the language to
#  be loaded. Urdu is unambiguously right-to-left and Qt's own QLocale says so
#  in the same run — and the application direction still comes back
#  LeftToRight, because there is no `qt_ur.qm`. That control is what makes this
#  a mechanism rather than a coincidence.
#
#  Nothing in APEX installs a QTranslator. `/usr/bin/quickshell` calls neither
#  QTranslator nor installTranslator. The thing that installs one is the
#  **qt6ct platform theme plugin**, which loads `qt_<lang>.qm` on the way to
#  applying a palette. So right-to-left mirroring on a shipped APEX desktop
#  works entirely as a side effect of a dark-mode theming choice, and the
#  comment in files/system/qt6ct/qt6ct.conf says so in as many words: "dark
#  palette".
#
#  ── The chain has THREE links in the image and all three were unguarded ─────
#    1. `QT_QPA_PLATFORMTHEME=<theme>` written into /etc/environment.
#    2. the same variable in files/desktop/labwc/environment, for the labwc
#       session, which does not inherit the Hyprland `env =` lines.
#    3. the package that OWNS the plugin, and the package that owns the
#       `qt_<lang>.qm` catalogues it loads.
#
#  Link 3 is the one nobody wrote down. Measured on a booted APEX host:
#  `qt6-qttranslations` owns /usr/share/qt6/translations/qt_ar.qm, no package
#  REQUIRES it, and it reaches the image only as a `Recommends:` of
#  qt6-qtbase-gui. Any build that ever passes --setopt=install_weak_deps=False
#  — the standard image-slimming move, and apex-pkg already has a
#  --no-weak-deps flag — deletes right-to-left support with nothing red
#  anywhere. This repository has been bitten by that exact class before:
#  files/system/libexec/apex-pkg records that mksquashfs reaches the image only
#  as a weak dependency of dracut-squash.
#
#  ── Why a VALUE check and not a grep ────────────────────────────────────────
#  tests/test-apex-ai-apps.sh greps `printf 'QT_QPA_PLATFORMTHEME` to LOCATE
#  the /etc/environment line and then asserts ELECTRON_OZONE_PLATFORM_HINT on
#  it. So DELETING the variable does go red — in a suite about Electron, and
#  only because the variable happens to be a convenient anchor. Changing its
#  VALUE is caught by nothing at all, and that is the change that silently
#  un-mirrors every surface.
#
#  A bare `grep -q qt6ct` is no better: the name appears in a COPY path in
#  Containerfile.base, in the /etc/xdg/qt6ct/qt6ct.conf comment that explains
#  the palette, and in the printf itself. Two of those three set nothing. So
#  everything below extracts an ASSIGNMENT and compares VALUES, the extraction
#  is self-tested in both directions on canned input, and the three links are
#  required to AGREE — a mutant that changes all three together still fails,
#  because the value has to name a package the image really installs.
#
#  ── Structural and live ─────────────────────────────────────────────────────
#  STRUCTURAL (always, including CI): what the Containerfiles and the shipped
#  labwc environment file say.
#  LIVE (booted APEX only): /usr/etc/environment — the image's own copy, not
#  the /etc one a local edit could have changed — plus the plugin and the
#  catalogue, each resolved back to the rpm that owns it.
#
#  The runtime half of this — actually reading Qt.application.layoutDirection
#  back out of a running engine — belongs to apex-shell's tests/run-rtl-test.sh
#  and is deliberately not duplicated here. That suite SKIPs off a booted APEX
#  host, which is why this one exists in the repository that can break it.
#
#  Run from anywhere: ./tests/test-apex-platform-theme.sh
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail
set +e
cd "$(dirname "$0")/.." || exit 2

LABWC="files/desktop/labwc/environment"
QTCONF="files/system/qt6ct/qt6ct.conf"

pass=0; fail=0; skip=0
ok()   { printf 'PASS  %s\n' "$1"; pass=$((pass+1)); }
bad()  { printf 'FAIL  %s\n' "$1"; fail=$((fail+1)); }
skp()  { printf 'SKIP  %s\n' "$1"; skip=$((skip+1)); }
note() { printf '      %s\n' "$1"; }
section() { printf '\n\033[1m── %s ──\033[0m\n' "$1"; }
want() { local d="$1"; shift; if "$@"; then ok "$d"; else bad "$d"; fi; }

WORK=$(mktemp -d); trap 'rm -rf "$WORK"' EXIT
EXTRACT="$WORK/extract-env.py"

# ── the extraction, isolated so it can be self-tested ────────────────────────
#
# Emits one `path<TAB>redirect-target<TAB>value` row per QT_QPA_PLATFORMTHEME
# assignment written by a printf. Three things it deliberately does:
#
#   * comment lines are dropped BEFORE matching, so the qt6ct.conf comment and
#     the Containerfile prose that explain this mechanism cannot satisfy an
#     assertion about the mechanism;
#   * the redirect is read with [ \t]* and never \s*, so a printf that writes
#     nowhere cannot borrow the `>> /etc/environment` of a LATER command;
#   * the payload is split on the literal two characters `\n`, which is what a
#     single-quoted printf format really contains.
cat > "$EXTRACT" <<'PY'
import re, sys

VAR = "QT_QPA_PLATFORMTHEME"
for path in sys.argv[1:]:
    try:
        src = open(path, encoding="utf-8").read()
    except OSError:
        continue
    code = "\n".join(l for l in src.split("\n") if not l.lstrip().startswith("#"))
    for m in re.finditer(r"printf\s+'([^']*)'", code):
        payload = m.group(1)
        if VAR not in payload:
            continue
        # The target stops at the shell's own punctuation. `\S+` would swallow
        # the `;` that ends the command and report "/etc/environment;", which
        # compares unequal to "/etc/environment" — a suite red about its own
        # parser while the product is fine. The positive self-test above is
        # what found that, which is the whole reason it is there.
        r = re.match(r"[ \t]*>>?[ \t]*([^\s;&|\\]+)", code[m.end():])
        target = r.group(1) if r else "(no-redirect)"
        for part in payload.split("\\n"):
            part = part.strip()
            if part.startswith(VAR + "="):
                print("%s\t%s\t%s" % (path, target, part.split("=", 1)[1]))
PY

section "the extraction is exercised in both directions before it is trusted"

# Positive: a real printf line, matched, with its redirect and its value.
cat > "$WORK/pos" <<'FIX'
RUN set -eux; \
    printf 'QT_QPA_PLATFORMTHEME=qt6ct\nTERMINAL=alacritty\n' >> /etc/environment; \
    true
FIX
got=$(python3 "$EXTRACT" "$WORK/pos" | cut -f2,3 | tr '\t' ' ')
want "a real printf yields its redirect target and its value ('/etc/environment qt6ct', got '${got}')" \
    test "$got" = "/etc/environment qt6ct"

# Negative 1: the three decoys that a bare grep cannot tell from a setting — a
# config comment, a COPY path, and prose. None of them sets anything.
cat > "$WORK/neg" <<'FIX'
# dark palette; activated via QT_QPA_PLATFORMTHEME=qt6ct (set in /etc/environment).
COPY files/system/qt6ct/qt6ct.conf /etc/xdg/qt6ct/qt6ct.conf
RUN grep -q qt6ct /etc/xdg/qt6ct/qt6ct.conf
FIX
want "the decoys — a comment, a COPY path, a grep for the name — yield nothing" \
    test -z "$(python3 "$EXTRACT" "$WORK/neg")"

# Negative 2: the redirect is removed. The printf still runs and still names the
# variable; it writes to stdout, the build stays green, and the image gets
# nothing. This must read as a MISSING target, not as a match.
cat > "$WORK/noredir" <<'FIX'
RUN printf 'QT_QPA_PLATFORMTHEME=qt6ct\n'; \
    echo done >> /etc/environment
FIX
want "a printf with its redirect removed reports (no-redirect), not the next command's" \
    test "$(python3 "$EXTRACT" "$WORK/noredir" | cut -f2)" = "(no-redirect)"

section "link 1 — the image writes the variable into /etc/environment"

python3 "$EXTRACT" Containerfile.* > "$WORK/rows"
rows=$(grep -c . "$WORK/rows")

want "exactly one Containerfile assignment of QT_QPA_PLATFORMTHEME exists (found ${rows})" \
    test "$rows" = 1

env_target=$(cut -f2 "$WORK/rows" | head -1)
env_value=$(cut -f3 "$WORK/rows" | head -1)
env_file=$(cut -f1 "$WORK/rows" | head -1)
note "${env_file:-(none)} writes QT_QPA_PLATFORMTHEME=${env_value:-(nothing)} to ${env_target:-(nowhere)}"

want "  ...and it is redirected into /etc/environment, where pam_env reads it" \
    test "$env_target" = "/etc/environment"

# The value, asserted as a value. This is the assertion that did not exist
# anywhere in either repository before round 26.
want "  ...and the theme it names is qt6ct, the plugin that installs a QTranslator" \
    test "$env_value" = "qt6ct"

section "link 2 — the labwc session sets the same theme"

# labwc reads this file itself, as KEY=VALUE pairs: no `export`, no shell
# syntax, no expansion. `export FOO=bar` here does not set FOO — it sets a
# variable whose name begins "export ". That is a silent no-op, so it gets its
# own assertion rather than being assumed.
want "the shipped labwc environment file exists" \
    test -s "$LABWC"
want "  ...and contains no shell syntax labwc would take literally" \
    test -z "$(grep -c '^[[:space:]]*export ' "$LABWC" | grep -v '^0$')"

labwc_rows=$(grep -c '^QT_QPA_PLATFORMTHEME=' "$LABWC")
labwc_value=$(sed -n 's/^QT_QPA_PLATFORMTHEME=//p' "$LABWC" | tail -1)
want "exactly one QT_QPA_PLATFORMTHEME assignment in $LABWC (found ${labwc_rows})" \
    test "$labwc_rows" = 1
note "$LABWC sets QT_QPA_PLATFORMTHEME=${labwc_value:-(nothing)}"

# The agreement, which is what makes a coordinated mutant fail too. A build that
# changed one file and not the other gives a desktop that mirrors on two
# compositors and not on the third — the worst shape of this bug, because it
# looks like a labwc problem.
want "  ...and it AGREES with /etc/environment (labwc='${labwc_value}', image='${env_value}')" \
    test -n "$env_value" -a "$labwc_value" = "$env_value"

section "link 3 — the image installs the plugin, and the catalogues it loads"

# Package names only: every comment stripped, every backslash continuation
# joined, then split on `;` so a name in a FATAL message or a COPY path is not
# mistaken for something dnf installs. Two of the three qt6ct decoys live in
# exactly those places.
for f in Containerfile.*; do
    grep -v '^[[:space:]]*#' "$f" \
        | sed -e :a -e '/\\$/N; s/\\\n[[:space:]]*//; ta'
done | tr ';' '\n' | sed -e 's/^[[:space:]]*//' \
     | sed -n 's/^dnf5\? .*[[:space:]]install[[:space:]]\+//p' \
     | tr ' ' '\n' | grep -v '^-' | grep -v '^"' | grep . \
     | sort -u > "$WORK/pkgs"
npkgs=$(grep -c . "$WORK/pkgs")
have_pkg() { grep -qx -- "$1" "$WORK/pkgs"; }

# The control on the extraction. Without it, a pipeline that silently matched
# nothing would report every package below as missing — which is the same
# false red as a gate that inspects nothing is a false green.
want "the package-name extraction found a plausible install set (${npkgs} names)" \
    test "$npkgs" -gt 100

want "the image installs the platform theme plugin the variable names (${env_value})" \
    have_pkg "${env_value:-__none__}"

# The catalogues. Without qt_<lang>.qm the plugin installs a QTranslator that
# has nothing to load, Qt never sees QT_LAYOUT_DIRECTION=RTL, and every mirrored
# surface goes quietly left-to-right. It is a Recommends: of qt6-qtbase-gui and
# nothing requires it, so it must be named explicitly or a weak-deps-off build
# drops it.
want "the image names qt6-qttranslations EXPLICITLY, not as a weak dependency" \
    have_pkg qt6-qttranslations

# The palette half, for completeness: this is what the theme was configured FOR,
# and it is the reason the mechanism looks optional to a reader.
want "the qt6ct configuration the theme reads is COPYed into the image" \
    grep -qE "^COPY +${QTCONF} +/etc/xdg/qt6ct/qt6ct.conf" Containerfile.*

section "the shipped artefacts on this machine, where this is a booted APEX"

if [ ! -e /run/ostree-booted ] || [ ! -r /usr/lib/os-release ] \
   || ! grep -q '^PRETTY_NAME="APEX-OS"' /usr/lib/os-release 2>/dev/null; then
    skp "the live half — this is not a booted APEX host"
    note "Everything above is structural and ran. The rows below need the"
    note "image's own /usr, so they are skipped rather than guessed at."
else
    # /usr/etc/environment, NOT /etc/environment. On an ostree system /usr/etc
    # holds the image's defaults and /etc is the merged, user-writable copy — so
    # reading /etc would measure this laptop's history and call it the product.
    live_img=$(sed -n 's/^QT_QPA_PLATFORMTHEME=//p' /usr/etc/environment 2>/dev/null | tail -1)
    want "the IMAGE default /usr/etc/environment sets QT_QPA_PLATFORMTHEME=${env_value}" \
        test "$live_img" = "$env_value"

    live_etc=$(sed -n 's/^QT_QPA_PLATFORMTHEME=//p' /etc/environment 2>/dev/null | tail -1)
    if [ "$live_etc" = "$live_img" ]; then
        ok "  ...and the merged /etc/environment on this machine agrees with it"
    else
        ok "  ...and the merged /etc/environment differs — a LOCAL edit, not a product defect"
        note "/usr/etc: ${live_img:-(unset)}   /etc: ${live_etc:-(unset)}"
        note "ostree merges /etc, so a local change survives updates and would"
        note "make a suite that read /etc report this machine as the image."
    fi

    PLUGIN=/usr/lib64/qt6/plugins/platformthemes/libqt6ct.so
    if [ -e "$PLUGIN" ]; then
        owner=$(rpm -qf --qf '%{NAME}' "$PLUGIN" 2>/dev/null)
        want "the plugin is present and owned by the image rpm '${env_value}' (owner: ${owner:-none})" \
            test "$owner" = "$env_value"
    else
        bad "the platform theme plugin $PLUGIN is installed"
        note "The variable names a theme with no plugin behind it. Qt falls back"
        note "silently, no QTranslator is installed, and nothing mirrors."
    fi

    QM=/usr/share/qt6/translations/qt_ar.qm
    if [ -e "$QM" ]; then
        qowner=$(rpm -qf --qf '%{NAME}' "$QM" 2>/dev/null)
        want "an RTL catalogue ships and an image rpm owns it (${QM} <- ${qowner:-none})" \
            test "$qowner" = "qt6-qttranslations"
    else
        bad "an RTL catalogue ships ($QM)"
        note "Without it the plugin's QTranslator loads nothing and Qt answers"
        note "LeftToRight for Arabic — the ur_PK control, for every language."
    fi

    # Not an assertion: the control from round 23, re-read off the filesystem.
    # Urdu is right-to-left and upstream ships no qt_ur.qm, so Urdu users get an
    # unmirrored desktop. That is upstream's catalogue set, not an APEX choice,
    # and failing on it would be failing on someone else's release.
    if [ -e /usr/share/qt6/translations/qt_ur.qm ]; then
        note "NOTE: qt_ur.qm now exists upstream — round 23's negative control is"
        note "gone and apex-shell's run-rtl-test.sh needs a new one."
    else
        note "control: qt_ur.qm is absent, so Urdu stays LeftToRight however the"
        note "locale is set. Same mechanism, visible as a product gap."
    fi
fi

printf '\napex-platform-theme: %d passed, %d failed, %d skipped\n' "$pass" "$fail" "$skip"
[ "$fail" -eq 0 ]
