# windows-installer-2

Branch `task/windows-installer-2`, worktree `/var/tmp/apex-work/wt-winst2`,
based on `roadmap/v2.2` @ `602a8376`. Pushed to origin at creation.

Large artefacts live in `/var/lab-scratch/winlab/` (never `/tmp` — it is RAM here).

## STATE — 2026-09-20 23:20 AWST

- Worktree created, branch pushed empty.
- Windows Server 2022 Evaluation ISO downloaded and size-verified:
  `/var/lab-scratch/winlab/ws2022-eval.iso`, 5044094976 bytes, from
  `https://go.microsoft.com/fwlink/p/?LinkID=2195280`.
  It is the right ISO: full Win32 storage stack, UEFI boot, no TPM gate, no
  Microsoft-account gate, unattended install via `autounattend.xml`.
- Architecture decision taken; see `windows-installer/ARCHITECTURE.md` on the
  branch once it lands.

## NEXT

Building `windows-installer/lab/` — a podman image with qemu-kvm + edk2-ovmf +
mtools + xorriso + wimlib + python3-virt-firmware, and the scripts that
remaster the ISO with `efisys_noprompt.bin` + `autounattend.xml`, run Setup
headless with `-no-reboot`, and produce a golden raw disk.

If this agent is dead, do not message it. Read this file, then continue from
`windows-installer/lab/` in the worktree above.
