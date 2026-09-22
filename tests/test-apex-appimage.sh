#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  test-apex-appimage.sh — assertions against the AppImage half of the SHIPPED
#  package engine, files/system/libexec/apex-pkg. Nothing here re-implements
#  it: every case either runs the script as a process or sources it and calls
#  the very function the image runs.
#
#  ── Why this file exists ────────────────────────────────────────────────────
#  `apex install ./Thing.AppImage` is a third kind of argument to a command
#  that already had two, and the thing it does with that argument is unpack an
#  untrusted archive as root and put files on PATH. Four edges are sharp enough
#  that a regression in any of them is silent:
#
#    1. ROUTING. is_local_rpm_arg answers true for ANY argument containing a
#       slash and is_flatpak_id matches `org.foo.Bar.AppImage`. Ask either
#       first and the AppImage is refused as "not an RPM package" or sent to
#       Flathub.
#    2. FUSE. libfuse.so.2 is absent from the APEX image — measured: no
#       libfuse.so.2, no fusermount, no squashfuse. The whole design rests on
#       never needing it: the payload offset is arithmetic on the ELF header
#       and unsquashfs does the rest, so the AppImage is never EXECUTED. The
#       fixtures here are mode 0644 for exactly that reason. An engine that
#       went back to `--appimage-extract-and-run` fails on the execute bit.
#    3. PRIVILEGE. A squashfs records modes and uids. A FUSE-mounted AppImage
#       is mounted nosuid, so preserving a 4755 helper out of a download into a
#       root-owned tree would grant MORE than running it normally ever does,
#       and a payload recording uid 1000 would leave the desktop user able to
#       rewrite the application under itself.
#    4. SHADOWING. /usr/local/bin comes before /usr/bin on PATH and
#       /usr/local/share before /usr/share in XDG_DATA_DIRS. apex-pkg decides
#       image ownership by asking the rpmdb, which has no answer for an
#       AppImage — so the question is asked about the path the install would
#       HIDE, and that guard is what stops `apex install ./firefox.AppImage`
#       taking over the browser for every user on the machine.
#
#  ── The two legs ────────────────────────────────────────────────────────────
#  LEG A runs unprivileged, always, and covers everything above by calling the
#  shipped functions directly — they take their prefix, system root and
#  registry as PARAMETERS precisely so this is possible without root.
#
#  LEG B re-executes this script inside `unshare -rm` with tmpfs over
#  /var/lib/apex and the /usr/local tree, and runs the shipped engine as a
#  PROCESS end to end: install, list, verify, upgrade, remove. Nothing escapes
#  that namespace — the mounts are private and vanish with it — so the live
#  machine's /usr/local and /var/lib/apex are never written. It skips, loudly
#  and naming what went unchecked, where unprivileged user namespaces are not
#  available.
#
#  PASS = every case prints the exact refusal it should with a non-zero exit
#         where one is expected, every file the engine creates has the content
#         and the mode asserted here, and no case reports a shell error.
#
#  Run from anywhere: ./tests/test-apex-appimage.sh
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail
# `set +e` for the same reason as every other suite here: this one COUNTS
# failures instead of aborting, and many assertions run commands that exit
# non-zero on purpose. GitHub Actions invokes a script as `bash -e {0}`.
set +e

LEG=${1:-A}
if [ "$LEG" = --leg-b ]; then
    WORK=$2
    ENGINE=$3
else
    SELF="$(cd "$(dirname "$0")" && pwd)/$(basename "$0")"
    cd "$(dirname "$0")" || exit 2
    ENGINE="$(cd .. && pwd)/files/system/libexec/apex-pkg"
    [ -f "$ENGINE" ] || { echo "cannot find $ENGINE"; exit 2; }
    WORK=$(mktemp -d "${TMPDIR:-/tmp}/apex-appimage-test.XXXXXX")
    trap 'rm -rf "$WORK"' EXIT
fi

pass=0; fail=0; skip=0
ok()      { printf 'PASS  %-52s\n' "$1"; pass=$((pass+1)); }
bad()     { printf 'FAIL  %-52s %s\n' "$1" "$2"; fail=$((fail+1)); }
skipped() { printf 'SKIP  %-52s %s\n' "$1" "$2"; skip=$((skip+1)); }

is() {
    local name=$1 want=$2 got=$3
    if [ "$got" = "$want" ]; then ok "$name"
    else bad "$name" "expected $(printf '%q' "$want"), got $(printf '%q' "$got")"; fi
}
has() {   # $1 name, $2 needle, $3 haystack
    if grep -qF -- "$2" <<<"$3"; then ok "$1"
    else bad "$1" "expected to find $(printf '%q' "$2") in: $(head -3 <<<"$3" | tr '\n' '|')"; fi
}
hasnt() {
    if grep -qF -- "$2" <<<"$3"; then bad "$1" "found $(printf '%q' "$2") and should not have"
    else ok "$1"; fi
}

# Run the shipped engine as a process. A refusal must also exit non-zero: a
# message with exit 0 would let `apex update` carry on as if nothing was wrong.
refuses() {
    local name=$1 want=$2; shift 2
    local out rc
    out=$(bash "$ENGINE" "$@" 2>&1 </dev/null); rc=$?
    if [ "$rc" = 0 ]; then bad "$name" "exited 0; expected a refusal"; return; fi
    if grep -qF -- "$want" <<<"$out"; then ok "$name"
    else bad "$name" "expected $(printf '%q' "$want"), got: $(head -2 <<<"$out" | tr '\n' '|')"; fi
}

# Source the shipped engine and run a snippet against it. `set --` before the
# source is load-bearing: the engine ends in `main "$@"`, and without it the
# engine would run with this helper's own arguments as its command line.
snippet() {
    local f
    f="$(mktemp "${WORK}/snip.XXXXXX")"
    cat > "$f"
    WORK="$WORK" bash -c '
        e=$1; s=$2; set --
        source "$e" >/dev/null 2>&1
        set +e
        source "$s"
    ' _ "$ENGINE" "$f" 2>&1
    rm -f "$f"
}
predicate() { # prints true/false for a shipped predicate
    local fn=$1; shift
    local args; args="$(printf '%q ' "$@")"
    snippet <<EOF
if $fn $args; then echo true; else echo false; fi
EOF
}
callfn() {
    local fn=$1; shift
    local args; args="$(printf '%q ' "$@")"
    snippet <<EOF
$fn $args
EOF
}

# ── fixture builders ────────────────────────────────────────────────────────
need_tool() {
    command -v "$1" >/dev/null 2>&1 && return 0
    echo "FATAL: this suite needs '$1' and it is not installed." >&2
    echo "       It builds its own AppImages rather than downloading any, so" >&2
    echo "       without it every extraction assertion would silently skip —" >&2
    echo "       which is the failure mode this file exists to prevent." >&2
    exit 2
}
need_tool mksquashfs
need_tool unsquashfs
need_tool od

# A little-endian integer, $1 wide $2 bytes, on stdout.
le() { local v=$1 n=$2 i; for ((i=0;i<n;i++)); do printf "\\$(printf '%03o' $((v & 255)))"; v=$((v>>8)); done; }

# A synthetic AppImage runtime: a real ELF64 header whose section-header table
# is placed so that e_shoff + e_shentsize*e_shnum lands exactly on the end of
# the runtime, which is where the payload begins. That is the same arithmetic
# `--appimage-offset` performs and the same number the engine computes.
#   mk_runtime <out> <size> <ai-type-byte> <e_machine> [elf-class] [elf-data]
mk_runtime() {
    local out=$1 rsize=$2 aitype=$3 mach=$4 cls=${5:-2} dat=${6:-1}
    local shnum=3 shentsize=64 shoff=$(( rsize - 3*64 ))
    {
        printf '\177ELF'
        le "$cls" 1; le "$dat" 1; le 1 1; le 0 1
        if [ "$aitype" = 0 ]; then le 0 1; le 0 1; le 0 1   # a plain ELF, no AI magic
        else le 65 1; le 73 1; le "$aitype" 1; fi
        le 0 1; le 0 1; le 0 1; le 0 1; le 0 1
        le 3 2; le "$mach" 2; le 1 4; le 0 8; le 64 8
        le "$shoff" 8
        le 0 4; le 64 2; le 56 2; le 1 2; le "$shentsize" 2; le "$shnum" 2; le 0 2
    } > "$out"
    dd if=/dev/zero bs=1 count=$(( rsize - 64 )) status=none >> "$out"
}

#   mk_appimage <out> <payload-dir> [ai-type] [machine] [runtime-size]
# The result is mode 0644 ON PURPOSE. It is never executed, and a build of this
# engine that went back to running the vendor runtime would fail right here.
mk_appimage() {
    local out=$1 dir=$2 aitype=${3:-2} mach=${4:-} rsize=${5:-4096} sq
    if [ -z "$mach" ]; then
        case "$(uname -m)" in
            x86_64) mach=62 ;; aarch64) mach=183 ;; riscv64) mach=243 ;; armv7l) mach=40 ;; *) mach=62 ;;
        esac
    fi
    sq="$(mktemp "${WORK}/sq.XXXXXX")"
    # -all-root is what appimagetool does, and it is also what lets LEG B run
    # inside a user namespace where only uid 0 is mapped.
    mksquashfs "$dir" "$sq" -noappend -no-progress -quiet -all-root >/dev/null 2>&1 || return 1
    mk_runtime "${out}.rt" "$rsize" "$aitype" "$mach"
    cat "${out}.rt" "$sq" > "$out"
    rm -f "${out}.rt" "$sq"
    chmod 0644 "$out"
}

# The payload of a plausible Electron-ish application.
mk_payload() {
    local d=$1
    mkdir -p "$d/usr/share/icons/hicolor/256x256/apps" "$d/usr/share/icons/hicolor/scalable/apps" "$d/usr/bin"
    printf '#!/bin/sh\necho "AppRun ran: APPDIR=$APPDIR APPIMAGE=$APPIMAGE"\n' > "$d/AppRun"
    chmod 0755 "$d/AppRun"
    cat > "$d/hello.desktop" <<'DESK'
[Desktop Entry]
Type=Application
Name=Hello
Exec=AppRun --no-sandbox %U
Icon=hello
TryExec=AppRun
DBusActivatable=true
Categories=Utility;
Actions=new-window;

[Desktop Action new-window]
Name=New Window
Exec=AppRun --new-window
DESK
    printf '\211PNG\r\n\032\nbig-icon' > "$d/usr/share/icons/hicolor/256x256/apps/hello.png"
    printf '<svg/>' > "$d/usr/share/icons/hicolor/scalable/apps/hello.svg"
    # Two things a real payload ships that must not survive as they are: a
    # setuid helper, and a world-writable directory.
    printf '#!/bin/sh\n:\n' > "$d/chrome-sandbox"; chmod 4755 "$d/chrome-sandbox"
    chmod 0777 "$d/usr/bin"
    # …and one a FUSE mount would have let the user read anyway, because that
    # mount enforces no modes. Handed to root unopened it would stop working.
    printf 'resource\n' > "$d/private.dat"; chmod 0600 "$d/private.dat"
    ln -sf usr/share/icons/hicolor/256x256/apps/hello.png "$d/.DirIcon"
}

# ─────────────────────────────────────────────────────────────────────────────
# LEG B — the shipped engine, as a process, as root, end to end.
# Re-entered through `unshare -rm` from leg A below. Everything it touches is
# on a tmpfs inside a private mount namespace that dies with the process.
# ─────────────────────────────────────────────────────────────────────────────
if [ "$LEG" = --leg-b ]; then
    echo "── LEG B: the shipped engine, as root, in a private namespace ─────────"
    mount --make-rprivate / 2>/dev/null
    # /var/lib/apex exists on an APEX machine and does not on a CI runner,
    # where a user namespace grants no DAC write on host-owned /var/lib either.
    # So take /var/lib itself when the directory cannot be made.
    mkdir -p /var/lib/apex 2>/dev/null \
        || { mount -t tmpfs tmpfs /var/lib && mkdir -p /var/lib/apex; } \
        || { echo "leg B: cannot create /var/lib/apex"; exit 3; }
    mount -t tmpfs tmpfs /var/lib/apex || { echo "leg B: cannot mount tmpfs on /var/lib/apex"; exit 3; }
    # /usr/local is a symlink to ../var/usrlocal on APEX and a real directory
    # everywhere else; mount over whichever this machine has.
    if [ -d /var/usrlocal ]; then LOCALMNT=/var/usrlocal; else LOCALMNT=/usr/local; fi
    mkdir -p "$LOCALMNT" 2>/dev/null || true
    mount -t tmpfs tmpfs "$LOCALMNT" || { echo "leg B: cannot mount tmpfs on ${LOCALMNT}"; exit 3; }

    FIX="$WORK/legb"; mkdir -p "$FIX/payload"
    mk_payload "$FIX/payload"
    mk_appimage "$FIX/Hello.AppImage" "$FIX/payload" || { echo "leg B: cannot build the fixture"; exit 3; }

    # A stub that records every chown the engine makes, then does it for real.
    # This is how the ownership arm is proved rather than assumed: it runs only
    # when euid is 0, which is exactly the case leg A cannot reach.
    mkdir -p "$WORK/stub"
    cat > "$WORK/stub/chown" <<STUB
#!/bin/sh
printf '%s\n' "\$*" >> "$WORK/chown.log"
exec /usr/bin/chown "\$@"
STUB
    chmod 0755 "$WORK/stub/chown"
    # ...and stubs for every FUSE entry point, which must never be reached.
    for t in fusermount fusermount3 squashfuse; do
        printf '#!/bin/sh\ntouch "%s/fuse.used"\nexit 1\n' "$WORK" > "$WORK/stub/$t"
        chmod 0755 "$WORK/stub/$t"
    done
    export PATH="$WORK/stub:$PATH"

    # Nothing is installed without the flag, and the refusal happens here —
    # past the root gate, which is the only place leg A cannot reach it.
    refuses "no AppImage installs unverified" "cannot verify" install "$FIX/Hello.AppImage"
    is "…and nothing was written"             gone "$([ -e /var/lib/apex/appimage ] && echo present || echo gone)"

    # An end-to-end shadow refusal against the REAL /usr/bin of the machine
    # running the suite: an AppImage called `ls` would sit in front of coreutils
    # on PATH for every user.
    mkdir -p "$FIX/lspay"; mk_payload "$FIX/lspay"
    mv "$FIX/lspay/hello.desktop" "$FIX/lspay/ls.desktop"
    mk_appimage "$FIX/Ls.AppImage" "$FIX/lspay"
    refuses "an AppImage that would shadow /usr/bin/ls" "/usr/bin/ls on PATH" \
            install --allow-unsigned "$FIX/Ls.AppImage"
    is "…and it installed nothing"            gone "$([ -e /usr/local/bin/ls ] && echo present || echo gone)"

    out=$(bash "$ENGINE" install --allow-unsigned "$FIX/Hello.AppImage" 2>&1); rc=$?
    is "install exits 0"                      0 "$rc"
    has "…and says FUSE was not involved"     "no FUSE involved" "$out"
    has "…and names the pinning"              "it is pinned" "$out"

    is "no FUSE helper was ever invoked"      absent "$([ -e "$WORK/fuse.used" ] && echo present || echo absent)"
    is "the AppImage file was never executed" 644 "$(stat -c %a "$FIX/Hello.AppImage")"

    is "the launcher exists"                  yes "$([ -x /usr/local/bin/hello ] && echo yes || echo no)"
    is "the desktop entry exists"             yes "$([ -f /usr/local/share/applications/hello.desktop ] && echo yes || echo no)"
    is "the icon exists"                      yes "$([ -f /usr/local/share/icons/hicolor/256x256/apps/hello.png ] && echo yes || echo no)"
    is "the payload was unpacked"             yes "$([ -x /usr/local/lib/apex-appimage/hello/AppRun ] && echo yes || echo no)"

    # The whole point: launching it works, with the runtime's own contract
    # reconstructed, and without FUSE, a mount or the original file's exec bit.
    ran="$(/usr/local/bin/hello 2>&1)"
    has "the launcher runs AppRun"            "AppRun ran" "$ran"
    has "…with APPDIR set"                    "APPDIR=/usr/local/lib/apex-appimage/hello" "$ran"
    has "…and APPIMAGE set"                   "APPIMAGE=/var/lib/apex/appimage/hello.AppImage" "$ran"

    # Ownership, for real, as root — leg A cannot reach this arm.
    has "the tree is chowned to root"         "-R --no-dereference root:root" "$(cat "$WORK/chown.log" 2>/dev/null)"
    is "AppRun is owned by root"              "0:0" "$(stat -c '%u:%g' /usr/local/lib/apex-appimage/hello/AppRun)"
    is "the setuid helper is not setuid"      755 "$(stat -c %a /usr/local/lib/apex-appimage/hello/chrome-sandbox)"
    is "the world-writable dir is not"        755 "$(stat -c %a /usr/local/lib/apex-appimage/hello/usr/bin)"

    # Self-update is impossible rather than merely forbidden.
    is "the kept AppImage is root-owned 0644" "0:0 644" "$(stat -c '%u:%g %a' /var/lib/apex/appimage/hello.AppImage)"
    is "the trust marker holds its checksum"  "$(sha256sum /var/lib/apex/appimage/hello.AppImage | cut -d' ' -f1)" \
                                              "$(cat /var/lib/apex/appimage/hello.trust)"

    out="$(bash "$ENGINE" list 2>&1)"
    has "list names it as pinned"             "apex update' does not change these" "$out"
    has "…and as unverified"                  "signature not verified" "$out"

    out="$(bash "$ENGINE" upgrade 2>&1)"
    has "upgrade says AppImages are pinned"   "AppImages are pinned and not updated by this command: hello" "$out"

    out="$(bash "$ENGINE" verify 2>&1)"; rc=$?
    has "verify reports it was never verified" "its signature was never verified" "$out"
    is "…and still exits 0 when intact"       0 "$rc"

    printf 'tampered' >> /var/lib/apex/appimage/hello.AppImage
    out="$(bash "$ENGINE" verify 2>&1)"; rc=$?
    has "verify catches tampered bytes"       "no longer has the bytes that were accepted" "$out"
    is "…and exits non-zero"                  1 "$rc"
    bash "$ENGINE" install --allow-unsigned "$FIX/Hello.AppImage" >/dev/null 2>&1

    # An `apex install` of an rpm could later put the same name in /usr/bin.
    # Nothing would say so, which is why verify asks.
    mkdir -p "$WORK/fakebin"
    out="$(bash "$ENGINE" verify 2>&1)"
    hasnt "verify is quiet when nothing shadows" "now SHADOWS" "$out"

    # One command naming both an AppImage and something else. Both halves take
    # the engine's lock, which is why take_lock is idempotent: a second
    # `exec 9>` would close the first descriptor and drop the lock mid-way.
    # The unknown package is still reported, and the AppImage is still gone.
    out="$(bash "$ENGINE" remove hello no-such-package 2>&1)"; rc=$?
    has "a mixed remove reports the unknown one" "not installed by apex: no-such-package" "$out"
    has "…and still removed the AppImage"        "removed the AppImage 'hello'" "$out"
    is "…exiting non-zero for the unknown one"   1 "$rc"
    bash "$ENGINE" install --allow-unsigned "$FIX/Hello.AppImage" >/dev/null 2>&1

    out="$(bash "$ENGINE" remove hello 2>&1)"; rc=$?
    is "remove exits 0"                       0 "$rc"
    is "…the launcher is gone"                gone "$([ -e /usr/local/bin/hello ] && echo present || echo gone)"
    is "…the desktop entry is gone"           gone "$([ -e /usr/local/share/applications/hello.desktop ] && echo present || echo gone)"
    is "…the icon is gone"                    gone "$([ -e /usr/local/share/icons/hicolor/256x256/apps/hello.png ] && echo present || echo gone)"
    is "…the unpacked tree is gone"           gone "$([ -e /usr/local/lib/apex-appimage/hello ] && echo present || echo gone)"
    is "…the registry is empty"               "" "$(ls /var/lib/apex/appimage 2>/dev/null)"
    refuses "removing it twice is refused"    "not installed by apex" remove hello

    printf '%s %s %s\n' "$pass" "$fail" "$skip" > "$WORK/legb.count"
    exit 0
fi

# ─────────────────────────────────────────────────────────────────────────────
# LEG A — unprivileged, always.
# ─────────────────────────────────────────────────────────────────────────────
mkdir -p "$WORK/payload"
mk_payload "$WORK/payload"
mk_appimage "$WORK/Hello.AppImage" "$WORK/payload" || { echo "cannot build the fixture"; exit 2; }
OFFSET=4096
printf 'PK\003\004 this is a zip, not an AppImage' > "$WORK/notanappimage.AppImage"
: > "$WORK/empty.AppImage"
mkdir -p "$WORK/adirectory.AppImage"
mk_runtime "$WORK/plainelf.AppImage" 4096 0 62      # an ELF with no AI magic
mk_appimage "$WORK/Type1.AppImage"  "$WORK/payload" 1
mk_appimage "$WORK/Wrong.AppImage"  "$WORK/payload" 2 183   # aarch64 on x86_64 and vice versa
mk_runtime "$WORK/nopayload.AppImage" 4096 2 62     # correct header, no squashfs behind it
# An offset that IS inside the file, with something that is not squashfs there.
mk_runtime "$WORK/nosquash.AppImage" 4096 2 62
dd if=/dev/zero bs=1024 count=8 status=none >> "$WORK/nosquash.AppImage"
mk_runtime "$WORK/32bit.rt" 4096 2 62 1; cat "$WORK/32bit.rt" > "$WORK/ThirtyTwo.AppImage"
cat "$WORK/Hello.AppImage" | tail -c +4097 >> "$WORK/ThirtyTwo.AppImage"

echo "── routing: an AppImage must beat BOTH the rpm rule and the flatpak one ─"
is "./Thing.AppImage is an AppImage"     true  "$(predicate is_appimage_arg ./Thing.AppImage)"
is "…and the rpm rule also claims it"    true  "$(predicate is_local_rpm_arg ./Thing.AppImage)"
is "org.foo.Bar.AppImage is an AppImage" true  "$(predicate is_appimage_arg org.foo.Bar.AppImage)"
is "…and the flatpak rule also claims it" true "$(predicate is_flatpak_id org.foo.Bar.AppImage)"
refuses "so the engine treats it as an AppImage" "no such file: ./Thing.AppImage" \
        install ./Thing.AppImage
is "lowercase .appimage counts"          true  "$(predicate is_appimage_arg ./thing.appimage)"
is "a bare name is never an AppImage"    false "$(predicate is_appimage_arg htop)"
# …and that stays true when a real AppImage with exactly that name is sitting
# in the working directory. `apex install code` installs the repository
# package; name it ./code and it is a file.
mkdir -p "$WORK/cwd" && cp "$WORK/Hello.AppImage" "$WORK/cwd/code"
is "…even with one of that name in cwd"  false "$( cd "$WORK/cwd" && predicate is_appimage_arg code )"
is "…while ./code IS one"                true  "$( cd "$WORK/cwd" && predicate is_appimage_arg ./code )"
is "a bare reverse-DNS id is not one"    false "$(predicate is_appimage_arg org.gimp.GIMP)"
is "a path to a real AppImage is one"    true  "$(predicate is_appimage_arg "$WORK/Hello.AppImage")"
cp "$WORK/notanappimage.AppImage" "$WORK/notanappimage.bin"
is "a path to a zip is not"              false "$(predicate is_appimage_arg "$WORK/notanappimage.bin")"
is "…but the .AppImage NAME still is"    true  "$(predicate is_appimage_arg "$WORK/notanappimage.AppImage")"
is "a .rpm path is not an AppImage"      false "$(predicate is_appimage_arg /media/usb/x.rpm)"

echo "── the file itself: every refusal names the file and the reason ────────"
refuses "missing file"        "no such file: /nonexistent/x.AppImage" install /nonexistent/x.AppImage
refuses "a directory"         "not a regular file: ${WORK}/adirectory.AppImage" install "${WORK}/adirectory.AppImage"
refuses "not an AppImage"     "is not an AppImage" install "${WORK}/notanappimage.AppImage"
refuses "an empty file"       "is not an AppImage" install "${WORK}/empty.AppImage"
refuses "an ELF with no AI magic" "no 'AI' type magic at offset 8" install "${WORK}/plainelf.AppImage"
refuses "a type-1 (ISO 9660) AppImage" "it is a type-1 AppImage" install "${WORK}/Type1.AppImage"
refuses "…and says why, not just no"   "APEX unpacks only the type-2 (squashfs) format" install "${WORK}/Type1.AppImage"
refuses "a foreign architecture"       "this machine is $(uname -m)" install "${WORK}/Wrong.AppImage"
refuses "a 32-bit runtime"             "32-bit ELF" install "${WORK}/ThirtyTwo.AppImage"
refuses "an offset past the end"       "is not inside the file" install "${WORK}/nopayload.AppImage"
refuses "an offset with no squashfs at it" "there is no squashfs filesystem at offset 4096" \
        install "${WORK}/nosquash.AppImage"
# A path echoed back verbatim is what proves it survived word splitting.
cp "$WORK/Hello.AppImage" "$WORK/a file with spaces.AppImage"
refuses "a path containing spaces"     "${WORK}/a file with spaces.AppImage (type 2" \
        install "${WORK}/a file with spaces.AppImage"
refuses "--source contradicts a named AppImage" "contradicts the AppImage you named" \
        install --source flatpak "${WORK}/Hello.AppImage"
refuses "…and there is no --source appimage"    "unknown source 'appimage'" \
        install --source appimage htop

echo "── the offset is arithmetic on the ELF header, never a question asked ──"
is "le_int reads a byte"        2    "$(callfn le_int "$WORK/Hello.AppImage" 4 1)"
is "le_int reads e_machine"     "$(case $(uname -m) in x86_64) echo 62;; aarch64) echo 183;; riscv64) echo 243;; armv7l) echo 40;; *) echo 62;; esac)" \
                                     "$(callfn le_int "$WORK/Hello.AppImage" 18 2)"
is "le_int reads e_shoff"       3904 "$(callfn le_int "$WORK/Hello.AppImage" 40 8)"
is "le_int fails past EOF"      ""   "$(callfn le_int "$WORK/Hello.AppImage" 99999999 8)"
is "appimage_type on type 2"    2    "$(callfn appimage_type "$WORK/Hello.AppImage")"
is "appimage_type on type 1"    1    "$(callfn appimage_type "$WORK/Type1.AppImage")"
is "appimage_type on a plain ELF" 0  "$(callfn appimage_type "$WORK/plainelf.AppImage")"
is "this machine's e_machine matches" true "$(predicate appimage_machine_matches "$(callfn le_int "$WORK/Hello.AppImage" 18 2)")"
is "a foreign one does not"     false "$(predicate appimage_machine_matches 999)"
is "the computed offset"        "$OFFSET" "$(snippet <<<'inspect_appimage "'"$WORK"'/Hello.AppImage"; printf %s "$AI_OFFSET"')"
is "…and there is squashfs there" hsqs "$(dd if="$WORK/Hello.AppImage" bs=1 skip=$OFFSET count=4 status=none)"

echo "── unpacking: no FUSE, and the payload's privileges are taken away ─────"
rm -rf "$WORK/x1"
is "appimage_extract succeeds"   true "$(predicate appimage_extract "$WORK/Hello.AppImage" "$OFFSET" "$WORK/x1")"
is "AppRun is there"             yes  "$([ -f "$WORK/x1/AppRun" ] && echo yes || echo no)"
is "the desktop file is there"   yes  "$([ -f "$WORK/x1/hello.desktop" ] && echo yes || echo no)"
is "the setuid bit is gone"      755  "$(stat -c %a "$WORK/x1/chrome-sandbox")"
is "the world-writable dir is not" 755 "$(stat -c %a "$WORK/x1/usr/bin")"
is "AppRun stays executable"     755  "$(stat -c %a "$WORK/x1/AppRun")"
is "an owner-only file is opened" 644  "$(stat -c %a "$WORK/x1/private.dat")"
rm -rf "$WORK/x2"
is "a bogus offset fails"        false "$(predicate appimage_extract "$WORK/Hello.AppImage" 17 "$WORK/x2")"
is "…and leaves nothing behind"  gone  "$([ -e "$WORK/x2" ] && echo present || echo gone)"
# harden on its own, over a tree that was never a squashfs: setgid too.
mkdir -p "$WORK/h/d"; printf x > "$WORK/h/f"; chmod 6777 "$WORK/h/f"; chmod 2777 "$WORK/h/d"
printf x > "$WORK/h/priv"; chmod 0600 "$WORK/h/priv"
printf x > "$WORK/h/privx"; chmod 0700 "$WORK/h/privx"
callfn appimage_harden "$WORK/h" >/dev/null
is "harden strips setuid+setgid"  755 "$(stat -c %a "$WORK/h/f")"
is "harden strips group/other write" 755 "$(stat -c %a "$WORK/h/d")"
is "harden opens an owner-only file" 644 "$(stat -c %a "$WORK/h/priv")"
is "…keeping x only where it was"   755 "$(stat -c %a "$WORK/h/privx")"

echo "── identity: what the application is called, read out of the payload ───"
idfn() { snippet <<EOF
inspect_appdir "$1" fixture
printf '%s|%s|%s' "\$AI_ID" "\$AI_CMD" "\$AI_ICON"
EOF
}
is "id, command and icon"       "hello|hello|hello" "$(idfn "$WORK/x1")"
# Exec=AppRun is the format's entry point, not the application's name, so the
# command falls back to the last segment of the desktop id.
rm -rf "$WORK/x3"; cp -a "$WORK/x1" "$WORK/x3"
mv "$WORK/x3/hello.desktop" "$WORK/x3/org.example.Krita.desktop"
is "Exec=AppRun falls back to the id" "org.example.Krita|krita|hello" "$(idfn "$WORK/x3")"
# …and an Exec that names a real program wins over the id.
rm -rf "$WORK/x4"; cp -a "$WORK/x1" "$WORK/x4"
sed -i 's|^Exec=AppRun|Exec=usr/bin/thunderbird|' "$WORK/x4/hello.desktop"
is "a real Exec names the command"    "hello|thunderbird|hello" "$(idfn "$WORK/x4")"
rm -rf "$WORK/x5"; cp -a "$WORK/x1" "$WORK/x5"; rm -f "$WORK/x5"/*.desktop
has "no desktop entry is refused"     "there is no .desktop file at the root of its payload" "$(idfn "$WORK/x5")"
rm -rf "$WORK/x6"; cp -a "$WORK/x1" "$WORK/x6"; cp "$WORK/x6/hello.desktop" "$WORK/x6/second.desktop"
has "two desktop entries are refused"  "the format allows exactly one" "$(idfn "$WORK/x6")"
# The name out of an untrusted payload becomes a root-owned path.
rm -rf "$WORK/x7"; cp -a "$WORK/x1" "$WORK/x7"
mv "$WORK/x7/hello.desktop" "$WORK/x7/-rf.desktop"
has "a hostile desktop id is refused"  "is not a name APEX will turn into a path" "$(idfn "$WORK/x7")"
# A traversal in Exec is normalised to its basename — `Exec=usr/bin/thunderbird`
# is ordinary and has to keep working — and what stops it there is the SECOND
# layer: the resulting command is one /usr/bin provides, so the shadow guard
# refuses it. Both halves asserted, because either alone would be a story.
# An id that fails the name rule on its own, while the command derived from it
# would pass — otherwise the command check below would be doing this one's job.
rm -rf "$WORK/x7b"; cp -a "$WORK/x1" "$WORK/x7b"
mv "$WORK/x7b/hello.desktop" "$WORK/x7b/.hidden.app.desktop"
has "a hidden desktop id is refused"   "its desktop entry is named '.hidden.app.desktop'" "$(idfn "$WORK/x7b")"
rm -rf "$WORK/x8"; cp -a "$WORK/x1" "$WORK/x8"
sed -i 's|^Exec=AppRun.*|Exec=../../../bin/sh|' "$WORK/x8/hello.desktop"
is "a traversal in Exec is basenamed"  "hello|sh|hello" "$(idfn "$WORK/x8")"
rm -rf "$WORK/x8b"; cp -a "$WORK/x1" "$WORK/x8b"
sed -i 's|^Exec=AppRun.*|Exec=-rf --now|' "$WORK/x8b/hello.desktop"
has "a hostile Exec command is refused" "is not a name APEX will turn into a path" "$(idfn "$WORK/x8b")"
rm -rf "$WORK/x8c"; cp -a "$WORK/x1" "$WORK/x8c"
sed -i 's|^Exec=AppRun.*|Exec=..|' "$WORK/x8c/hello.desktop"
has "…and so is a bare .."              "is not a name APEX will turn into a path" "$(idfn "$WORK/x8c")"
# A .desktop that resolves outside the payload is a file the engine must not read.
rm -rf "$WORK/x9"; cp -a "$WORK/x1" "$WORK/x9"; rm -f "$WORK/x9"/*.desktop
printf 'secret\n' > "$WORK/outside.desktop"
ln -s "$WORK/outside.desktop" "$WORK/x9/escape.desktop"
has "an escaping desktop symlink is refused" "resolves to a path outside the AppImage" "$(idfn "$WORK/x9")"

echo "── desktop_key reads the right group and the right key ────────────────"
is "Exec from [Desktop Entry]"  "AppRun --no-sandbox %U" "$(callfn desktop_key "$WORK/x1/hello.desktop" Exec)"
is "Icon"                       "hello"                  "$(callfn desktop_key "$WORK/x1/hello.desktop" Icon)"
is "a key that is not there"    ""                       "$(callfn desktop_key "$WORK/x1/hello.desktop" Nope)"
printf '[Desktop Entry]\nName[de]=Hallo\nName=Hello\n' > "$WORK/loc.desktop"
is "a locale suffix is not the key" "Hello"              "$(callfn desktop_key "$WORK/loc.desktop" Name)"

echo "── path_within is the guard on everything read out of the payload ─────"
is "a file inside"        true  "$(predicate path_within "$WORK/x1" "$WORK/x1/AppRun")"
is "a symlink inside"     true  "$(predicate path_within "$WORK/x1" "$WORK/x1/.DirIcon")"
is "the root itself"      true  "$(predicate path_within "$WORK/x1" "$WORK/x1")"
is "a sibling directory"  false "$(predicate path_within "$WORK/x1" "$WORK/payload/AppRun")"
ln -sf /etc/hostname "$WORK/x1/escape.link"
is "a symlink pointing out" false "$(predicate path_within "$WORK/x1" "$WORK/x1/escape.link")"
rm -f "$WORK/x1/escape.link"

echo "── shadowing: an AppImage may never hide what the OS provides ─────────"
SYS="$WORK/sysroot"; PFX="$WORK/prefix"; REG="$WORK/reg"
mkdir -p "$SYS/usr/bin" "$SYS/usr/sbin" "$SYS/usr/share/applications" "$PFX/bin" "$PFX/share/applications" "$REG"
shadow() { snippet <<EOF
appimage_shadow_check "$1" "$2" "$PFX" "$SYS" "$REG" && echo ALLOWED
EOF
}
is "a name nothing provides is allowed"  ALLOWED "$(shadow hello hello)"
# …and the basenamed traversal from the identity block above lands here.
: > "$SYS/usr/bin/sh"
has "the basenamed traversal is caught here" "comes before ${SYS}/usr/bin/sh on PATH" "$(shadow sh some.id)"
: > "$SYS/usr/bin/firefox"
has "a name /usr/bin provides is refused" "comes before ${SYS}/usr/bin/firefox on PATH" "$(shadow firefox org.moz.Firefox)"
: > "$SYS/usr/sbin/netplan"
has "…and /usr/sbin too"                  "${SYS}/usr/sbin/netplan" "$(shadow netplan org.x.Netplan)"
: > "$SYS/usr/share/applications/org.gimp.GIMP.desktop"
has "an id /usr/share provides is refused" "already exists" "$(shadow gimpthing org.gimp.GIMP)"
has "…and says why it matters"             "comes first in XDG_DATA_DIRS" "$(shadow gimpthing org.gimp.GIMP)"
: > "$PFX/bin/handmade"
has "a foreign file in the prefix is refused" "no AppImage APEX installed owns it" "$(shadow handmade some.id)"
printf '%s\n' "$PFX/bin/handmade" > "$REG/handmade.files"
is "…but one this engine installed is not" ALLOWED "$(shadow handmade some.id)"
is "appimage_registry_owns finds it"       true  "$(predicate appimage_registry_owns "$REG" "$PFX/bin/handmade")"
is "…and not a path it never placed"       false "$(predicate appimage_registry_owns "$REG" "$PFX/bin/other")"

echo "── integration: the launcher, the desktop entry and the icon ──────────"
IPFX="$WORK/iprefix"; MAN="$WORK/manifest"
mkdir -p "$IPFX"
callfn appimage_integrate "$WORK/x1" "$IPFX" hello hello /var/lib/apex/appimage/hello.AppImage \
        "$WORK/x1/hello.desktop" hello "$MAN" >/dev/null
W="$(cat "$IPFX/bin/hello" 2>/dev/null)"
is "the launcher is executable"    755 "$(stat -c %a "$IPFX/bin/hello" 2>/dev/null)"
has "…sets APPDIR"                 "APPDIR='$WORK/x1'" "$W"
has "…sets APPIMAGE to the kept copy" "APPIMAGE='/var/lib/apex/appimage/hello.AppImage'" "$W"
has "…sets ARGV0"                  'ARGV0="$0"' "$W"
has "…sets OWD"                    'OWD="$(pwd)"' "$W"
has "…and execs AppRun"            'exec "$APPDIR/AppRun" "$@"' "$W"
hasnt "…and never extracts at run time" "appimage-extract" "$W"
D="$(cat "$IPFX/share/applications/hello.desktop" 2>/dev/null)"
has "Exec points at the launcher"  "Exec=${IPFX}/bin/hello --no-sandbox %U" "$D"
has "…keeping the flags after it"  "--no-sandbox %U" "$D"
has "the desktop action's Exec too" "Exec=${IPFX}/bin/hello --new-window" "$D"
has "TryExec points at it as well" "TryExec=${IPFX}/bin/hello" "$D"
hasnt "DBusActivatable is dropped" "DBusActivatable" "$D"
has "Icon survives"                "Icon=hello" "$D"
has "…and so does everything else" "Categories=Utility;" "$D"
is "the icon went to the theme"    yes "$([ -f "$IPFX/share/icons/hicolor/256x256/apps/hello.png" ] && echo yes || echo no)"
is "…and the scalable one too"     yes "$([ -f "$IPFX/share/icons/hicolor/scalable/apps/hello.svg" ] && echo yes || echo no)"
is "the manifest records all four" 4 "$(wc -l < "$MAN")"
has "…including the launcher"      "${IPFX}/bin/hello" "$(cat "$MAN")"

# Only icons named by Icon= are taken. A payload that ships a whole theme must
# not be able to drop an index.theme or another application's icon into a
# directory that outranks /usr/share for every user on the machine.
rm -rf "$WORK/xi" "$WORK/iprefix2"; cp -a "$WORK/x1" "$WORK/xi"; mkdir -p "$WORK/iprefix2"
printf 'evil' > "$WORK/xi/usr/share/icons/hicolor/index.theme"
printf 'evil' > "$WORK/xi/usr/share/icons/hicolor/256x256/apps/firefox.png"
callfn appimage_install_icons "$WORK/xi" "$WORK/iprefix2" hello "$WORK/manifest2" >/dev/null
is "another app's icon is not taken" gone "$([ -e "$WORK/iprefix2/share/icons/hicolor/256x256/apps/firefox.png" ] && echo present || echo gone)"
is "index.theme is not taken"        gone "$([ -e "$WORK/iprefix2/share/icons/hicolor/index.theme" ] && echo present || echo gone)"
is "the named icon is"               yes  "$([ -f "$WORK/iprefix2/share/icons/hicolor/256x256/apps/hello.png" ] && echo yes || echo no)"

# .DirIcon is conventionally a symlink, so it is the obvious way to make a root
# process copy /etc/shadow somewhere world-readable.
rm -rf "$WORK/xe" "$WORK/eprefix"; cp -a "$WORK/x1" "$WORK/xe"; mkdir -p "$WORK/eprefix"
rm -rf "$WORK/xe/usr/share/icons"
printf 'SECRET\n' > "$WORK/secret"
ln -sf "$WORK/secret" "$WORK/xe/.DirIcon"
callfn appimage_install_icons "$WORK/xe" "$WORK/eprefix" hello "$WORK/manifest3" >/dev/null 2>&1
is "an escaping .DirIcon is not copied" 0 "$(find "$WORK/eprefix" -type f 2>/dev/null | wc -l)"
# …and one that stays inside is, with the extension read off the bytes.
rm -f "$WORK/xe/.DirIcon"; printf '\211PNG\r\n\032\nx' > "$WORK/xe/.DirIcon"
callfn appimage_install_icons "$WORK/xe" "$WORK/eprefix" hello "$WORK/manifest4" >/dev/null
is ".DirIcon lands as a PNG"  yes "$([ -f "$WORK/eprefix/share/icons/hicolor/256x256/apps/hello.png" ] && echo yes || echo no)"

echo "── nothing is ever overwritten, and nothing outside the prefix removed ─"
mkdir -p "$WORK/op/bin"; printf 'theirs\n' > "$WORK/op/bin/hello"; : > "$WORK/opman"
has "placing over a foreign file is refused" "refusing to overwrite" \
    "$(callfn appimage_place "$WORK/x1/AppRun" "$WORK/op/bin/hello" 0755 "$WORK/opman" 2>&1)"
is "…and the file is untouched"  "theirs" "$(cat "$WORK/op/bin/hello")"
UREG="$WORK/ureg"; UPFX="$WORK/uprefix"; UAPP="$WORK/uapps"
mkdir -p "$UREG" "$UPFX/bin" "$UAPP/hello"
printf 'x' > "$UPFX/bin/hello"; printf 'x' > "$UAPP/hello/AppRun"
printf 'keepme\n' > "$WORK/outside-the-prefix"
printf '%s\n%s\n' "$UPFX/bin/hello" "$WORK/outside-the-prefix" > "$UREG/hello.files"
: > "$UREG/hello.json"
out="$(callfn appimage_uninstall hello "$UREG" "$UPFX" "$UAPP" 2>&1)"
is "uninstall removes what it placed"  gone "$([ -e "$UPFX/bin/hello" ] && echo present || echo gone)"
is "…and the unpacked tree"            gone "$([ -e "$UAPP/hello" ] && echo present || echo gone)"
is "…and the record"                   gone "$([ -e "$UREG/hello.json" ] && echo present || echo gone)"
is "…but NOT a path outside the prefix" present "$([ -e "$WORK/outside-the-prefix" ] && echo present || echo gone)"
has "…and says so out loud"            "it is outside" "$out"
is "uninstall with no name refuses"    false "$(predicate appimage_uninstall "" "$UREG" "$UPFX" "$UAPP")"

echo "── the registry: identity, lookup and what apex update will not do ───"
LREG="$WORK/lreg"; mkdir -p "$LREG"
cp "$WORK/Hello.AppImage" "$LREG/hello.AppImage"; : > "$LREG/hello.json"
is "appimage_installed lists it"   "hello" "$(callfn appimage_installed "$LREG")"
is "lookup by name"                "hello" "$(callfn appimage_lookup "$LREG" hello)"
cp "$WORK/Hello.AppImage" "$WORK/moved-elsewhere.AppImage"
is "lookup by the same bytes"      "hello" "$(callfn appimage_lookup "$LREG" "$WORK/moved-elsewhere.AppImage")"
is "lookup of a stranger fails"    false   "$(predicate appimage_lookup "$LREG" nothing-like-this)"
is "lookup of other bytes fails"   false   "$(predicate appimage_lookup "$LREG" "$WORK/notanappimage.AppImage")"
is "the update channel is empty here" "" "$(callfn appimage_update_info "$WORK/Hello.AppImage")"

echo "── verify: the questions only time can answer ─────────────────────────"
VREG="$WORK/vreg"; VSYS="$WORK/vsys"; VPFX="$WORK/vpfx"
mkdir -p "$VREG" "$VSYS/usr/bin" "$VPFX/bin"
cp "$WORK/Hello.AppImage" "$VREG/hello.AppImage"
printf '%s\n' "$VPFX/bin/hello" > "$VREG/hello.files"; : > "$VPFX/bin/hello"
printf '{"image_sha256":"%s"}\n' "$(sha256sum "$VREG/hello.AppImage" | cut -d' ' -f1)" > "$VREG/hello.json"
V="$(callfn appimage_verify "$VREG" "$VPFX" "$VSYS" 2>&1)"
has "verify says it was never verified"  "its signature was never verified" "$V"
hasnt "…and finds nothing else wrong"    "no longer has the bytes" "$V"
is   "…exiting 0"                        true "$(predicate appimage_verify "$VREG" "$VPFX" "$VSYS" | tail -1)"
# The case only time produces: an `apex install` of the rpm, months later, puts
# the same name in /usr/bin behind /usr/local/bin. Nothing else would say so.
: > "$VSYS/usr/bin/hello"
V="$(callfn appimage_verify "$VREG" "$VPFX" "$VSYS" 2>&1)"
has "verify catches a new shadow"        "now SHADOWS ${VSYS}/usr/bin/hello" "$V"
is   "…and exits non-zero"               false "$(predicate appimage_verify "$VREG" "$VPFX" "$VSYS" | tail -1)"
rm -f "$VSYS/usr/bin/hello" "$VPFX/bin/hello"
V="$(callfn appimage_verify "$VREG" "$VPFX" "$VSYS" 2>&1)"
has "verify catches a missing file"      "${VPFX}/bin/hello is missing" "$V"
rm -f "$VREG/hello.files"
V="$(callfn appimage_verify "$VREG" "$VPFX" "$VSYS" 2>&1)"
has "…and an unreadable manifest"        "file manifest ${VREG}/hello.files cannot be read" "$V"

echo "── the refusal every AppImage gets, because none of them is verifiable ─"
U="$(snippet <<EOF
refuse_unsigned_appimage "$WORK/Hello.AppImage"
EOF
)"
has "it names the file"            "cannot verify ${WORK}/Hello.AppImage" "$U"
has "…says a signature proves nothing here" "a key taken from the file it signs proves nothing" "$U"
has "…says it will be pinned"      "pinned to these bytes" "$U"
has "…and gives the exact command" "sudo apex install --allow-unsigned ${WORK}/Hello.AppImage" "$U"

# ─────────────────────────────────────────────────────────────────────────────
# LEG B, dispatched. It needs an unprivileged user namespace and a private
# mount namespace; on a machine without them it skips, LOUDLY and naming every
# assertion that did not run, rather than reporting a green suite that checked
# the shipped engine's end-to-end behaviour not at all.
# ─────────────────────────────────────────────────────────────────────────────
echo
if unshare -rm true 2>/dev/null; then
    unshare -rm "$SELF" --leg-b "$WORK" "$ENGINE"
    if [ -r "$WORK/legb.count" ]; then
        read -r bp bf bs < "$WORK/legb.count"
        pass=$((pass+bp)); fail=$((fail+bf)); skip=$((skip+bs))
    else
        bad "leg B" "it did not finish; its assertions did not run"
    fi
elif [ -n "${GITHUB_ACTIONS:-}" ]; then
    # In CI this is a FAILURE, not a skip. ubuntu-24.04 gates unprivileged user
    # namespaces behind AppArmor (kernel.apparmor_restrict_unprivileged_userns),
    # and a runner image that turns that on would silently take leg B away —
    # a loud skip nobody reads is how a gate stops inspecting anything.
    bad "leg B (the shipped engine, end to end, as root)" \
        "no unprivileged user namespace on this runner; see kernel.apparmor_restrict_unprivileged_userns"
else
    skipped "leg B (the shipped engine, end to end, as root)" \
            "no unprivileged user namespace here; install/list/verify/upgrade/remove, the chown to root:root and the launcher actually running were NOT checked"
fi

echo
printf 'apex-appimage: %d passed, %d failed, %d skipped\n' "$pass" "$fail" "$skip"
[ "$fail" -eq 0 ] || exit 1
exit 0
