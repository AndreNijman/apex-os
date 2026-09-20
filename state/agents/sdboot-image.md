# sdboot-image — pivot APEX to systemd-boot on every machine

- **Repo / branch**: apex-os, `task/sdboot-image` (pushed).
- **Worktree**: `/var/tmp/apex-work/wt-sdboot-image`.
- **Evidence**: `ROADMAP/evidence/sdboot-image-20260920-lab.md` (predecessor's
  VM-lab transcripts), `ROADMAP/evidence/sdboot-image-20260921-decision.md`
  (this round's design decision + measurements).
- **Andre's decision**: fully pivot APEX to systemd-boot on all machines. Not
  ours to relitigate. The reasons GRUB was kept are now the constraints the
  design must satisfy.

## What the predecessor measured (2026-09-20 lab, all in the evidence file)

1. The **ostree backend cannot use systemd-boot** — `bootc install
   --bootloader systemd` errors `bootupd is required for ostree-based
   installs`. The pivot is a *storage-backend* change, not a bootloader flag.
2. The **composefs backend** with `--bootloader systemd` installs, boots under
   OVMF, and `bootc upgrade` produces a correct new entry.
3. `systemd-boot-unsigned` absent ⇒ bootc still exits 0 and leaves an
   **unbootable** ESP. Must be asserted in the image.
4. Boot counting: bootc writes none; renaming inside
   `/boot/loader/entries.staged/` **survives** `bootc-finalize-staged`, and the
   counter rolled back by itself at `+0-3`. Blessing not yet observed.
5. ESP capacity: ~374 MiB per deployment ⇒ ~1.1 GiB needed. **L16's ESP is
   600 MiB.** In-place migration is not a matter of writing a loader.
6. Legacy BIOS: the 1 MiB BIOS boot partition is created but **nothing is
   written into it** on the L16 (`bootupd-state.json` has only `EFI`).
7. Not measured: Secure Boot, UKIs on this path, the APEX image itself,
   migration, blessing, XBOOTLDR (ruled out by probe).

## Status

See `## NEXT` at the bottom — kept current for a stranger.
