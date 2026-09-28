#!/usr/bin/env bash
#
# enroll-vars.sh — build an SB-enforcing OVMF variable store with the Rime
# test certificate enrolled as PK + KEK + db, Secure Boot switched ON, and
# (by default) NO Microsoft keys. This is the headless, scriptable equivalent
# of enrolling a key in firmware setup / MokManager — no interactive UI.
#
# Result: firmware that will boot ONLY images signed by the Rime key.
#   * Rime-signed image        -> allowed
#   * unsigned image           -> rejected (Security Violation)
#   * differently-signed image -> rejected (Access Denied / not in db)
#   * Microsoft/Fedora-signed  -> rejected UNLESS --with-microsoft is passed
#
# Usage:  enroll-vars.sh CERT_DER OUT_VARS [TEMPLATE_VARS]
#   CERT_DER       Rime cert in DER form (from keygen.sh)
#   OUT_VARS       output VARS file to create
#   TEMPLATE_VARS  pristine OVMF VARS template
#                  (default: /usr/share/edk2/x64/OVMF_VARS.4m.fd)
#
# Env:
#   RIME_GUID        owner GUID for enrolled sigs (default: read rime-guid.txt
#                    beside CERT_DER, else a fixed test GUID)
#   VFV              path to virt-fw-vars (default: looks on PATH)
#   WITH_MICROSOFT   set to 1 to ALSO enroll Microsoft UEFI CA + KEK
#                    (lets stock distro shim/kernels boot too)
#
set -euo pipefail

CERT_DER="${1:?need Rime cert (DER)}"
OUT_VARS="${2:?need output VARS path}"
TEMPLATE="${3:-/usr/share/edk2/x64/OVMF_VARS.4m.fd}"

VFV="${VFV:-virt-fw-vars}"
command -v "$VFV" >/dev/null 2>&1 || { echo "virt-fw-vars not found (set VFV=)"; exit 1; }

# owner GUID
if [[ -n "${RIME_GUID:-}" ]]; then
  GUID="$RIME_GUID"
elif [[ -f "$(dirname "$CERT_DER")/rime-guid.txt" ]]; then
  GUID="$(tr -d '[:space:]' < "$(dirname "$CERT_DER")/rime-guid.txt")"
else
  GUID="18dbe522-6169-48c8-b28c-702b8124f6f3"
fi

[[ -f "$TEMPLATE" ]] || { echo "template VARS not found: $TEMPLATE"; exit 1; }

MS_ARGS=(--no-microsoft)
if [[ "${WITH_MICROSOFT:-0}" == "1" ]]; then
  MS_ARGS=(--microsoft-db all --microsoft-kek all)
fi

echo ">> Enrolling Rime cert as PK/KEK/db (owner $GUID)"
echo "   template : $TEMPLATE"
echo "   microsoft: ${WITH_MICROSOFT:-0}"

"$VFV" \
  --input  "$TEMPLATE" \
  --set-pk  "$GUID" "$CERT_DER" \
  --add-kek "$GUID" "$CERT_DER" \
  --add-db  "$GUID" "$CERT_DER" \
  "${MS_ARGS[@]}" \
  --secure-boot \
  --output "$OUT_VARS"

echo ">> Wrote $OUT_VARS"
echo ">> Verifying enrolled variables:"
"$VFV" --input "$OUT_VARS" --print | grep -Ei 'secure ?boot|SetupMode|^PK|^KEK|^db|PlatformKey' || true
