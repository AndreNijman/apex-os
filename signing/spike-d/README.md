# signing/spike-d: Secure Boot signing-chain proof (M0 Spike D)

Reusable, parameterized scripts that prove the Rime OS Secure Boot signing
chain end to end in a QEMU/OVMF VM: our own key signs a kernel that boots under
**SB enforcing**, and the firmware refuses unsigned or foreign-signed kernels.
These were the reference commands for the CI image-signing pipeline (M1/M5),
which now lives in `Containerfile.core`.

**No key material lives here.** The scripts write keys and certificates to an
out-of-tree work dir you pass in. `.gitignore` blocks private-key patterns
repo-wide.

## Scripts

| Script | Purpose |
|--------|---------|
| `keygen.sh [OUTDIR]` | Generate the Rime test signing keypair + self-signed cert (PEM + DER). Stands in for the Rime MOK/db key, which is a CI secret in production. |
| `enroll-vars.sh CERT_DER OUT_VARS [TEMPLATE]` | Build an SB-enforcing OVMF varstore with the Rime cert enrolled as PK+KEK+db, SecureBoot ON, no Microsoft keys (headless; no MokManager). `WITH_MICROSOFT=1` also enrolls MS UEFI CA/KEK. |
| `sign-kernel.sh SRC OUT [KEY CERT]` | `sbsign` a kernel/UKI/EFI app with the Rime key and `sbverify` the result. |
| `boot-sb-vm.sh --kernel … --initramfs … --loader … --vars … ` | Build a throwaway FAT ESP, boot it under `OVMF_CODE.secure` + the enrolled VARS in QEMU (SMM on, headless), capture serial, hard-timeout. |
| `sign-module.sh MODULE.ko [KEY CERT …]` | Sign an out-of-tree kmod with the Rime key via the kernel's `scripts/sign-file` (pipeline reference; see enforcement caveat in the header + `docs/m0-results.md`). |

## Quick run (see docs/m0-results.md for full evidence)

```sh
WORK=~/rime-os-m0-work/spike-d; mkdir -p "$WORK"/keys "$WORK"/runs
export VFV="$WORK/venv/bin/virt-fw-vars"     # virt-firmware in a venv

# 1. key
./keygen.sh "$WORK/keys"
# 2. SB-enforcing varstore (Rime key only)
./enroll-vars.sh "$WORK/keys/rime-mok.der" "$WORK/rime-VARS.ours-only.4m.fd"
# 3. sign a kernel (bzImage w/ EFI stub)
./sign-kernel.sh /boot/vmlinuz-X "$WORK/vmlinuz-rime-signed.efi" \
    "$WORK/keys/rime-mok.key" "$WORK/keys/rime-mok.crt"
# 4. boot it under SB enforcing  -> boots
./boot-sb-vm.sh --kernel "$WORK/vmlinuz-rime-signed.efi" \
    --initramfs "$WORK/rime-initramfs.cpio.gz" \
    --loader "$WORK/shell-rime-signed.efi" \
    --vars "$WORK/rime-VARS.ours-only.4m.fd" --name pos --outdir "$WORK/runs"
```
