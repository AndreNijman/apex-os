#!/usr/bin/env bash
#
# sign-kernel.sh — sign an EFI PE image (kernel bzImage w/ EFI stub, a UKI,
# shim, or any EFI app) with the APEX Secure Boot key using sbsign.
#
# Same command the CI image pipeline (M1/M5) will run to sign kernels/UKIs
# before they are shipped in an APEX-OS image.
#
# Usage:  sign-kernel.sh SRC_EFI OUT_EFI [KEY CERT]
#   SRC_EFI   input PE/EFI image to sign
#   OUT_EFI   output (signed) image path
#   KEY       private key      (default: $APEX_KEY or ./apex-mok.key)
#   CERT      signing cert PEM  (default: $APEX_CERT or ./apex-mok.crt)
#
set -euo pipefail

SRC="${1:?need source EFI image}"
OUT="${2:?need output path}"
KEY="${3:-${APEX_KEY:-apex-mok.key}}"
CERT="${4:-${APEX_CERT:-apex-mok.crt}}"

for f in "$SRC" "$KEY" "$CERT"; do
  [[ -f "$f" ]] || { echo "missing: $f"; exit 1; }
done

echo ">> sbsign $SRC -> $OUT"
sbsign --key "$KEY" --cert "$CERT" --output "$OUT" "$SRC"

echo ">> sbverify against signing cert:"
sbverify --cert "$CERT" "$OUT"
