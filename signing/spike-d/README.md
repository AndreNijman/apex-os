# signing/spike-d — Secure Boot signing-chain proof (M0 Spike D)

Reusable, parameterized scripts that prove the APEX-OS Secure Boot signing
chain end to end in a QEMU/OVMF VM: our own key signs a kernel that boots under
**SB enforcing**, while unsigned / foreign-signed kernels are refused by the
firmware. These are the reference commands for the CI image-signing pipeline
(M1/M5).

**No key material lives here.** Scripts write keys/certs to an out-of-tree work
dir you pass in. `.gitignore` blocks private-key patterns repo-wide.

## Scripts

| Script | Purpose |
|--------|---------|
| `keygen.sh [OUTDIR]` | Generate the APEX test signing keypair + self-signed cert (PEM + DER). Stands in for the future APEX MOK/db key (HSM/CI secret in production). |
| `enroll-vars.sh CERT_DER OUT_VARS [TEMPLATE]` | Build an SB-enforcing OVMF varstore with the APEX cert enrolled as PK+KEK+db, SecureBoot ON, no Microsoft keys (headless; no MokManager). `WITH_MICROSOFT=1` also enrolls MS UEFI CA/KEK. |
| `sign-kernel.sh SRC OUT [KEY CERT]` | `sbsign` a kernel/UKI/EFI app with the APEX key and `sbverify` the result. |
| `boot-sb-vm.sh --kernel … --initramfs … --loader … --vars … ` | Build a throwaway FAT ESP, boot it under `OVMF_CODE.secure` + the enrolled VARS in QEMU (SMM on, headless), capture serial, hard-timeout. |
| `sign-module.sh MODULE.ko [KEY CERT …]` | Sign an out-of-tree kmod with the APEX key via the kernel's `scripts/sign-file` (pipeline reference; see enforcement caveat in the header + `docs/m0-results.md`). |

## Quick run (see docs/m0-results.md for full evidence)

```sh
WORK=~/apex-os-m0-work/spike-d; mkdir -p "$WORK"/keys "$WORK"/runs
export VFV="$WORK/venv/bin/virt-fw-vars"     # virt-firmware in a venv

# 1. key
./keygen.sh "$WORK/keys"
# 2. SB-enforcing varstore (APEX key only)
./enroll-vars.sh "$WORK/keys/apex-mok.der" "$WORK/apex-VARS.ours-only.4m.fd"
# 3. sign a kernel (bzImage w/ EFI stub)
./sign-kernel.sh /boot/vmlinuz-X "$WORK/vmlinuz-apex-signed.efi" \
    "$WORK/keys/apex-mok.key" "$WORK/keys/apex-mok.crt"
# 4. boot it under SB enforcing  -> boots
./boot-sb-vm.sh --kernel "$WORK/vmlinuz-apex-signed.efi" \
    --initramfs "$WORK/apex-initramfs.cpio.gz" \
    --loader "$WORK/shell-apex-signed.efi" \
    --vars "$WORK/apex-VARS.ours-only.4m.fd" --name pos --outdir "$WORK/runs"
```
