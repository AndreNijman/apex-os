#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  build-local.sh — build APEX-OS images locally, with the kernel signed.
#
#  WHY THIS EXISTS. Containerfile.base signs the CachyOS kernel with the APEX
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
#     ./build-local.sh                 base + daily + gaming-nvidia, signed
#     ./build-local.sh base            just the base
#     ./build-local.sh daily gaming-mesa
#     ./build-local.sh --allow-unsigned base       no key needed
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
TARGETS=()

for a in "$@"; do
    case "$a" in
        --allow-unsigned) ALLOW_UNSIGNED=1 ;;
        -h|--help) sed -n '2,30p' "$0"; exit 0 ;;
        -*) echo "unknown option: $a" >&2; exit 2 ;;
        *) TARGETS+=("$a") ;;
    esac
done
[ "${#TARGETS[@]}" -gt 0 ] || TARGETS=(base daily gaming-nvidia)

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

Containerfile.base would silently produce an image whose kernel is unsigned,
which cannot boot with Secure Boot on and gives users nothing to enrol. That is
too easy to ship by accident, so this script refuses instead.

Either put apex-mok.key and apex-mok.crt in $SIGNDIR (or set APEX_SIGNING_DIR),
or pass --allow-unsigned if you genuinely want an unsigned image.
EOF
    exit 1
fi

REV="$(git rev-parse HEAD 2>/dev/null || echo unknown)"

build_base() {
    echo "== base =="
    sudo podman build --isolation=chroot \
        "${SECRET_ARGS[@]}" \
        --build-arg APEX_REVISION="$REV" \
        -f Containerfile.base -t localhost/apex-os-base:latest .

    # Assert rather than trust. The Containerfile degrades to `unsigned` on any
    # signing failure, so a green build is not evidence the kernel is signed —
    # check the marker AND the real PE signature, and check the cert users need
    # to enrol actually shipped.
    if [ "${#SECRET_ARGS[@]}" -gt 0 ]; then
        st=$(sudo podman run --rm --entrypoint /bin/sh localhost/apex-os-base:latest \
               -c 'cat /usr/share/apex-os/secureboot/kernel-signed 2>/dev/null || echo missing')
        [ "$st" = signed ] || { echo "FATAL: signing key was supplied but the image is stamped '$st'"; exit 1; }
        sudo podman run --rm --entrypoint /bin/sh localhost/apex-os-base:latest -c \
            'dnf5 -y install -q sbsigntools >/dev/null 2>&1; sbverify --list /usr/lib/modules/$(cat /usr/lib/apex-kver)/vmlinuz' \
            | grep -qi 'signature certificates\|image signature issuers\|APEX' \
            || { echo "FATAL: sbverify found no signature on the built vmlinuz"; exit 1; }
        sudo podman run --rm --entrypoint /bin/sh localhost/apex-os-base:latest \
            -c 'test -s /usr/share/apex-os/secureboot/apex-mok.der' \
            || { echo "FATAL: apex-mok.der missing — users would have nothing to enrol"; exit 1; }
        echo "base: kernel signed, apex-mok.der present"
    fi
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

    # Editions inherit signing from the base, so this catches building an
    # edition on top of a stale unsigned base — the same hole the CI edition
    # jobs now cover.
    if [ "${#SECRET_ARGS[@]}" -gt 0 ]; then
        st=$(sudo podman run --rm --entrypoint /bin/sh "localhost/apex-os:$f" \
               -c 'cat /usr/share/apex-os/secureboot/kernel-signed 2>/dev/null || echo missing')
        [ "$st" = signed ] \
            || { echo "FATAL: $f is stamped '$st' — its base is not signed. Rebuild the base."; exit 1; }
        echo "$f: kernel signed"
    fi
}

for t in "${TARGETS[@]}"; do
    if [ "$t" = base ]; then build_base; else build_flavor "$t"; fi
done
echo "done: ${TARGETS[*]}"
