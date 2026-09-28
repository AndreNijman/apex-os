# signing

**PRIVATE KEYS ARE NEVER COMMITTED TO THIS REPO.** CI injects them at build time from repository secrets only, and `.gitignore` excludes anything matching a private-key pattern.

This directory holds `spike-d/`, the M0 scripts that proved the APEX-OS Secure Boot signing chain in a QEMU/OVMF VM. The production signing lives in `Containerfile.core`: `build-image.yml` decodes the `APEX_SB_KEY_B64` and `APEX_SB_CRT_B64` secrets and mounts them as the build secrets `apex_sb_key` and `apex_sb_crt`; core signs the kernel with `sbsign` and every out-of-tree module with the kernel's `sign-file`, and writes the public certificate into the image at `/usr/share/apex-os/secureboot/apex-mok.der` for MOK enrollment.
