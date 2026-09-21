# sdboot-xbootldr — can Type #1 entries live on an XBOOTLDR partition?

items: none (no task id; dispatched as a design question)
repo: apex-os
worktree: /var/tmp/apex-work/wt-sdboot-xbootldr
branch: task/sdboot-xbootldr, cut from roadmap/v2.2 @ 76aa2b95 — 7 commits, all pushed
lab: /var/lab-scratch/sdboot-xbootldr
evidence: ROADMAP/evidence/sdboot-xbootldr-20260921.md

## THE ANSWER, IN ONE LINE

**No. bootc's composefs backend writes the kernel and initramfs to the ESP
unconditionally and has never implemented XBOOTLDR.** The idea is sound —
systemd-boot reads that layout perfectly well, and its own binary carries
`config_load_xbootldr` — but sd-boot is not the actor.

## How it was established

Read the source, not the binary. `strings` was how the previous round reached
the same conclusion and it is not evidence: the XBOOTLDR GUID const *is* in
bootc's source (`discoverable_partition_specification.rs:482`) and is absent
from the binary only because nothing reachable references it.

**Five bootc revisions, identical in all of them**: 1.16.4, 1.16.10 (L16 host),
**1.16.11** (what `apex-os:daily` actually ships — read out of a booted guest),
1.16.13, and upstream `main` @ `65321ae`.

* `spec.rs:288` — `--bootloader systemd` → `BootloaderKind::BLSCompatible`.
* `bootc_composefs/boot.rs:751` vs `:775` — the two arms of
  `setup_composefs_bls_boot` are **not symmetric**. `GRUBClassic` asks
  `root.is_mountpoint("boot")`; `BLSCompatible` never asks, calls
  `mount_esp_writable()` and sets `abs_entries_path = /EFI/Linux`.
* `boot.rs:1422` — `// TODO: support XBOOTLDR … bootc does not yet detect or use
  XBOOTLDR in the composefs install path`.
* `bootloader.rs:287` — `// If we supported XBOOTLDR in the future, that'd go
  here with --boot-path.`
* `store/mod.rs:396` — `// NOTE: Handle XBOOTLDR partitions here if and when we
  use it`, in the **runtime** store. This kills the obvious workaround (let
  bootc write to the ESP, then move the files): `boot_dir` is the ESP for every
  later `upgrade`, `status`, `rollback` and `image delete`, so a relocation
  breaks the first steady-state update and would have to be redone forever.
* `install/config.rs:74` — `xbootldr` and `esp` are commented-out TODOs in a
  `deny_unknown_fields` struct, so an install config naming either is
  **rejected**. No way to ask for an XBOOTLDR, no way to ask for a bigger ESP.
* bootc PR **#2440** (merged 2026-09-16): *"GrubCC and SystemdBoot both do not
  store anything inside of /sysroot/boot and will (should) always have the ESP
  mounted at /boot."* The separate-`/boot` support upstream does have was added
  for GRUB and deliberately not for systemd-boot.

## The premise was wrong twice, and that matters more than the answer

1. **APEX is already on Type #1.** `BootType::Bls` is `#[default]`;
   `docs/boot-v2.md` already calls it *phase 1*. There is nothing to switch to.
2. **The 1.1 GiB is three deployments, not two UKIs.** Measured in the guest:
   `vmlinuz` 16 910 408 B + `initramfs.img` 376 456 492 B = **375 MiB**,
   `need = per*3 + 48 MiB` = **1173 MiB**.
3. **A UKI is not the answer either** — same kernel, same initramfs, plus a stub
   and section headers, so marginally *larger*. UKI vs Type #1 is a signing and
   measurement decision, not a space one.

The one lever inside bootc that does cut ESP cost is
`find_vmlinuz_initrd_duplicate` (`boot.rs:533`): a new deployment whose
vmlinuz+initrd digest matches an existing directory reuses it and writes
nothing. Whether that ever fires for APEX is **unmeasured** and depends on
initramfs reproducibility — see NEXT.

## Secure Boot: the delta is zero

ESP vs XBOOTLDR is the same bytes on a different partition. The real delta is
Type #1 vs UKI (initrd outside the signature, no `.pcrsig`, no PCR 11) and APEX
already pays it. `secure-boot-unsigned-loader` is about `systemd-bootx64.efi`
itself; XBOOTLDR never touched it. PCR 11 is unusable on a shipped APEX machine
anyway (`luks-enroll`).

## katana — Andre's mid-unit requirement, and the correction it needs

**The brief's premise is wrong in APEX's favour.** It assumed `find_esp()`
resolves the ESP the machine currently boots from and asked for it to prefer the
machine's own. **It already prefers the machine's own**: `find_esp` walks up
from `findmnt --target /sysroot` to a disk and scans *that* disk for the ESP
type GUID — the comment in the file was written for katana specifically. bootc
agrees from its own source: `find_first_colocated_esp` searches
`find_all_roots()`, the disks backing the root device.

So on katana both pick `nvme0n1p2` — the APEX disk's own unused 512 MiB
`EFI-SYSTEM` — not the 200 MiB ESP on the Windows disk it boots from today.
**Migrating moves APEX's boot onto its own disk as a side effect of migrating at
all.** Measured in a two-disk lab guest: with an ESP on each disk,
`find_esp() = /dev/vda2`, the root's.

**Windows is untouched**: the second disk's three files hash identically before
and after. Structurally the engine only ever mounts `find_esp()`'s partition,
and that cannot be on a disk the root is not on.

**What was missing and now is not**: the user was never told their boot moved
disks. The precheck now reads the PARTUUID out of `BootCurrent`'s device path
and prints a note when it differs from the ESP being written. **A note, not a
refusal** — against `sdboot-migrate`'s NEXT #5, which proposed refusing.
Refusing keeps katana on the Windows disk forever, and the case a refusal would
guard (firmware that cannot see the new ESP) is already handled: the trial boot
returns to GRUB, confirm records `failed`, nothing re-arms.

**katana still refuses today: 512 MiB against 1173 MiB.** Solving the engine's
rule for the initramfs, `vmlinuz` at 16.1 MiB:

| machine | ESP | largest initramfs that migrates |
| --- | --- | --- |
| **katana** | 512 MiB | **≤ 138.6 MiB** |
| L16 today | 600 MiB | ≤ 168.6 MiB |
| L16 with `p2` absorbed | 2648 MiB | ≤ 866 MiB (not a constraint) |

**For `initramfs-slim`: the rule is THREE deployments, not two.** Its stated
target ("~120 MiB so two deployments fit 512") lands correctly but is sized
against the wrong constraint — two would allow 215.9 MiB, three allows 138.6.
At 120 MiB katana migrates with 56 MiB spare; at 160 MiB, comfortably inside a
"two deployments" target, it does not.

## What the lab measured

`apexgrub.img`: `apex-os:daily` installed through `tests/lab/bootc-install-lab`
(so `--generic-image`, under `nvram-guard`, `verdict: verified` both runs) onto
45 GiB, **btrfs**, no `--bootloader` → **ostree + GRUB**, plus a 2 GiB `EA00`
XBOOTLDR in the tail and a second disk carrying katana's 200 MiB Windows ESP.
Firmware read out of the binary: `edk2-2970e5699ba6`.

Two refusals, in order:

1. **`no-rsync`** — `apex-os:daily` ships no `rsync`, so that is the *first*
   refusal on a real APEX machine today, not `esp-too-small`.
   `Containerfile.base` on `roadmap/v2.2` asserts rsync, so the next image build
   fixes it; until one exists, this is what a user gets.
2. **`esp-too-small`** (with a stub rsync only to reach it; the run stages
   nothing so rsync is never called) — 503 MiB free against 1173 MiB, and the
   new XBOOTLDR note naming `/dev/vda4`. The XBOOTLDR is still **0 files**
   afterwards.

**Not measured**: the stage/commit/trial-boot/confirm on the APEX image and on
btrfs (that is `sdboot-migrate-2`'s NEXT #1 — hand it `apexgrub.img`); the
cross-disk note end to end (the `ctl.img` engine predated it and the guest's
NVRAM was fresh; covered by tests and by the L16 read-only run); Secure Boot
(non-secboot OVMF).

## What landed — 7 commits, all pushed, none merged

* `9b91fa6e` the evidence · `bc55e032` `esp-too-small` names the XBOOTLDR it
  cannot use, `find_xbootldr()`, `docs/boot-v2.md` gains the XBOOTLDR section and
  loses the `strings`-absence argument, the stale "back up and reinstall" section
  is marked superseded · `861f51d3` the install-config finding · `49fbada3`
  sd-boot's own `config_load_xbootldr` · `ea302795` naming the actor ·
  `302bb31d` UKI-vs-Type#1 cost and the dedup lever · `1d5e7f24` the cross-disk
  ESP note and katana.

`test-boot-migrate` **61 → 71**, ten new assertions, **eight mutations checked**
(wrong GUID; note dropped; finder never matches; finder matches everything; note
turned into a refusal; parser drops the GPT form; parser ignores BootCurrent;
precheck never calls it). Shellcheck clean. `check-doc-verbs`,
`check-containerfile-assertions`, `check-no-conflict-markers`,
`check-suites-run-in-ci` all pass.

## VERDICTS

**Would the L16 still refuse?** Yes, and XBOOTLDR removes neither reason.
`secure-boot-unsigned-loader` fires first (Secure Boot on, sd-boot unsigned);
`esp-too-small` second (600 MiB against 1173). `p2` and `p4` stay as useless to
this path as they are now. The levers are **shrink the initramfs**
(`initramfs-slim`/`kernel-build` — every MiB is worth three) or **delete the dead
`p2` and grow `p1`** (Andre's call; `p2` is contiguous with the ESP).

**A Windows dual-boot machine?** Nothing changes. `windows-installer` measured a
stock shared Windows ESP at 96.0 / 27.7 used / **68.3 MiB free**. sd-boot's
136 KiB fits; 375 MiB per deployment does not, and cannot be diverted to an
XBOOTLDR the installer creates. The "second ESP or stay on GRUB" fork stands —
though at a 120 MiB initramfs a second APEX-owned ESP need only be ~512 MiB.

## NEXT — for a stranger

1. **Do not reopen the XBOOTLDR question without new upstream code.** The six
   sites are `boot.rs:775`, `boot.rs:1422`, `bootloader.rs:287`,
   `store/mod.rs:396`, `status.rs:408`, `finalize.rs:149`/`delete.rs:159`, all
   keyed off the same `boot_dir` decision. Watch bootc for a PR touching
   `esp_subdir`. Do **not** carry it as an APEX fork.
2. **Does `find_vmlinuz_initrd_duplicate` ever fire for APEX?** If the initramfs
   were byte-reproducible across image builds, an update that does not change
   the kernel would cost **zero** additional ESP and `per*3` would stop being
   the binding number. Nobody has checked; dracut is not reproducible by
   default. Worth more than any partition layout. For `initramfs-slim`.
3. **`apex-os:daily` ships no `rsync`** — measured in the guest, the first
   refusal a real APEX machine gets. Fixed by the next image build
   (`Containerfile.base` asserts it on `roadmap/v2.2`); until then, expect
   `no-rsync` and not `esp-too-small`.
4. **The `bootc-too-old` gate is a `grep -q` under `pipefail`**
   (`apex-boot-migrate:298`). Measured five times on the L16: rc=0, because the
   help is 5874 bytes and fits the pipe buffer. Latent, not live — if that help
   grows past 64 KiB every machine refuses `bootc-too-old` with a working bootc.
   Same family as the recorded `printf | grep -q` → 141 trap. One-line fix
   (`out="$(… --help 2>/dev/null)"; case "$out" in …`), deliberately not taken
   because it is another unit's file and it is not firing.
5. **Hand `sdboot-migrate-2` the guest rather than letting it build one.**
   `/var/lab-scratch/sdboot-xbootldr/apexgrub.img` **is** `apex-os:daily`,
   ostree + GRUB, **btrfs**, 512 MiB ESP, `--root-size 40G`, **with** a 2 GiB
   `EA00` XBOOTLDR at p4 and the labrun unit injected into the deployment's
   `/etc`. `windisk.img` is katana's second disk; `ctl.img` carries `action.sh`
   and the engine (rebuild it after editing either — `mkfs.ext4 -d ctl/`).
   Harness: `boot-mig.sh IMG SERIAL TIMEOUT --oci windisk.img --ctl ctl.img`.
   It cost ~40 minutes to build.
6. **Close the cross-disk note end to end.** Boot the guest once, run
   `efibootmgr --create --disk /dev/vdb --part 1 --loader '\EFI\APEX\SHIMX64.EFI'`
   and make it BootCurrent, then re-run the precheck with a current `ctl.img`.
   The note is tested against both real `efibootmgr -v` spellings but has never
   fired in a guest.
7. **`check-shellcheck-coverage` already fails on `roadmap/v2.2`**, before this
   branch: `android/tools/release-version.sh:92`, SC2034, `head_sha` unused.
   Reproduced against `76aa2b95` itself. Not this unit's file; whoever lands
   next hits it.
8. **A version read from `podman run <registry ref>` can be months stale.**
   Rootless and root podman keep separate storages and neither asks the registry
   when a local copy exists. The rootless `apex-os:daily` here is **7 weeks**
   old and answered `bootc 1.16.4`; the freshly pulled one in the guest says
   **1.16.11**. Read image facts out of a booted guest, or `podman pull` first.
9. **The two-ESP question is now half-owned.** `find_esp` and bootc agree on the
   root's disk, and the user is told when that differs from BootCurrent. What is
   still unwritten: what `confirm` should do with the *old* entry on the other
   disk (today it is demoted in `BootOrder` and left, which is right for
   Windows), and whether a machine whose root disk has **no** ESP should say
   something better than `no-esp`.

## BOUNDS HONOURED

Nothing touched the L16's partitions, ESP, NVRAM or bootloader. Reads only:
`stat` on two files under `/usr/lib/modules`, `lsblk`, `efibootmgr -v` (prints
variables, writes none), and the engine's finders. The APEX composefs guest in
the evidence was mounted **read-only** from an image `sdboot-image` left behind.
Every install went through `tests/lab/bootc-install-lab` under `nvram-guard`;
every run returned `verdict: verified`. **katana was not touched at all** — its
layout is the orchestrator's measurement and the lab guest stands in for it.

## A LAB TRAP

The first guest boot died with `/work/boot-mig.sh: Permission denied`, exit 126
— not a mode bit. A fresh directory under `/var/lab-scratch` is `var_t`; a bind
mount into a container needs `container_file_t` (what `-v …:z` sets).
`sdboot-migrate`'s lab directory already had it, so the harness looked like it
just works. `sudo chcon -R -t container_file_t <labdir>`.
