#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  build-local.sh — build APEX-OS images locally, with the kernel signed.
#
#  WHY THIS EXISTS. Containerfile.core signs the CachyOS kernel with the APEX
#  MOK only when the key is mounted as a build secret:
#
#     podman build --secret id=apex_sb_key,src=… --secret id=apex_sb_crt,src=…
#
#  and when the secret is absent it stamps the image `unsigned` and carries on
#  by design, so local builds keep working for people without the key. The
#  consequence is that forgetting the flag produces an image that looks fine,
#  builds green, and cannot be used with Secure Boot — with nothing but one log
#  line saying so. Nothing in the repo passed the flag; it was done by hand
#  every time, which is a coin flip nobody should be asked to keep winning.
#
#  This script passes it, and REFUSES to produce an unsigned image unless you
#  explicitly ask for one with --allow-unsigned.
#
#  Usage:
#     ./build-local.sh                 core + base + apex, signed
#     ./build-local.sh base            just the base (reuses the existing core)
#     ./build-local.sh apex            just the image tier
#     ./build-local.sh --allow-unsigned base       no key needed
#     ./build-local.sh --force-core                rebuild core even if present
#
#  ONE IMAGE. `daily`, `gaming-mesa` and `gaming-nvidia` are gone as build
#  targets; there is a single image and the three names survive only as published
#  tags pointing at it. They are still accepted here and map to `apex`, so a
#  habit or a stale script does not fail with "unknown target".
#
#  CORE vs BASE. The image is built in three tiers (see Containerfile.core's
#  header): `core` is the slow-moving ~45 min foundation, `base` is the thin
#  per-commit tier on top of it, and the image tier comes last. Core is REUSED
#  when it already exists locally, because rebuilding it is both slow and — on a
#  published image — a multi-gigabyte download for every machine on the fleet.
#  Pass --force-core when you actually mean to move it.
#
#  --force-core IS NOT OPTIONAL FOR A GPU OR MODULE CHANGE. The NVIDIA and
#  controller akmods, their MOK signatures and the NVIDIA userspace all live in
#  core now. Reusing an older local core produces an image without them and every
#  check below still passes, which is exactly how a validation build in this
#  project reported green against an artifact that did not contain the change.
#
#  Key location: ~/.apex-signing/apex-mok.{key,crt}, overridable with
#  APEX_SIGNING_DIR. The key is never copied, never committed, and never enters
#  the image — --secret is a tmpfs mount that leaves no layer behind.
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail
cd "$(dirname "$0")"

SIGNDIR="${APEX_SIGNING_DIR:-$HOME/.apex-signing}"
KEY="$SIGNDIR/apex-mok.key"
CRT="$SIGNDIR/apex-mok.crt"
ALLOW_UNSIGNED=0
FORCE_CORE=0
TARGETS=()

for a in "$@"; do
    case "$a" in
        --allow-unsigned) ALLOW_UNSIGNED=1 ;;
        --force-core) FORCE_CORE=1 ;;
        -h|--help) sed -n '2,30p' "$0"; exit 0 ;;
        -*) echo "unknown option: $a" >&2; exit 2 ;;
        *) TARGETS+=("$a") ;;
    esac
done
[ "${#TARGETS[@]}" -gt 0 ] || TARGETS=(core base apex)

# ── The signing key ──────────────────────────────────────────────────────────
SECRET_ARGS=()
if [ -s "$KEY" ] && [ -s "$CRT" ]; then
    # Validate before building rather than discovering at sbsign time, an hour
    # into a base build.
    openssl rsa  -in "$KEY" -noout -check >/dev/null 2>&1 \
        || { echo "FATAL: $KEY is not a valid RSA private key"; exit 1; }
    openssl x509 -in "$CRT" -noout >/dev/null 2>&1 \
        || { echo "FATAL: $CRT is not a valid certificate"; exit 1; }
    k=$(openssl rsa  -in "$KEY" -noout -modulus | sha256sum)
    c=$(openssl x509 -in "$CRT" -noout -modulus | sha256sum)
    [ "$k" = "$c" ] || { echo "FATAL: $KEY and $CRT are not a matching pair"; exit 1; }
    SECRET_ARGS=(--secret "id=apex_sb_key,src=$KEY" --secret "id=apex_sb_crt,src=$CRT")
    echo "signing key: $KEY (validated)"
elif [ "$ALLOW_UNSIGNED" = 1 ]; then
    echo "WARNING: building UNSIGNED — the result cannot be used with Secure Boot."
else
    cat >&2 <<EOF
FATAL: no signing key at $SIGNDIR

Containerfile.core would silently produce an image whose kernel is unsigned,
which cannot boot with Secure Boot on and gives users nothing to enrol. That is
too easy to ship by accident, so this script refuses instead.

Either put apex-mok.key and apex-mok.crt in $SIGNDIR (or set APEX_SIGNING_DIR),
or pass --allow-unsigned if you genuinely want an unsigned image.
EOF
    exit 1
fi

REV="$(git rev-parse HEAD 2>/dev/null || echo unknown)"

CORE_IMG=localhost/apex-os-core:latest

# ── The shell ref, resolved rather than named ────────────────────────────────
# Containerfile.base defaults APEX_SHELL_REF to `main`, and `git clone --branch
# main` is a cache hit forever: podman cannot know the remote moved, so a local
# build silently vendors whatever apex-shell was at the first build and keeps
# doing so. Observed directly — a base build begun minutes after apex-shell's
# main advanced printed `Using cache` for the clone layer and shipped the old
# shell.
#
# CI does not have this problem because build-image.yml resolves the SHA first
# and passes it, so the build-arg changes whenever the shell does. This does the
# same, which also makes a local build reproduce what CI produces instead of
# something subtly older.
#
# A failure to reach the remote is fatal rather than a fallback to `main`: a
# build that quietly vendors a stale shell is the thing this exists to prevent.
SHELL_REF="${APEX_SHELL_REF:-}"
if [ -z "$SHELL_REF" ]; then
    SHELL_REF="$(git ls-remote https://github.com/AndreNijman/apex-shell refs/heads/main 2>/dev/null | awk '{print $1}')"
    [ -n "$SHELL_REF" ] || {
        echo "FATAL: cannot resolve apex-shell main. Set APEX_SHELL_REF=<sha> to build offline." >&2
        exit 1
    }
fi
echo "== shell == vendoring apex-shell $SHELL_REF"


# Everything the shipped kernel's signature can be checked against. Used after
# core (where signing happens) and after base (which only inherits it).
assert_signed() {  # $1 = image, $2 = label
    [ "${#SECRET_ARGS[@]}" -gt 0 ] || return 0
    local st
    st=$(sudo podman run --rm --entrypoint /bin/sh "$1" \
           -c 'cat /usr/share/apex-os/secureboot/kernel-signed 2>/dev/null || echo missing')
    [ "$st" = signed ] \
        || { echo "FATAL: $2 is stamped '$st' — refusing to continue with an unsigned kernel"; exit 1; }
    sudo podman run --rm --entrypoint /bin/sh "$1" -c \
        'dnf5 -y install -q sbsigntools >/dev/null 2>&1; sbverify --list /usr/lib/modules/$(cat /usr/lib/apex-kver)/vmlinuz' \
        | grep -qi 'signature certificates\|image signature issuers\|APEX' \
        || { echo "FATAL: sbverify found no signature on $2's vmlinuz"; exit 1; }
    sudo podman run --rm --entrypoint /bin/sh "$1" \
        -c 'test -s /usr/share/apex-os/secureboot/apex-mok.der' \
        || { echo "FATAL: $2 has no apex-mok.der — users would have nothing to enrol"; exit 1; }
    # The out-of-tree modules, checked the same way and for the same reason: a
    # marker file is a claim the build wrote about itself. `modinfo -F signer`
    # reads the PKCS#7 signature out of the module that will actually ship.
    #
    # Written so it cannot pass on an empty set — a loop over nothing succeeds,
    # and "no modules found" is precisely the failure this is here to catch.
    sudo podman run --rm --entrypoint /bin/sh "$1" -c '
        set -eu
        KVER=$(cat /usr/lib/apex-kver)
        MODDIR=/usr/lib/modules/$KVER
        [ "$(cat /usr/share/apex-os/secureboot/modules-signed 2>/dev/null || echo missing)" = signed ] || {
            echo "modules-signed is not \"signed\""; exit 1; }
        SIGNER=$(cat /usr/share/apex-os/secureboot/module-signer)
        OOT=""
        for d in extra updates; do [ -d "$MODDIR/$d" ] && OOT="$OOT $MODDIR/$d"; done
        [ -n "$OOT" ] || { echo "no out-of-tree module directory"; exit 1; }
        n=0
        for m in $(find $OOT -type f -name "*.ko*"); do
            got=$(modinfo -F signer "$m" 2>/dev/null || true)
            [ "$got" = "$SIGNER" ] || { echo "$m signed by \"$got\", expected \"$SIGNER\""; exit 1; }
            n=$((n + 1))
        done
        [ "$n" -gt 0 ] || { echo "zero out-of-tree modules — vacuous pass"; exit 1; }
        for pat in nvidia xone xpadneo; do
            [ "$(find $OOT -type f -name "*$pat*.ko*" | wc -l)" -gt 0 ] || { echo "no $pat module"; exit 1; }
        done
        echo "  $n out-of-tree modules, all signed by \"$SIGNER\""
    ' || { echo "FATAL: $2 has unsigned or missing out-of-tree kernel modules"; exit 1; }
    echo "$2: kernel signed, modules signed, apex-mok.der present"
}

build_core() {
    if [ "$FORCE_CORE" = 0 ] && sudo podman image exists "$CORE_IMG"; then
        echo "== core == reusing existing $CORE_IMG (pass --force-core to rebuild)"
        return 0
    fi
    echo "== core == (this is the slow one, ~45 min)"
    sudo podman build --isolation=chroot \
        "${SECRET_ARGS[@]}" \
        --build-arg APEX_REVISION="$REV" \
        -f Containerfile.core -t "$CORE_IMG" .

    # Assert rather than trust. The Containerfile degrades to `unsigned` on any
    # signing failure, so a green build is not evidence the kernel is signed.
    assert_signed "$CORE_IMG" core
}

build_base() {
    # The base is FROM the core, so it cannot be built without one. Say so
    # clearly instead of letting podman fail on a missing image reference.
    sudo podman image exists "$CORE_IMG" \
        || { echo "FATAL: $CORE_IMG does not exist — run ./build-local.sh core first"; exit 1; }
    echo "== base =="
    sudo podman build --isolation=chroot \
        --build-arg CORE="$CORE_IMG" \
        --build-arg APEX_REVISION="$REV" \
        --build-arg APEX_SHELL_REF="$SHELL_REF" \
        -f Containerfile.base -t localhost/apex-os-base:latest .

    # Catches building on a stale unsigned core.
    assert_signed localhost/apex-os-base:latest base
}

build_image() {  # $1 = apex (or a legacy tag name, which maps to it)
    local f=$1
    case "$f" in
        apex|daily|gaming-mesa|gaming-nvidia) ;;
        *) echo "unknown target: $f" >&2; exit 2 ;;
    esac
    [ "$f" = apex ] || echo "note: '$f' is a published TAG, not a build target — building the one image"
    echo "== apex =="
    sudo podman build --isolation=chroot \
        --build-arg BASE=localhost/apex-os-base:latest \
        --build-arg APEX_REVISION="$REV" \
        -f Containerfile.apex -t localhost/apex-os:apex .
    # The three published names all resolve to this one image in the registry;
    # tag them locally too so a local `bootc switch` against any of them works.
    for t in daily gaming-mesa gaming-nvidia; do
        sudo podman tag localhost/apex-os:apex "localhost/apex-os:$t"
    done

    # Then read them back. `podman tag` cannot plausibly fail here — the point
    # is not to doubt it, it is to pin the INVARIANT. If this function is ever
    # changed to build per-name images again, the tags stop being one image and
    # every machine tracking a legacy name starts drifting onto different bytes.
    # That regression is silent, and this is the only local thing that would
    # notice it. The registry-side equivalent lives in build-image.yml and can
    # only run on a real publish.
    want="$(sudo podman image inspect --format '{{.Id}}' localhost/apex-os:apex)"
    [ -n "$want" ] || { echo "FATAL: localhost/apex-os:apex has no image ID" >&2; exit 1; }
    for t in apex daily gaming-mesa gaming-nvidia; do
        got="$(sudo podman image inspect --format '{{.Id}}' "localhost/apex-os:$t" 2>/dev/null || echo MISSING)"
        [ "$got" = "$want" ] || {
            echo "FATAL: localhost/apex-os:$t resolves to '$got', expected '$want'" >&2
            echo "       the four names must be ONE image; see docs/ci-release-tiers.md" >&2
            exit 1
        }
    done
    echo "tags: apex, daily, gaming-mesa, gaming-nvidia all resolve to $want"

    # The image inherits signing from core via the base, so this catches building
    # on top of a stale or unsigned tier — the same hole the CI job covers.
    assert_signed localhost/apex-os:apex apex
}

for t in "${TARGETS[@]}"; do
    case "$t" in
        core) build_core ;;
        base) build_base ;;
        *)    build_image "$t" ;;
    esac
done
echo "done: ${TARGETS[*]}"
