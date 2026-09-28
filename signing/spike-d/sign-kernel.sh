#!/usr/bin/env bash
#
# sign-kernel.sh — sign an EFI PE image (kernel bzImage w/ EFI stub, a UKI,
# shim, or any EFI app) with the Rime Secure Boot key using sbsign.
#
# Same command the CI image pipeline (M1/M5) will run to sign kernels/UKIs
# before they are shipped in a Rime OS image.
#
# Usage:  sign-kernel.sh SRC_EFI OUT_EFI [KEY CERT]
#   SRC_EFI   input PE/EFI image to sign
#   OUT_EFI   output (signed) image path
#   KEY       private key      (default: $RIME_KEY or ./rime-mok.key)
#   CERT      signing cert PEM  (default: $RIME_CERT or ./rime-mok.crt)
#
set -euo pipefail

SRC="${1:?need source EFI image}"
OUT="${2:?need output path}"
KEY="${3:-${RIME_KEY:-rime-mok.key}}"
CERT="${4:-${RIME_CERT:-rime-mok.crt}}"

for f in "$SRC" "$KEY" "$CERT"; do
  [[ -f "$f" ]] || { echo "missing: $f"; exit 1; }
done

echo ">> sbsign $SRC -> $OUT"
sbsign --key "$KEY" --cert "$CERT" --output "$OUT" "$SRC"

echo ">> sbverify against signing cert:"
sbverify --cert "$CERT" "$OUT"
