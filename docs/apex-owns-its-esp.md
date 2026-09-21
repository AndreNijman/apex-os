# APEX owns its own ESP

**Decision (Andre, 2026-09-21):** *"windows side install should be like
everything else with the systemd-boot. maybe it should build a new esp for
apex or something."*

This settles the first of the two product questions the Windows installer unit
was told to leave alone. It is not a Windows-only decision — it removes a
special case rather than adding one.

## What was decided

1. **There is no ostree/GRUB variant for Windows machines.** Every APEX machine
   boots systemd-boot, from a UKI, through the same `bootc install
   --composefs-backend --bootloader systemd` path. One boot story, one set of
   tests, one failure mode to understand.
2. **APEX gets its own EFI System Partition. It does not write into Windows'.**
   Not "prefers not to" — the Windows ESP is read for facts and never written.

The second question — whether the tool may ever retype a basic-data partition
itself — is still Andre's and is **not** answered here.

## Why this is the right shape, and not a workaround

The 512 MiB problem was never really about 512 MiB. It was about *borrowing*.

A stock Windows ESP was measured at **68.3 MiB free** while composefs needs
about **1.1 GiB**. Every route out of that — XBOOTLDR, shrinking the initramfs
far enough to fit, sharing the loader directory — was an attempt to squeeze
APEX into a partition Microsoft sized for Microsoft, which also means competing
with it forever: a Windows feature update that grows `\EFI\Microsoft` reclaims
the slack, and the next `apex update` fails on a machine that worked yesterday.

Owning the partition ends that class of failure outright, and it is *also* the
safer option. The strongest guarantee this project can offer a dual-boot user
is that Windows' own boot path survives byte-identical, and the cheapest way to
guarantee that is to never open the file for writing.

## The mechanism already mostly exists

Three things are already true on `roadmap/v2.2`, checked rather than assumed:

- **`bootc install to-filesystem` uses the ESP the CALLER mounted.** Its own
  help says partitions "are prepared and mounted by an external tool or
  script". bootc does not go hunting; it writes where it is pointed. So
  choosing the ESP is entirely APEX's decision to make.
- **`apex-boot-migrate` already accepts `APEX_MIGRATE_ESP`** — "an already-
  mounted ESP, instead of finding one". The override needed to target a chosen
  partition is in the shipped tool today.
- **The tool already knows a machine can have two ESPs, and which is which.**
  `find_esp()` answers "which ESP belongs to this machine's root disk";
  `booted_esp_partuuid()` answers "which ESP did the firmware load the loader
  from". Its comment records that on katana those are different partitions on
  different disks.

So the work is not "invent ESP selection". It is "add ESP *creation*, and make
the chooser prefer APEX's own".

## What this fixes that is already broken

**katana is booting APEX off the Windows disk right now.** `Boot0000* APEX-OS
Primary` points at PARTUUID `2ba9a2ea…`, the 200 MiB ESP Windows created, while
the APEX disk carries its own **unused 512 MiB `EFI-SYSTEM`** at PARTUUID
`99af3362…`. That machine therefore depends on another operating system's disk
to boot, and its NVMe device names reorder across ordinary reboots. Under this
decision katana needs no new partition at all — it needs to start using the one
it already has.

That is the first migration to run, because it is the cheapest proof: no
partitioning, no shrink, and a machine that measurably stops depending on
Windows.

## Where the space comes from, in order of preference

1. **An existing unused ESP on the APEX disk** — katana. Nothing to create.
2. **Free/unallocated space** on the target disk. No data moves.
3. **Shrinking the Windows NTFS volume.** The installer already surveys
   partitions and produces a plan; this becomes a planned, consented step, and
   per the standing constraint the tool tells the user the commands rather than
   silently repartitioning.

Size it for the job — room for two deployments plus slack, not 68 MiB of
someone else's leftovers.

## This does NOT retire the initramfs work

`initramfs-slim` stays necessary and its priority does not drop. Two different
machines, two different problems:

| machine | situation | answer |
|---|---|---|
| L16, and every existing APEX install | 512 MiB ESP that is **already APEX's own** | shrink the initramfs — there is nothing to create |
| Windows dual-boot | Windows' ESP has 68.3 MiB free | build APEX its own ESP |
| katana | has an unused 512 MiB ESP on its own disk | use it; stop booting off the Windows disk |

Andre's *"it has to work in 512. i dont care how it just has to work"* was
about the L16, which has no Windows on it. A new ESP does not help there. A
smaller initramfs helps everywhere, including making case 2 fit on machines
with little free space to give.

## What must be MEASURED before any of this is claimed to work

None of the following is safe to assume, and the first three are the whole risk
of the design:

1. **Two ESPs on one GPT disk.** Does the firmware boot the intended one from
   an explicit NVRAM entry? APEX already writes its own `Boot0000`, so the
   mechanism is there — but "the spec permits it" is not evidence, and firmware
   is where this project has been bitten before.
2. **Does Windows tolerate a second ESP?** Specifically across a feature
   update, a repair install, and `bcdboot`. A design that survives installation
   and dies at the next Patch Tuesday is worse than no design.
3. **Does `bootupd` stay on the mounted ESP** for every later `bootc upgrade`,
   or does it re-discover one? XBOOTLDR was ruled out partly because bootupd
   "mounts the ESP unconditionally" and records paths in a runtime store every
   later upgrade reads. The same store must not point at Windows' ESP.
4. **Windows' own boot path byte-identical** either side, GPT included, proven
   by comparison against a pristine fixture — not by Windows still booting once.

## Bounds that do not change

- The Windows ESP is **read-only, always**. There is no flag that makes it
  writable.
- Never `bootc install` without `--generic-image` outside a real target, and
  always through `tests/lab/bootc-install-lab`. `--generic-image` is the
  prevention; `tests/lab/nvram-guard` is only detection. bootc runs `--pid=host`
  and `nsenter`s into the host mount namespace, so a tmpfs over efivars inside
  the container is inert. That mistake broke the L16 twice in one evening.
- An `efibootmgr -v` diff either side of anything that could touch firmware
  variables.
- The single commit point stays one `SetVariable` of `BootNext`. Creating a
  partition must be complete and verified before the boot entry moves, and a
  failed trial boot must land back where it started.

## Open, and still Andre's

Whether the tool may retype a Windows basic-data partition itself, rather than
printing `set id=` and `gpt attributes=0x0` for the user to run in an elevated
diskpart. Unchanged by this decision.
