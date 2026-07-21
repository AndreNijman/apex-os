#!/usr/bin/env bash
#
# keygen.sh — generate the APEX-OS Secure Boot signing keypair (TEST KEY).
#
# Spike D (M0) proof-of-mechanics helper. Produces a self-signed X.509
# certificate + RSA private key that plays the role of the future "APEX MOK"
# / db signing key. The private key NEVER leaves the output directory and is
# NEVER committed (repo .gitignore blocks *.key / *.pem / *.der patterns are
# handled by callers keeping material out of the tree).
#
# For real releases the private key is an HSM/CI secret; this script is the
# reference for what shape of key the pipeline expects.
#
# Usage:  keygen.sh [OUTDIR]
#   OUTDIR  directory to write key material into (default: $PWD)
#
# Env overrides:
#   APEX_KEY_CN     certificate common name
#   APEX_KEY_BITS   RSA key size (default 2048; Secure Boot db keys are 2048/4096)
#   APEX_KEY_DAYS   cert validity in days (default 3650)
#   APEX_GUID_FILE  path to write/read the owner GUID (default OUTDIR/apex-guid.txt)
#
set -euo pipefail

OUTDIR="${1:-$PWD}"
CN="${APEX_KEY_CN:-APEX-OS TEST Secure Boot key (SPIKE-D, DO NOT TRUST)}"
BITS="${APEX_KEY_BITS:-2048}"
DAYS="${APEX_KEY_DAYS:-3650}"

mkdir -p "$OUTDIR"
KEY="$OUTDIR/apex-mok.key"      # private key  (SECRET — never commit)
CRT="$OUTDIR/apex-mok.crt"      # self-signed cert, PEM   (public)
DER="$OUTDIR/apex-mok.der"      # same cert, DER          (public; for firmware enroll)
GUID_FILE="${APEX_GUID_FILE:-$OUTDIR/apex-guid.txt}"

# Stable owner GUID for the enrolled signature (identifies who owns the db entry).
if [[ ! -f "$GUID_FILE" ]]; then
  if command -v uuidgen >/dev/null 2>&1; then uuidgen > "$GUID_FILE"
  else cat /proc/sys/kernel/random/uuid > "$GUID_FILE"; fi
fi
GUID="$(tr -d '[:space:]' < "$GUID_FILE")"

echo ">> Generating RSA-$BITS keypair + self-signed cert"
echo "   CN   : $CN"
echo "   GUID : $GUID"

openssl req -new -x509 -newkey "rsa:$BITS" -nodes \
  -keyout "$KEY" -out "$CRT" \
  -days "$DAYS" -sha256 \
  -subj "/CN=$CN/"

# DER form is what UEFI variable stores / virt-fw-vars want for enrollment.
openssl x509 -in "$CRT" -outform DER -out "$DER"
chmod 600 "$KEY"

echo ">> Wrote:"
echo "   $KEY  (PRIVATE — do not commit)"
echo "   $CRT  (PEM cert)"
echo "   $DER  (DER cert, for firmware enroll)"
openssl x509 -in "$CRT" -noout -subject -issuer -fingerprint -sha256
