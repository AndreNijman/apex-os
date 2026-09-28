# signing

**PRIVATE KEYS ARE NEVER COMMITTED TO THIS REPO.** CI injects them at build time from repository secrets only, and `.gitignore` excludes anything matching a private-key pattern.

This directory holds `spike-d/`, the M0 scripts that proved the Rime OS Secure Boot signing chain in a QEMU/OVMF VM. The production signing lives in `Containerfile.core`: `build-image.yml` decodes the `RIME_SB_KEY_B64` and `RIME_SB_CRT_B64` secrets and mounts them as the build secrets `rime_sb_key` and `rime_sb_crt`; core signs the kernel with `sbsign` and every out-of-tree module with the kernel's `sign-file`, and writes the public certificate into the image at `/usr/share/rime-os/secureboot/rime-mok.der` for MOK enrollment.
