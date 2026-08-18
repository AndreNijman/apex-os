#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  test-apex-pkg.sh — assertions against the SHIPPED package engine,
#  files/system/libexec/apex-pkg. Nothing here re-implements it: every case
#  either runs the script as a process or sources it and calls the very function
#  the image runs.
#
#  ── Why this file exists ────────────────────────────────────────────────────
#  `apex install /path/to/some.rpm` added a second kind of argument to a command
#  that previously took only package names, and the classifier that tells them
#  apart is a policy decision with two sharp edges:
#
#    1. The Flatpak rule matches `org.foo.Bar.rpm`. If the file test does not run
#       FIRST, a file the user pointed at is silently sent to Flathub, which then
#       fails with a message about an application id the user never typed.
#    2. The package NAME read out of an untrusted RPM header is used to build a
#       path under /var/lib/apex/pkg/local, by a script running as root. A name
#       like `../../…` must be refused, not concatenated.
#
#  On top of that sits the signature policy. APEX refuses any RPM it cannot
#  verify; the ONLY way past that is `--allow-unsigned` for a named file, and the
#  refusal has to tell the user exactly that. A regression there is not a crash —
#  it is a machine that installs unverified software quietly, or one that refuses
#  a legitimate vendor RPM with no way forward. Both are silent until someone
#  reads the source, so they are pinned here instead.
#
#  ── What it deliberately does NOT do ────────────────────────────────────────
#  No root, no network, no writes outside a temp directory, no extension is ever
#  built or merged. Every case below is reached BEFORE the engine's root gate or
#  is a pure function. Cases that need `rpm` to read a header are skipped, out
#  loud, where rpm is absent, so the suite is meaningful on a plain CI runner and
#  more thorough on a Fedora one.
#
#  PASS = every case prints the exact refusal it should, with a non-zero exit
#         where one is expected, and no case reports an unexpected shell error.
#
#  Run from anywhere: ./tests/test-apex-pkg.sh
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail
cd "$(dirname "$0")" || exit 2

ENGINE=../files/system/libexec/apex-pkg
[ -f "$ENGINE" ] || { echo "cannot find $ENGINE"; exit 2; }

WORK=$(mktemp -d /tmp/apex-pkg-test.XXXXXX)
trap 'rm -rf "$WORK"' EXIT

pass=0; fail=0; skip=0

ok()      { printf 'PASS  %-42s\n' "$1"; pass=$((pass+1)); }
bad()     { printf 'FAIL  %-42s %s\n' "$1" "$2"; fail=$((fail+1)); }
skipped() { printf 'SKIP  %-42s %s\n' "$1" "$2"; skip=$((skip+1)); }

# Run the shipped engine as a process. $1 = case name, $2 = expected substring
# anywhere in its output, rest = argv. A refusal must also exit non-zero: a
# message with exit 0 would let `apex update` carry on as if nothing was wrong.
refuses() {
    local name=$1 want=$2; shift 2
    local out rc
    out=$(bash "$ENGINE" "$@" 2>&1 </dev/null); rc=$?
    if [ "$rc" = 0 ]; then
        bad "$name" "exited 0; expected a refusal"
        return
    fi
    if grep -qF -- "$want" <<<"$out"; then ok "$name"
    else bad "$name" "expected $(printf '%q' "$want"), got: $(head -1 <<<"$out")"; fi
}

# Same, but the expected text must NOT appear. Used where the danger is the
# engine taking a different route rather than failing.
not_refuses_with() {
    local name=$1 unwanted=$2; shift 2
    local out
    out=$(bash "$ENGINE" "$@" 2>&1 </dev/null)
    if grep -qF -- "$unwanted" <<<"$out"; then
        bad "$name" "took the wrong route: $(head -1 <<<"$out")"
    else ok "$name"; fi
}

# Source the shipped engine and call one of its functions. Sourcing runs its
# main() with NO arguments on purpose — that prints usage and returns 0, leaving
# every function and constant defined exactly as the image has them.
call() {
    bash -c '
        e=$1; f=$2; shift 2; a=("$@"); set --
        source "$e" >/dev/null 2>&1
        set +e
        "$f" "${a[@]}"
    ' _ "$ENGINE" "$@"
}
predicate() {  # prints true/false for a shipped predicate
    bash -c '
        e=$1; f=$2; shift 2; a=("$@"); set --
        source "$e" >/dev/null 2>&1
        set +e
        if "$f" "${a[@]}"; then echo true; else echo false; fi
    ' _ "$ENGINE" "$@"
}
is() {
    local name=$1 want=$2 got=$3
    if [ "$got" = "$want" ]; then ok "$name"
    else bad "$name" "expected $(printf '%q' "$want"), got $(printf '%q' "$got")"; fi
}

# ── fixtures ────────────────────────────────────────────────────────────────
printf 'PK\003\004 this is a zip, not an rpm' > "$WORK/notanrpm.rpm"
mkdir -p "$WORK/adirectory.rpm"
: > "$WORK/empty.rpm"
printf '\355\253\356\333garbage after a valid lead' > "$WORK/badheader.rpm"
printf 'PK\003\004' > "$WORK/a file with spaces.rpm"
printf 'PK\003\004' > "$WORK/unreadable.rpm"; chmod 000 "$WORK/unreadable.rpm"

echo "── argument routing: a file must beat the Flatpak rule ────────────────"
# is_flatpak_id matches org.foo.Bar.rpm. Testing it before the file rule would
# send a file the user pointed at to Flathub.
is "org.foo.Bar.rpm is a file"      true  "$(predicate is_local_rpm_arg org.foo.Bar.rpm)"
is "…and also matches the flatpak rule" true "$(predicate is_flatpak_id org.foo.Bar.rpm)"
refuses "so the engine treats it as a file" "no such file: org.foo.Bar.rpm" \
        install org.foo.Bar.rpm
is "a bare reverse-DNS id is not a file" false "$(predicate is_local_rpm_arg org.gimp.GIMP)"
if [ "$(id -u)" = 0 ]; then
    # Running this as root would reach flatpak_install and could really install
    # something, which a test must never do.
    skipped "org.gimp.GIMP stays a Flatpak" "(would install as root)"
else
    not_refuses_with "org.gimp.GIMP stays a Flatpak" "no such file" \
            install org.gimp.GIMP
    refuses "…and stops at the root gate instead" "this needs root" \
            install org.gimp.GIMP
fi

# A bare package name must not become a file just because something with that
# name happens to sit in the working directory.
is "plain name is not a file"       false "$(predicate is_local_rpm_arg htop)"
is "python3.12 is not a file"       false "$(predicate is_local_rpm_arg python3.12)"
is "python3.12 is not a Flatpak"    false "$(predicate is_flatpak_id python3.12)"
is "a path is always a file"        true  "$(predicate is_local_rpm_arg /media/usb/x)"
is "a relative path is a file"      true  "$(predicate is_local_rpm_arg ./x.rpm)"
is "a requested-list id is neither" false "$(predicate is_local_rpm_arg local:chrome)"
is "…and not a Flatpak either"      false "$(predicate is_flatpak_id local:chrome)"

echo "── file validation: every refusal names the file and the reason ───────"
refuses "missing file"          "no such file: /nonexistent/apex-test.rpm" \
        install /nonexistent/apex-test.rpm
refuses "directory named *.rpm" "not a regular file: ${WORK}/adirectory.rpm" \
        install "${WORK}/adirectory.rpm"
refuses "not an RPM at all"     "is not an RPM package" \
        install "${WORK}/notanrpm.rpm"
refuses "empty file"            "is not an RPM package" \
        install "${WORK}/empty.rpm"
# The path is echoed back verbatim, which is what proves it survived word
# splitting on its way through the engine.
refuses "a path containing spaces" "${WORK}/a file with spaces.rpm is not an RPM package" \
        install "${WORK}/a file with spaces.rpm"

if [ "$(id -u)" = 0 ]; then
    skipped "unreadable file" "(root can read anything)"
else
    refuses "unreadable file" "permission denied" install "${WORK}/unreadable.rpm"
fi

if command -v rpm >/dev/null 2>&1; then
    # Correct lead magic, nothing behind it: the magic check passes and rpm's own
    # header read is what must reject it.
    refuses "RPM magic but no header" "cannot read the RPM header" \
            install "${WORK}/badheader.rpm"
else
    skipped "RPM magic but no header" "(no rpm on this machine)"
fi

echo "── package names out of an untrusted header build root-owned paths ────"
is "ordinary name"          true  "$(predicate valid_pkg_name google-chrome-stable)"
is "name with . + _ and +"  true  "$(predicate valid_pkg_name ok_1.2+x)"
is "traversal"              false "$(predicate valid_pkg_name ../../etc/passwd)"
is "embedded slash"         false "$(predicate valid_pkg_name a/b)"
is "leading dash"           false "$(predicate valid_pkg_name -rf)"
is "empty"                  false "$(predicate valid_pkg_name '')"
is "newline"                false "$(predicate valid_pkg_name 'a
b')"

echo "── cache identifiers must round-trip ──────────────────────────────────"
# The requested list is the only thing that survives a reboot, an OS upgrade and
# an `apex update`. If `local:<NAME>` does not map back to exactly one cached
# file, a rebuild either loses the package or reads the wrong one.
is "local_id"              "local:chrome" "$(call local_id chrome)"
is "local_name inverts it" "chrome"       "$(call local_name local:chrome)"
is "id survives two trips" "local:chrome" "$(call local_id "$(call local_name local:chrome)")"
is "is_local_id on an id"  true           "$(predicate is_local_id local:chrome)"
is "is_local_id on a name" false          "$(predicate is_local_id chrome)"
is "is_local_id on 'local:'" false        "$(predicate is_local_id 'local:')"
is "cache path"  "/var/lib/apex/pkg/local/chrome.rpm"   "$(call local_cache local:chrome)"
is "marker path" "/var/lib/apex/pkg/local/chrome.trust" "$(call local_marker local:chrome)"
# The cache path must be derivable from the bare name too — that is the form
# `apex remove chrome` and the state file carry.
is "cache path from a bare name" "/var/lib/apex/pkg/local/chrome.rpm" \
   "$(call local_cache chrome)"

echo "── remove matches by package name, not by the path used to install ────"
is "name matches its local: entry" true  "$(predicate requested_matches local:chrome chrome)"
is "id matches itself"             true  "$(predicate requested_matches local:chrome local:chrome)"
is "repo entry matches its name"   true  "$(predicate requested_matches htop htop)"
is "no cross-matching"             false "$(predicate requested_matches htop chrome)"
is "a prefix is not a match"       false "$(predicate requested_matches local:chromium chrome)"
is "the path is not the entry"     false "$(predicate requested_matches local:chrome ./chrome.rpm)"

# Round-trip through a file shaped exactly like the requested list, including a
# comment and a blank line, which load_requested has to drop.
{ printf '# apex requested packages\n\n'; printf 'htop\nlocal:chrome\n'; } > "$WORK/requested"
mapfile -t entries < <(grep -vE '^\s*(#|$)' "$WORK/requested")
matched=""
for e in "${entries[@]}"; do
    [ "$(predicate requested_matches "$e" chrome)" = true ] && matched="$e"
done
is "list round-trip finds the entry" "local:chrome" "$matched"

echo "── signature policy: refusal is closed, and it tells the user what to do"
is "no marker means not trusted" false \
   "$(predicate trusted_unsigned chrome "$WORK/notanrpm.rpm")"

unsigned_msg=$(bash -c '
    e=$1; set --
    source "$e" >/dev/null 2>&1
    RPM_SIGSTATUS="foo.rpm: DIGESTS SIGNATURES NOT OK"
    refuse_unsigned "/media/usb/foo.rpm"
' _ "$ENGINE" 2>&1); unsigned_rc=$?
is "refusal exits non-zero" 1 "$unsigned_rc"
for want in 'cannot verify /media/usb/foo.rpm' \
            'DIGESTS SIGNATURES NOT OK' \
            'sudo apex install --allow-unsigned /media/usb/foo.rpm' \
            'apex repo enable-copr'; do
    if grep -qF -- "$want" <<<"$unsigned_msg"; then ok "refusal mentions: $want"
    else bad "refusal mentions: $want" "not in the message"; fi
done

# The opt-in must be off on every run. It is a per-file decision recorded under
# LOCAL_DIR, never a mode the engine remembers.
is "--allow-unsigned defaults to off" 0 \
   "$(bash -c 'e=$1; set --; source "$e" >/dev/null 2>&1; echo "$ALLOW_UNSIGNED"' _ "$ENGINE")"
# …and it must be consumed as a flag, not mistaken for a package to install.
refuses "the flag is not a package" "no such file: /nonexistent/apex-test.rpm" \
        install --allow-unsigned /nonexistent/apex-test.rpm

echo "── the engine advertises the local-file form ──────────────────────────"
help=$(bash "$ENGINE" --help 2>&1)
for want in 'file.rpm' '--allow-unsigned' '/var/lib/apex/pkg/local'; do
    if grep -qF -- "$want" <<<"$help"; then ok "--help mentions: $want"
    else bad "--help mentions: $want" "absent"; fi
done

echo
printf 'apex-pkg: %d passed, %d failed, %d skipped\n' "$pass" "$fail" "$skip"
[ "$fail" = 0 ]
