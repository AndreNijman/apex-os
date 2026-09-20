#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  verify-signing-identity.sh — is this the key we told everybody signs APEX?
#
#  WHY THIS EXISTS AT ALL. `verifyReleaseSigning` proves an artefact is signed.
#  It cannot prove it is signed by the RIGHT key, because the build's only idea
#  of the right key is whatever `APEX_KEYSTORE_BASE64` happened to decode to.
#  Swap that secret — by accident, during a rotation done half-way, or by
#  anyone who can write repository secrets — and every gate in the release
#  still passes while the published APK becomes uninstallable over the top of
#  every copy already on a phone. Android refuses an update signed by a
#  different key, and the only escape is uninstall, which destroys the paired
#  device key this app keeps. There is no server-side fix, so the check has to
#  happen before the file reaches a person.
#
#  The published fingerprint in `android/signing-certificate.sha256` is what
#  makes that checkable: it is in the repository, it is in the docs, and it is
#  what a user can compare their download against. This script is the same
#  comparison, run by the release.
#
#  TWO MODES, both used by release-artifacts.sh:
#
#      --keystore FILE --alias NAME     before the build. Two seconds, and a
#                                       wrong key stops the release before ten
#                                       minutes of Gradle. The password comes
#                                       from APEX_KEYSTORE_PASSWORD in the
#                                       environment — never an argument, which
#                                       `ps` would show to every other process.
#
#      --apk FILE                       after it. THIS is the evidence: the
#                                       certificate read back out of the file
#                                       that would be uploaded, not out of the
#                                       inputs that were supposed to produce it.
#
#  It never prints key material or a password. A certificate fingerprint is
#  public and is printed on purpose.
#
#  Exit: 0 they match, 1 refusal, 2 wrong usage.
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail

fatal() { echo "FATAL: $*" >&2; exit 1; }
usage() { sed -n '2,40p' "$0"; exit "${1:-2}"; }

apk=""; keystore=""; alias=""; pin=""; published_only=0
while [ $# -gt 0 ]; do
    case "$1" in
        --apk)      apk="${2:?--apk needs a value}"; shift 2 ;;
        --keystore) keystore="${2:?--keystore needs a value}"; shift 2 ;;
        --alias)    alias="${2:?--alias needs a value}"; shift 2 ;;
        --pin)      pin="${2:?--pin needs a value}"; shift 2 ;;
        # "Is there a published identity at all?" — the release's first-minute
        # gate, so a repository with no key refuses before the SDK is installed
        # rather than after the build.
        --published) published_only=1; shift ;;
        -h|--help)  usage 0 ;;
        *) echo "FATAL: unknown argument '$1'" >&2; exit 2 ;;
    esac
done

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
android=$(dirname "$here")
[ -n "$pin" ] || pin="$android/signing-certificate.sha256"

if [ -n "$apk" ] && [ -n "$keystore" ]; then
    echo "FATAL: --apk and --keystore are different checks; run one at a time" >&2; exit 2
fi
[ -n "$apk" ] || [ -n "$keystore" ] || [ "$published_only" = 1 ] \
    || { echo "FATAL: one of --apk, --keystore or --published is required" >&2; exit 2; }
[ -z "$keystore" ] || [ -n "$alias" ] || { echo "FATAL: --keystore needs --alias" >&2; exit 2; }

# ── The published fingerprints ───────────────────────────────────────────────
#
# Read first. A missing or unreadable pin file must stop the release rather
# than be treated as "nothing to check" — this repository's dominant CI defect
# is a gate that runs and inspects nothing.
[ -f "$pin" ] || fatal "$pin does not exist, so there is no published fingerprint to check the signing key against. A release cannot claim an identity it does not publish. See docs/android-signing.md."

# A fingerprint is normalised before it is validated, so the keytool spelling
# (uppercase, colon-separated) and the apksigner spelling (lowercase, bare) are
# the same value here rather than two that silently never match.
#
# The whole line is lowercased, not just A-F. `tr 'A-F' 'a-f'` turns UNSET into
# UNSeT — the E is in the range — and the sentinel then reads as a malformed
# fingerprint instead of as "no key exists yet", which is a different refusal
# with a different message.
normalise() { printf '%s' "$1" | tr -d ':[:space:]' | tr 'A-Z' 'a-z'; }

pins=(); unset_seen=0
while IFS= read -r line || [ -n "$line" ]; do
    line="${line%%#*}"
    line=$(normalise "$line")
    [ -n "$line" ] || continue
    if [ "$line" = "unset" ]; then unset_seen=1; continue; fi
    case "$line" in
        *[!0-9a-f]* | "") fatal "$pin contains '$line', which is not a SHA-256 fingerprint" ;;
    esac
    [ "${#line}" -eq 64 ] || fatal "$pin contains a fingerprint of ${#line} hex characters; a SHA-256 fingerprint is 64"
    pins+=("$line")
done < "$pin"

if [ "$unset_seen" = 1 ]; then
    [ "${#pins[@]}" -eq 0 ] \
        || fatal "$pin says UNSET *and* lists a fingerprint. One of the two is a leftover, and guessing which would be guessing about the key that decides whether every installed app can be updated."
    fatal "$pin says UNSET: no signing certificate has been published yet, so nothing can state which key a release is supposed to carry.

Generate the key with android/tools/generate-signing-key.sh, which writes the
fingerprint here, then commit it. Until then this refuses, and that refusal is
the point: an APK signed by a key nobody has published and nobody is keeping is
worse than no APK — every phone that installs it can only ever be updated by
that same key, and there is no way back except uninstalling. See
docs/android-signing.md."
fi
[ "${#pins[@]}" -gt 0 ] || fatal "$pin lists no fingerprint at all. An empty pin file is not 'no constraint', it is a release with no published identity."

if [ "$published_only" = 1 ]; then
    for p in "${pins[@]}"; do
        echo "published signing certificate: $p"
    done
    exit 0
fi

# ── What actually signed the thing in front of us ────────────────────────────
found=()

if [ -n "$keystore" ]; then
    [ -f "$keystore" ] || fatal "--keystore '$keystore' is not a file"
    [ -n "${APEX_KEYSTORE_PASSWORD:-}" ] \
        || fatal "APEX_KEYSTORE_PASSWORD is not set, so the keystore cannot be opened. The password is taken from the environment on purpose: an argument is visible in ps to every process on the machine."
    command -v keytool >/dev/null 2>&1 || fatal "keytool is not on PATH, so the keystore's certificate cannot be read. A run that could not look must fail rather than report that it found no problem."

    der=$(mktemp) || fatal "cannot create a temporary file"
    # `-storepass:env`, never `-storepass <value>`: same reason as above. The
    # alias is not secret and is an argument.
    if ! keytool -exportcert -keystore "$keystore" -alias "$alias" \
            -storepass:env APEX_KEYSTORE_PASSWORD > "$der" 2>/dev/null; then
        rm -f "$der"
        fatal "the keystore did not open with the supplied password, or it holds no key called '$alias'. Nothing about the key material is printed here on purpose."
    fi
    [ -s "$der" ] || { rm -f "$der"; fatal "keytool exported an empty certificate for '$alias'"; }
    # Written to a file and hashed, rather than piped: under `pipefail` a
    # pipeline hides which half failed, and this repository has been mis-seeded
    # by exactly that before.
    sum=$(sha256sum "$der") || { rm -f "$der"; fatal "sha256sum failed on the exported certificate"; }
    rm -f "$der"
    found+=("${sum%% *}")
    subject="the keystore's certificate for alias '$alias'"
else
    [ -f "$apk" ] || fatal "--apk '$apk' is not a file"
    signer="${APKSIGNER:-}"
    if [ -z "$signer" ]; then
        sdk="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}"
        [ -n "$sdk" ] || fatal "neither APKSIGNER nor ANDROID_HOME/ANDROID_SDK_ROOT is set, so apksigner cannot be found and the APK's certificate cannot be read."
        # Newest build-tools, chosen by version order rather than by whatever
        # the shell happens to glob first.
        for cand in $(ls -1 "$sdk/build-tools" 2>/dev/null | sort -V); do
            [ -x "$sdk/build-tools/$cand/apksigner" ] && signer="$sdk/build-tools/$cand/apksigner"
        done
    fi
    [ -n "$signer" ] && [ -x "$signer" ] \
        || fatal "apksigner was not found under ${ANDROID_HOME:-${ANDROID_SDK_ROOT:-<no SDK>}}/build-tools. A run that could not look must fail rather than report that it found no problem."

    certs=$(mktemp) || fatal "cannot create a temporary file"
    if ! "$signer" verify --print-certs "$apk" > "$certs" 2>&1; then
        echo "--- apksigner ---" >&2
        grep -v '^WARNING' "$certs" >&2
        rm -f "$certs"
        fatal "apksigner refused $apk. It is not signed, or its signature does not verify, so its signing identity cannot be established."
    fi
    # Both spellings apksigner uses. An unrotated APK prints
    #   "Signer #1 certificate SHA-256 digest: <hex>"
    # and a rotated one, which carries a lineage, prints one line per signer:
    #   "Signer (minSdkVersion=33, maxSdkVersion=2147483647) certificate SHA-256 digest: <hex>"
    # Measured with build-tools 36.0.0 rather than read off documentation.
    while IFS= read -r d; do found+=("$d"); done < <(
        sed -n 's/.*certificate SHA-256 digest: *\([0-9a-fA-F]\{64\}\).*/\1/p' "$certs" | tr 'A-F' 'a-f'
    )
    rm -f "$certs"
    [ "${#found[@]}" -gt 0 ] \
        || fatal "apksigner verified $apk but printed no certificate digest, so nothing was actually compared. That is a check that inspects nothing, and it fails."
    subject="the APK's signer(s)"
fi

# ── Compare ──────────────────────────────────────────────────────────────────
#
# Set EQUALITY for an APK, not "at least one matches". A published APK must be
# signed by exactly the certificates this repository publishes: an extra signer
# is a key nobody named, and a missing one is a key some installed copy is
# expecting. For a keystore it is membership — one keystore holds one of the
# published certificates, and after a rotation there are two keystores.
in_list() { local want="$1" n; shift; for n in "$@"; do [ "$n" = "$want" ] && return 0; done; return 1; }

fail=0
for f in "${found[@]}"; do
    if ! in_list "$f" "${pins[@]}"; then
        echo "FATAL: $subject is $f, which is NOT the published signing certificate." >&2
        echo "       Published in $pin:" >&2
        for p in "${pins[@]}"; do echo "         $p" >&2; done
        echo "       Refusing. An APK signed by an unpublished key can never be installed over one already on a phone, and the user's only escape is to uninstall — which destroys their pairing. See docs/android-signing.md." >&2
        fail=1
    fi
done
if [ -n "$apk" ]; then
    for p in "${pins[@]}"; do
        if ! in_list "$p" "${found[@]}"; then
            echo "FATAL: $pin publishes $p, and the APK is not signed by it." >&2
            echo "       Every phone holding a build signed by that certificate would refuse this update. If this is a deliberate rotation, the lineage must carry the old key — see the rotation section of docs/android-signing.md." >&2
            fail=1
        fi
    done
fi
[ "$fail" = 0 ] || true

for f in "${found[@]}"; do
    echo "signing certificate verified: $f"
done
