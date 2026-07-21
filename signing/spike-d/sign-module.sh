#!/usr/bin/env bash
#
# sign-module.sh — sign an out-of-tree kernel module (.ko) with the APEX key,
# using the in-kernel `scripts/sign-file` helper. This is the reference command
# the image pipeline (M1/M5) runs for every kmod shipped in an APEX-OS image.
#
# NOTE ON ENFORCEMENT (see docs/m0-results.md, Spike D):
# Module signatures are verified against the kernel's *keyring*
# (.builtin_trusted_keys / .secondary_trusted_keys / .machine), NOT against the
# UEFI db used for the boot chain. For the kernel to *accept* an APEX-signed
# module, the APEX public key must be either:
#   (a) built into the APEX kernel via CONFIG_SYSTEM_TRUSTED_KEYS / bundled as
#       an additional MODULE_SIG_KEY at kernel build time, or
#   (b) enrolled as a MOK and linked into the .machine keyring (needs shim +
#       CONFIG_INTEGRITY_MACHINE_KEYRING, and CONFIG_MODULE_SIG=y).
# UEFI-db enrollment alone (what Spike D's VM uses for the boot chain) does NOT
# make the kernel trust the key for modules.
#
# Usage:  sign-module.sh MODULE.ko [KEY CERT [HASH [SIGN_FILE]]]
#   MODULE.ko   module to sign in place (must be decompressed .ko, not .ko.zst)
#   KEY         private key   (default: $APEX_KEY or ./apex-mok.key)
#   CERT        signing cert  (default: $APEX_CERT or ./apex-mok.crt; sign-file
#               wants DER — this wrapper converts PEM->DER automatically)
#   HASH        digest        (default: sha512, matching the kernel's
#               CONFIG_MODULE_SIG_HASH)
#   SIGN_FILE   path to sign-file (default: /lib/modules/$(uname -r)/build/scripts/sign-file)
#
set -euo pipefail

MOD="${1:?need module .ko}"
KEY="${2:-${APEX_KEY:-apex-mok.key}}"
CERT="${3:-${APEX_CERT:-apex-mok.crt}}"
HASH="${4:-sha512}"
SIGN_FILE="${5:-/lib/modules/$(uname -r)/build/scripts/sign-file}"

[[ -f "$MOD" ]]  || { echo "no module: $MOD"; exit 1; }
[[ -f "$KEY" ]]  || { echo "no key: $KEY"; exit 1; }
[[ -f "$CERT" ]] || { echo "no cert: $CERT"; exit 1; }
[[ -x "$SIGN_FILE" ]] || {
  echo "sign-file not found/executable: $SIGN_FILE"
  echo "install the kernel-devel/headers package for the target kernel."
  exit 1
}

# sign-file wants a DER certificate; accept PEM and convert on the fly.
DER_CERT="$CERT"
if head -c 32 "$CERT" | grep -q 'BEGIN CERTIFICATE'; then
  DER_CERT="$(mktemp --suffix=.der)"
  openssl x509 -in "$CERT" -outform DER -out "$DER_CERT"
fi

echo ">> sign-file $HASH $KEY <cert> $MOD"
"$SIGN_FILE" "$HASH" "$KEY" "$DER_CERT" "$MOD"

echo ">> signature appended; tail of module now shows the PKCS#7 marker:"
if tail -c 28 "$MOD" | grep -q 'Module signature appended'; then
  echo "   OK: '~Module signature appended~' trailer present"
else
  echo "   WARNING: expected signature trailer not found"
fi
