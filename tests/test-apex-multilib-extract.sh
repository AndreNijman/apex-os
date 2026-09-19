#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  test-apex-multilib-extract.sh — extract a REAL multilib set with the shipped
#  engine and assert that nothing 32-bit lands anywhere the image keeps its own
#  executables.
#
#  ── Why this exists next to test-apex-multilib.sh ───────────────────────────
#  That suite stops at guard_rpms on purpose, and says so: extracting a full set
#  through the normal path needs the container's installed versions to match the
#  repository, and when they do not the guard correctly refuses the transaction.
#  So the decisions are the discriminating step there. This defect is not in the
#  decisions. Every decision was right on katana on 2026-09-19 and the machine
#  still broke, because the second (32-bit) pass of extract_rpms excluded
#  /usr/bin, /usr/sbin, /usr/share and /etc and nothing else — while i686
#  packages also ship helper executables under /usr/libexec.
#
#  What that cost, measured: at-spi2-core.i686, dconf.i686, glib2.i686,
#  glib-networking.i686, glycin-loaders.i686, gstreamer1.i686 and p11-kit.i686
#  are all in steam.i686's dependency closure, so a plain `apex install steam`
#  shadowed seven image helpers. gst-plugin-scanner is the one that bites:
#  GStreamer probes every plugin through it out of process, so a 32-bit scanner
#  loads no 64-bit plugin and the registry fell from 1344 usable features to 2.
#
#  extract_rpms is called DIRECTLY here, not through the install path, which is
#  what lets this run where the fuller suite would be refused: --nodeps
#  --replacefiles --replacepkgs into a throwaway root does not care whether the
#  container's versions match the repository's.
#
#  ── The assertion is a discovery, not a list ────────────────────────────────
#  It does not check that /usr/libexec specifically is clean. It finds every
#  32-bit ELF file in the extracted tree and requires each one to live in a
#  library directory. A future i686 package that ships executables somewhere new
#  fails this too, which is the whole point — the list in the engine is the
#  thing under test and a test that repeats it proves nothing.
#
#  ── And it cannot go blind ──────────────────────────────────────────────────
#  A negative control extracts the SAME set a second time with the pre-fix
#  exclude list and requires it to produce a violation. If Fedora ever stops
#  shipping 32-bit helpers in these packages, the control fails and says the
#  instrument has gone blind, instead of the main assertion passing for free.
#
#  Run from anywhere: ./tests/test-apex-multilib-extract.sh
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail
cd "$(dirname "$0")/.." || exit 2

ENGINE=files/system/libexec/apex-pkg
[ -f "$ENGINE" ] || { echo "cannot find $ENGINE"; exit 2; }

if ! command -v podman >/dev/null 2>&1; then
    echo "SKIP  podman is absent; this suite needs a real dnf and repository"
    exit 0
fi

IMAGE=${APEX_MULTILIB_IMAGE:-registry.fedoraproject.org/fedora:43}
PROBE=$(mktemp) || exit 2
trap 'rm -f "$PROBE"' EXIT

cat > "$PROBE" <<'PROBE_EOF'
set -uo pipefail
PKG=/repo/files/system/libexec/apex-pkg
WORK=/tmp/x; rm -rf "$WORK"; mkdir -p "$WORK/dl"

# gstreamer1 is the package the katana defect was measured through, and
# at-spi2-core is the one that left the greeter on a 32-bit accessibility bus.
# Both ship an i686 build carrying /usr/libexec.
dnf5 -y download --resolve --arch=x86_64 --arch=noarch --arch=i686 \
    --destdir "$WORK/dl" gstreamer1 at-spi2-core >/dev/null 2>&1 \
    || { echo "PROBE_SKIP no repository reachable"; exit 0; }

echo "PROBE_SET $(ls "$WORK/dl" | wc -l) rpms, $(ls "$WORK/dl" | grep -c i686) i686"

# ELF class without `file(1)`, which the base image does not ship: byte 4 of an
# ELF header is EI_CLASS, 1 = 32-bit, 2 = 64-bit.
is_elf32() {
    local hdr
    hdr="$(od -An -tu1 -N5 -- "$1" 2>/dev/null | tr -s ' ')" || return 1
    [ "$(echo "$hdr" | cut -d' ' -f2-5)" = "127 69 76 70" ] || return 1
    [ "$(echo "$hdr" | cut -d' ' -f6)" = "1" ]
}

# Every 32-bit ELF in a tree, as absolute paths inside it, tagged OK when it
# sits in a library directory and BAD when it does not.
report_tree() {
    local root="$1" tag="$2" f rel
    while IFS= read -r -d '' f; do
        is_elf32 "$f" || continue
        rel="${f#"$root"}"
        case "$rel" in
            /usr/lib/*|/lib/*) echo "$tag OK $rel" ;;
            *)                 echo "$tag BAD $rel" ;;
        esac
    done < <(find "$root" -type f -print0 2>/dev/null)
}

# ── The engine, as shipped ──────────────────────────────────────────────────
# shellcheck disable=SC1090
source "$PKG" >/dev/null 2>&1
set +e
extract_rpms "$WORK/dl" "$WORK/root" >/dev/null 2>&1
echo "PROBE_EXTRACT_RC $?"
report_tree "$WORK/root" PROBE_ENGINE

# The non-ELF half of the same defect. An i686 package also ships unit files,
# udev rules, tmpfiles and sysusers text, and those cannot be found by looking
# at ELF headers. Ask the packages instead: any path that ONLY an i686 rpm
# ships, landing anywhere but a library directory, was carried in by the 32-bit
# pass. Paths a native rpm also ships are excluded because the native pass
# places those legitimately and the tree cannot say which pass won.
{
    for f in "$WORK/dl"/*.rpm; do
        a="$(rpm -qp --nosignature --qf '%{ARCH}' -- "$f" 2>/dev/null)"
        case "$a" in
            x86_64|noarch) tag=N ;;
            *)             tag=F ;;
        esac
        rpm -qlp --nosignature -- "$f" 2>/dev/null | sed "s/^/$tag /"
    done
} > "$WORK/owned.txt"
awk '$1=="N"{n[$2]=1} $1=="F"{f[$2]=1} END{for (p in f) if (!(p in n)) print p}' \
    "$WORK/owned.txt" > "$WORK/foreign-only.txt"
echo "PROBE_FOREIGN_ONLY $(wc -l < "$WORK/foreign-only.txt")"
while IFS= read -r rel; do
    case "$rel" in /usr/lib/*|/lib/*) continue ;; esac
    [ -f "$WORK/root$rel" ] && echo "PROBE_CARRIED $rel"
done < "$WORK/foreign-only.txt"

# ── Negative control: the pre-fix exclude list, on the same set ─────────────
# A replica of the command as it stood before /usr/libexec was added, and its
# only job is to prove this probe can SEE a shadow on today's packages.
mkdir -p "$WORK/ctl"
rpm --root "$WORK/ctl" --initdb >/dev/null 2>&1
mapfile -t ctl_foreign < <(
    for f in "$WORK/dl"/*.rpm; do
        a="$(rpm -qp --nosignature --qf '%{ARCH}' -- "$f" 2>/dev/null)"
        [ "$a" = x86_64 ] || [ "$a" = noarch ] || echo "$f"
    done
)
if [ "${#ctl_foreign[@]}" -gt 0 ]; then
    rpm --root "$WORK/ctl" -Uvh --nodeps --noscripts --notriggers --noplugins \
        --nosignature --replacefiles --replacepkgs --excludepath /boot \
        --excludepath /usr/bin --excludepath /usr/sbin \
        --excludepath /usr/share --excludepath /etc \
        "${ctl_foreign[@]}" >/dev/null 2>&1
    report_tree "$WORK/ctl" PROBE_CONTROL
else
    echo "PROBE_CONTROL NONE no-foreign-rpms"
fi
PROBE_EOF

out=$(podman run --rm --privileged \
        -v "$PWD":/repo:ro,Z -v "$PROBE":/probe.sh:ro,Z \
        "$IMAGE" bash /probe.sh 2>&1)

if printf '%s\n' "$out" | grep -q PROBE_SKIP; then
    echo "SKIP  $(printf '%s\n' "$out" | grep PROBE_SKIP | sed 's/PROBE_SKIP //')"
    exit 0
fi

pass=0; fail=0
ok()  { printf 'PASS  %s\n' "$1"; pass=$((pass+1)); }
bad() { printf 'FAIL  %s\n' "$1"; fail=$((fail+1)); }

printf '%s\n' "$out" | grep PROBE_SET | sed 's/PROBE_SET/      set:/'

rc=$(printf '%s\n' "$out" | grep PROBE_EXTRACT_RC | awk '{print $2}')
engine_ok=$(printf  '%s\n' "$out" | grep -c '^PROBE_ENGINE OK ')
engine_bad=$(printf '%s\n' "$out" | grep -c '^PROBE_ENGINE BAD ')
ctl_bad=$(printf    '%s\n' "$out" | grep -c '^PROBE_CONTROL BAD ')
printf '      extract rc %s; 32-bit ELF in tree: %s in a library dir, %s outside\n' \
       "${rc:-?}" "$engine_ok" "$engine_bad"

if [ "${rc:-1}" = 0 ]; then
    ok "extract_rpms completed on a real multilib set"
else
    bad "extract_rpms failed (rc ${rc:-unknown}), so nothing below was measured"
fi

# Can-only-pass guard. An empty tree satisfies "nothing is misplaced" without
# testing anything, and this set contains i686 libraries by construction.
if [ "$engine_ok" -gt 0 ]; then
    ok "the set really did carry 32-bit libraries into the tree ($engine_ok)"
else
    bad "no 32-bit ELF reached the tree at all, so the placement rule was never exercised"
fi

if [ "$engine_bad" -eq 0 ]; then
    ok "every 32-bit ELF landed in a library directory"
else
    bad "$engine_bad 32-bit ELF file(s) landed outside a library directory:"
    printf '%s\n' "$out" | grep '^PROBE_ENGINE BAD ' | sed 's/^PROBE_ENGINE BAD /        /' | head -12
fi

fonly=$(printf '%s\n' "$out" | grep '^PROBE_FOREIGN_ONLY ' | awk '{print $2}')
carried=$(printf '%s\n' "$out" | grep -c '^PROBE_CARRIED ')
printf '      paths only an i686 rpm ships: %s; of those, outside a library dir and present: %s\n' \
       "${fonly:-?}" "$carried"
if [ "${fonly:-0}" -gt 0 ]; then
    ok "the set contains i686-only paths, so this assertion had something to find"
else
    bad "no i686-only paths in the set at all, so the non-ELF half was never exercised"
fi
if [ "$carried" -eq 0 ]; then
    ok "no i686-only path landed outside a library directory"
else
    bad "$carried i686-only path(s) landed outside a library directory:"
    printf '%s\n' "$out" | grep '^PROBE_CARRIED ' | sed 's/^PROBE_CARRIED /        /' | head -12
fi

# The control proves the instrument, every run, on today's packages.
if [ "$ctl_bad" -gt 0 ]; then
    ok "negative control: the pre-fix exclude list still produces $ctl_bad shadow(s), so this suite can see one"
else
    bad "negative control produced NO shadow, so this suite is blind and its passes above mean nothing"
fi

echo
printf 'apex-multilib-extract: %d passed, %d failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ]
