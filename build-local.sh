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
#     ./build-local.sh                 core + base + daily + gaming-nvidia, signed
#     ./build-local.sh base            just the base (reuses the existing core)
#     ./build-local.sh daily gaming-mesa
#     ./build-local.sh --allow-unsigned base       no key needed
#     ./build-local.sh --force-core                rebuild core even if present
#
#  CORE vs BASE. The image is built in three tiers (see Containerfile.core's
#  header): `core` is the slow-moving ~45 min foundation, `base` is the thin
#  per-commit tier on top of it, and the editions come last. Core is REUSED when
#  it already exists locally, because rebuilding it is both slow and — on a
#  published image — a multi-gigabyte download for every machine on the fleet.
#  Pass --force-core when you actually mean to move it.
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
[ "${#TARGETS[@]}" -gt 0 ] || TARGETS=(core base daily gaming-nvidia)

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
    echo "$2: kernel signed, apex-mok.der present"
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
        -f Containerfile.base -t localhost/apex-os-base:latest .

    # Catches building on a stale unsigned core.
    assert_signed localhost/apex-os-base:latest base
}

build_flavor() {  # $1 = daily | gaming-mesa | gaming-nvidia
    local f=$1 cf args=()
    case "$f" in
        daily)         cf=Containerfile.daily ;;
        gaming-mesa)   cf=Containerfile.gaming; args=(--build-arg GPU=mesa) ;;
        gaming-nvidia) cf=Containerfile.gaming; args=(--build-arg GPU=nvidia) ;;
        *) echo "unknown target: $f" >&2; exit 2 ;;
    esac
    echo "== $f =="
    sudo podman build --isolation=chroot \
        --build-arg BASE=localhost/apex-os-base:latest \
        --build-arg APEX_REVISION="$REV" \
        "${args[@]}" -f "$cf" -t "localhost/apex-os:$f" .

    # Editions inherit signing from core via the base, so this catches building
    # an edition on top of a stale unsigned tier — the same hole the CI edition
    # jobs cover.
    assert_signed "localhost/apex-os:$f" "$f"
}

for t in "${TARGETS[@]}"; do
    case "$t" in
        core) build_core ;;
        base) build_base ;;
        *)    build_flavor "$t" ;;
    esac
done
echo "done: ${TARGETS[*]}"
