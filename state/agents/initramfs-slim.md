# initramfs-slim — make the initramfs fit a 512 MiB ESP

items: none (no task id; dispatched as "it has to work in 512")
repo: apex-os
worktree: /var/tmp/apex-work/wt-initramfs-slim
branch: task/initramfs-slim, cut from roadmap/v2.2 @ 4031d43f
lab: /var/lab-scratch/initramfs-slim
evidence: ROADMAP/evidence/initramfs-slim-20260921.md (being written)

## STATE — round 2 (fresh agent, 2026-09-21 ~11:45 AWST)

Round 1 left `847b5bfb` (Containerfile.apex + .core + .kernel) and measured
artifacts in `/var/lab-scratch/initramfs-slim/out`. It explicitly did NOT do:
the evidence file, a VM boot proof, the reproducibility answer.

### Confirmed on arrival (checked, not assumed)

* Host kernel config `/usr/lib/modules/7.2.3-cachyos2.fc43.x86_64/config`:
  `CONFIG_DRM=y CONFIG_DRM_SIMPLEDRM=y CONFIG_SYSFB_SIMPLEFB=y
  CONFIG_DRM_FBDEV_EMULATION=y CONFIG_FRAMEBUFFER_CONSOLE=y` — all five `=y`.
  The Containerfile.kernel assertion CAN pass.
* This L16's own boot log: `[drm] Initialized simpledrm 1.0.0 for
  simple-framebuffer.0 on minor 0`. The GOP path works on the target hardware.
* Sizes (bytes): baseline 375,738,711 · varA 138,952,921 · varB 103,248,585 ·
  varB19 99,981,645 · varC 102,200,674.
  varB == repro/a.img == repro/b.img, sha256
  `704337bc653941d29f10a8baec5716be1177c7bba581310bceac7109ce8e578a`.

### DEFECT FOUND IN ROUND 1'S OWN GATE — fix before anything else

`Containerfile.apex` asserts `! grep -qE 'kernel/drivers/net/ethernet/'`.
The slim initramfs round 1 produced **contains**
`kernel/drivers/net/ethernet/broadcom/cnic.ko.zst`, `.../chelsio/cxgb4/cxgb4.ko.zst`
and `.../qlogic/qed/qed.ko.zst` (pulled in as deps of SCSI offload modules, not
by the `network` dracut module). So that gate FATALs on the image it exists to
pass — the "Containerfile assertions that cannot pass" family. Must be replaced
with a true predicate.

## NEXT

1. [done] read 847b5bfb, verify kernel config, verify repro hashes
2. [done] card
3. fix the ethernet gate; move the gates into a script both the Containerfile
   and a tests/ suite call, so both-ways can be demonstrated without a build
4. regenerate the size matrix with attribution (4 dracut runs, apex-tier flags)
5. cross-BUILD reproducibility (answers sdboot-xbootldr NEXT #2)
6. VM boot proof on a reflink copy of /var/lab-scratch/sdboot-xbootldr/apexgrub.img
7. evidence file + commit + push --force-with-lease

## CONSTRAINTS I AM UNDER

Never land on roadmap/v2.2. Never push main. Headless only. /var/lab-scratch
not /tmp. podman/qemu in the FOREGROUND.
