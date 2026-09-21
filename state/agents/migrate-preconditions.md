# migrate-preconditions — decide, before anything is written, whether this machine may migrate

## LANDABLE — `f3280072` on `task/migrate-preconditions`, pushed

`roadmap/v2.2` (`b137f03f`) is merged in; `tests/test-boot-migrate.sh` is
**83 passed, 0 failed**, and the new gate was mutation-tested red both ways.
It makes `precheck` enforce `docs/apex-owns-its-esp.md` — a machine whose
chosen ESP is Windows' is now refused instead of written — and stops the tool
claiming an ESP override that does not exist. Nothing here creates a partition
or writes a boot variable.

**What landing it changes for a live machine:** one new refusal,
`esp-is-windows`, on machines whose root-disk first ESP carries
`bootmgfw.efi`. Checked, not assumed, on both machines this program owns: the
L16 has no Windows on any disk, and katana's root-disk ESP is its own
`EFI-SYSTEM` (Windows' is on a disk bootc never searches). Neither is
affected. The behaviour it removes is writing Windows' ESP, which the decision
forbids outright.

---

## ROUND 39 STATUS — read this first; it supersedes every section below it

### THE JUDGEMENT ON THE 342 LINES: they survive. Do not revert them.

The round-39 brief said to revert on the grounds that "`bootc install
to-filesystem` writes to the ESP the caller mounts, so there was never a bootc
choice to mirror". **That is about the wrong subcommand.** `cmd_stage` runs
`bootc install to-existing-root`, and `to-existing-root --help` lists no ESP
option and no `--boot-mount-spec` at all — only `[ROOT_PATH]`. On the migration
path the caller CANNOT point bootc at an ESP. So the 342 lines describe a
**constraint of the subcommand this tool uses**, not an ESP-selection policy
APEX invented, and the ownership decision does not contradict them.

Reframed, not reverted: from *"mirror bootc's choice"* to **"know what bootc
WILL write, and refuse when that turns out to be Windows' ESP."** The decision
is about ownership; the code is about what the subcommand does; they are
compatible once precheck enforces the bound.

### AND THE DECISION DOC'S MECHANISM CLAIM IS FALSE FOR THIS PATH — read source, at `bootc 1.16.10`

`docs/apex-owns-its-esp.md` says "**`bootc install to-filesystem` uses the ESP
the CALLER mounted** … bootc does not go hunting; it writes where it is
pointed." Checked against the source, not assumed —
`/var/lab-scratch/sdboot-xbootldr/bootc-src` is `3e76c16`/`v1.16.10`, which is
**exactly the `bootc --version` on this machine**, and `bootc-main` is
`v1.16.11`:

`crates/lib/src/bootc_composefs/boot.rs`, **four** call sites — 691, 729,
1256, 1273 in 1.16.10; 708, 746, 1285, 1302 in 1.16.11 — every one of them:

```
// Locate ESP partition device by walking up to the root disk(s)
let esp_part = root_setup.device_info.find_first_colocated_esp()?;
...
let esp_mount = mount_esp_writable(&esp_device)
```

`boot_mount_spec()` appears in that function exactly once and only to build a
`systemd.mount-extra=…:/boot` karg. **It does not steer the loader write.** On
the composefs + systemd-boot path — the only path APEX uses — the ESP is always
`find_first_colocated_esp()`: `find_colocated_esps()?.remove(0)`, i.e. the
FIRST ESP-typed partition in partition order on the first disk backing the
root. Not the biggest, not the booted one, not the mounted one.

Two consequences, both bigger than this unit:

1. **Mounting APEX's ESP does not make bootc write it.** A design that mounts a
   chosen ESP and expects the UKI to land there is not what 1.16.10 or 1.16.11
   does. → `windows-installer-3`, see BLOCKED/COORDINATION below.
2. **The `Upgrade` arm re-discovers too.** Both `BootSetupType::Upgrade` arms
   call `find_first_colocated_esp()` afresh. So this is not an install-time
   choice that can be pinned once — **every later `bootc upgrade` re-walks the
   GPT**, and if Windows' ESP is first in partition order it is written forever,
   not once. That is the decision doc's **must-measure #3 answered from source**:
   nothing persists a pointer; there is nothing to pin.

### DONE this round

- Merged `origin/roadmap/v2.2` (`b137f03f`) — clean, no conflict on the tool.
- `dc2590ef` `test(boot): let precheck describe a machine it is not running on`
  — the orphan dirty hunk, attributed. It is `APEX_MIGRATE_FAKEROOT` /
  `APEX_MIGRATE_MOUNTDIR`: test plumbing for the both-ways gate, orthogonal to
  the ESP decision, empty-by-default so production paths are unchanged. Checked
  it is complete: every bare path test in `cmd_precheck` is now prefixed.
- `011eed35` `feat(boot): refuse to migrate onto Windows' ESP, and stop
  claiming bootc can be steered` — the reframe, plus the new **`esp-is-windows`
  REFUSAL**: if the ESP bootc would write carries
  `EFI/Microsoft/Boot/bootmgfw.efi`, stop. Refusal and not note because it is
  CLEARABLE (an ESP of APEX's own, which APEX creates), which is the exact test
  that made BitLocker a note. Also fixed the `esp-choice` verdict, which tagged
  `APEX_MIGRATE_ESP` as "(overridden)" and so claimed a steering that does not
  exist. `bitlocker-shared-esp` kept but now only reachable under `--explain`,
  reworded to describe the fix rather than a hazard that can no longer happen.
  **77 passed, 0 failed** (`tests/test-boot-migrate.sh`).
- `b45d641b` `test(boot): prove the Windows-ESP refusal by running it, not by
  grepping for it` — six BEHAVIOURAL assertions, not greps: they run the engine
  against two fixture ESPs differing by one file. **Mutation-tested both ways**
  — `if windows_loader_on_esp` → `if false` gives "an ESP carrying Windows'
  loader was allowed"; → `if true` gives "it is unconditional, so it proves
  nothing"; unmutated 83/0. A grep for the refusal's name would have passed the
  second mutation. `precheck --explain` is what makes this runnable without
  root, loopback or UEFI on a CI runner.
- `918f360f` + `7b509f19` — `ROADMAP/evidence/migrate-preconditions-20260921.md`.
- `f3280072` `fix(boot): correct the ESP doc's mechanism claim, and stop the
  refusal promising a remedy that does not exist` — **a dated CORRECTION block
  now sits in `docs/apex-owns-its-esp.md`** under "The mechanism already mostly
  exists", because two of its bullets were labelled "checked rather than
  assumed" and were checked only against `--help`. The decision is untouched;
  the claim that the mechanism is nearly free is not. And the refusal no longer
  says the partition "is APEX's to create" without adding that APEX does not
  create it yet — a refusal that sends a user hunting for a command nobody
  wrote is the failure mode this project was told to stop shipping.

### Robustness checks the suite now has behind it

- **83/0 with `bootc`, `podman`, `mkfs.vfat` and `rsync` genuinely absent**
  (a 2,774-entry symlink PATH minus those four) — the CI runner's shape. The
  engine records them as refusals instead of crashing, and `esp-is-windows`
  still fires: 14 verdicts rather than 12.
- **Five consecutive runs, no flake** (the `awk | grep -q` remedy assertion was
  the pipefail-141 suspect).
- `tests/check-suites-run-in-ci.sh`: 102 suites, 95 run by CI, 7 exempt, **0
  unrun and undeclared** — this suite is not on `suites-not-in-ci.txt`, so the
  six behavioural assertions really do execute in CI.

### MEASURED ON THE REAL L16 — read-only, and it proved the leak fix too

`sudo apex-boot-migrate precheck --explain`, ESP **not** previously mounted, so
this exercised the `ro` mount and the new `trap release_esp EXIT` against the
production ESP. Afterwards `findmnt -S /dev/nvme0n1p1` is empty and
`/run/apex-boot-migrate/esp` is empty. **No leak, no dirty bit.**

The table showed **two** refusals: `secure-boot-unsigned-loader` AND
`esp-too-small` (590 MiB free, 1170 needed, short by 580). **Plain `precheck`
shows only the first** — it exits at Secure Boot having never mounted the ESP.
That is the case for `--explain` in one screen, on a real machine.

### THE NUMBER `initramfs-slim` IS AIMING AT — and it is tighter than anyone said

The refusal now computes the budget from the live partition at run time, not a
constant: **the L16 needs a kernel + initramfs of 180 MiB or less**, and
**katana needs 154 MiB or less** (512 MiB ESP, so `(511-48)/3`). varB (~100 MB
initramfs) + this vmlinuz is ~117 MiB and clears both — their number to
confirm, not mine.

**Correction to `docs/apex-owns-its-esp.md`, worth the orchestrator's
attention:** it lists katana as needing "no new partition at all", with no
shrink and no partitioning, as "the cheapest first proof". True, and not
sufficient — the partition katana already has is **512 MiB**, so it fails the
same fit check the L16 does, by more than 2x. **The katana proof and "it has to
work in 512" are one requirement, and both wait on `initramfs-slim`.**

### MUST-MEASURE CHECKLIST (from `docs/apex-owns-its-esp.md`) — who takes what

| # | item | owner |
|---|---|---|
| 1 | firmware boots the intended one of two ESPs from an explicit NVRAM entry | **`windows-installer-3`** (needs the guest lab) — not me |
| 2 | Windows tolerates a second ESP across feature update / repair / `bcdboot` | **`windows-installer-3`** — not me |
| 3 | does `bootupd`/bootc stay on the mounted ESP for later upgrades | **ME — ANSWERED ABOVE, from source.** No: it re-discovers every upgrade |
| 4 | Windows' boot path byte-identical, GPT included, vs a pristine fixture | **`windows-installer-3`** — not me |
| 5 | whether BitLocker's PCR profile binds PCR 5 | `windows-installer-3`'s `bitlocker-discover` job |

**I take none of the four lab measurements.** I contribute the source read that
answers #3 and that #1 depends on.

### NEXT

1. Land `7b509f19`. Then the next unit of work — and it is NOT this branch's —
   is ESP **creation**, which is blocked as described under BLOCKED ON: with
   `find_first_colocated_esp()` unoverridable, "prefer APEX's own ESP" means
   being FIRST in the root disk's partition order, which is a GPT change and
   therefore must-measure #1 (and #5 where Windows shares the disk). Whoever
   takes it should start from `ROADMAP/evidence/migrate-preconditions-20260921.md`
   §1, not from `docs/apex-owns-its-esp.md`'s "the caller mounts it" claim.

### FOUND (round 39 — defects in shipped/landed material, not mine)

- **`APEX_MIGRATE_ESP` does not do what `docs/apex-owns-its-esp.md` says it
  does.** The doc calls it "the override needed to target a chosen partition".
  It is not: it only changes where `with_esp` **measures**. `cmd_stage` still
  calls `find_esp()` for the FAT volume id, and bootc still self-discovers, so
  the override cannot move a single byte of the write. The `esp-choice`
  verdict's `(overridden)` tag implies steering that does not happen — fixing
  that wording is part of the next commit.
- **`find_first_colocated_esp()` is not overridable by any flag on either
  subcommand.** There is no supported way for APEX to hand bootc an ESP on the
  composefs path today. ESP *preference* therefore has to be expressed as GPT
  partition ORDER, not as an argument.

### BLOCKED ON / COORDINATION

- **ESP creation and preference on the migration path is blocked**, and not on
  effort: with `find_first_colocated_esp()` taking the first ESP in partition
  order and no flag to override it, "prefer APEX's own ESP" on an existing
  machine means making it the first ESP-typed partition on the root's disk —
  a GPT change, which is must-measure #1 and (where Windows shares the disk)
  #5. **I did not attempt creation this round**, deliberately.
- **`windows-installer-3` should read the source note above before building on
  "mount the ESP you want".** It is the one finding here that could invalidate
  a design outside this unit. Coordinating through this card, not by entering
  its lab.
- `initramfs-slim`: unchanged, still the lever for the L16's 600 MiB ESP. I
  consume its per-deployment number; the fit check reads sizes off the
  deployment rather than hardcoding 374.3 MiB.

### Still Andre's, untouched

Whether the tool may retype a basic-data partition itself — **settled**
(`b137f03f`, the tool does it). Firmware writes via
`SetFirmwareEnvironmentVariable` — **still open**, not touched here.

---

items: none (dispatched as the precondition half of the GRUB → systemd-boot pivot)
repo: apex-os
worktree: /var/tmp/apex-work/wt-migrate-preconditions
branch: task/migrate-preconditions, cut from roadmap/v2.2 @ 4031d43f
lab: /var/lab-scratch/migrate-preconditions

## THE UNIT

Everything that runs BEFORE `apex-boot-migrate stage` writes a byte: is this
machine safe to migrate, which ESP would be written, and — when the answer is
no — what the user can actually do about it. The refusal path is the product.

Andre, verbatim: "active machines should automitcally migrate with sudo apex
install, not this dumbass reinstall shit" / "You have to figure out a way to
make it work for all machiens" / "it has to work in 512. i dont care how it
just has to work." / "also it should work on the katana."

## MEASURED FIRST — the brief's description of the L16 is STALE

Do not plan against the brief's "512M ESP, BitLocker p3, btrfs p4 /sysroot".
Measured on the live L16 2026-09-21 (`lsblk`, `blkid`, `bootc status`,
`efibootmgr -v`, all read-only):

| part | size | type | contents |
| --- | --- | --- | --- |
| nvme0n1p1 | 600M | EFI System | vfat, PARTUUID 1c417de2-5766-455f-9318-198610885424 |
| nvme0n1p2 | 2G | Linux extended boot (XBOOTLDR) | ext4 |
| nvme0n1p3 | 396.3G | Linux filesystem | btrfs `fl_fedora` |
| nvme0n1p4 | 2G | Linux extended boot (XBOOTLDR) | ext4 `apex-newboot` |
| nvme0n1p5 | 1.5T | Linux filesystem | btrfs `apex-root` -> /sysroot |

* ESP is **600 MiB, not 512**.
* **No BitLocker and no NTFS on this machine at all** — blkid shows only
  vfat/ext4/btrfs. The L16 is NOT the BitLocker test case; katana is.
* /sysroot is **p5**, not p4. There are **two** XBOOTLDR partitions.
* `bootc status`: store `ostreeContainer`, nothing staged.
* `BootCurrent: 0000` -> `HD(1,GPT,1c417de2...)/\EFI\fedora\shimx64.efi`, so the
  booted ESP IS on the root's disk here. The katana split does not reproduce.

Kernel cost, measured on 7.2.3-cachyos2.fc43.x86_64:
vmlinuz 16 898 120 B + initramfs.img 375 558 646 B = **374.3 MiB per
deployment**; `need = per*3 + 48 MiB` = **1170.8 MiB**. The L16's own 600 MiB
ESP fails by ~2x. `initramfs-slim` is the lever (WIP commit 847b5bfb, touches
Containerfile.{apex,core,kernel} ONLY — no overlap with this unit's file; it
has no agent card yet).

## NEXT (fill in as you go)

1. Mirror bootc's OWN ESP choice, do not invent one. Check whether cmd_stage
   passes an ESP to bootc or only lets it discover one.
2. BitLocker: verdict is `proceed-with-note`, NOT a refusal. See DECIDED.
3. precheck must be read-only + leak-free before it is run on the real L16.
4. `precheck --explain`: evaluate every check, print all verdicts. That is the
   decision table.
5. Both-ways gate as PATH shims in tests/. Loopback is evidence, not the gate.

## DECIDED (with reasons, so nobody re-opens them)

- **BitLocker is a NOTE, not a refusal.** Suspending BitLocker leaves the
  `-FVE-FS-` signature in place — suspension stores the key in the clear and
  the volume stays FVE-formatted. So a refusal keyed on that signature can
  never be cleared by the instruction we would give the user; only full
  decryption clears it. That makes it a PERMANENT refusal on every BitLocker
  dual-boot machine, which loses the same argument sdboot-migrate already
  settled for katana ("a refusal would keep katana on the Windows disk
  forever") and contradicts "all machiens". The migration itself is additive
  on the ESP, touches nothing under \EFI\Microsoft and never edits Windows'
  Boot####, so it does not move Windows' PCRs. The real hazard is
  POST-migration: systemd-boot auto-detects bootmgfw.efi on a shared ESP and
  chainloading Windows through sd-boot is what trips PCR 4/7.
- **The gate is PATH shims, not loopback.** The CI runner is a second
  environment with no guaranteed /dev/loop-control or CAP_SYS_ADMIN, and a
  loop image can never reproduce the katana case at all, because
  `booted_esp_partuuid` reads the host's real `efibootmgr -v`.

## FOUND (defects in the shipped path, not mine)

- `cmd_precheck` mounts the host ESP **read-write** via `with_esp`, while its
  own documented contract says "(writes nothing)". Mounting FAT rw sets the
  dirty bit.
- The ESP mount **leaks**: `refuse "no-kernel"` (line ~378) fires after
  `with_esp` with no `release_esp`, and `refuse` is `exit 10`. There is no
  EXIT trap. Already true on the shipped path.

## IN PROGRESS

**STALE — do not read this line as status.** It said "nothing committed yet"
while two commits already existed, and it misled round 38 into re-deriving the
branch state by hand. Current status is the `## LANDABLE` block and the
`ROUND 39 STATUS` section at the TOP of this file.

---

## ROUND 38 CONTINUATION — written by the orchestrator, 2026-09-21 13:55 AWST

The round-37 agent died at the 12:06 shutdown. **The card above lags its
commits** ("nothing committed yet" is false). Verified in the worktree:

- `89ff6803 fix(boot): precheck must not mount the host ESP writable, or leak it`
- `754b6174 feat(boot): the precondition decision — BitLocker, which ESP, and every check at once`
  (+342/-28 on `files/system/libexec/apex-boot-migrate` between them)
- ONE dirty file: `files/system/libexec/apex-boot-migrate` (+22/-9),
  uncommitted, also in refs/wip. Read `git diff` before anything else — it
  is the last thing the dead agent was typing.

The orchestrator pushed both commits: `origin/task/migrate-preconditions`
now exists at `754b6174` with upstream set. `origin/roadmap/v2.2` is
`71bc2177` (luks-boot + windows-installer-3; `installer/`,
`windows-installer/`, `tests/suites-not-in-ci.txt` — no overlap). Merge it.

### NEXT (items 1-5 above stand; sharpened)

0. Read the dirty diff; finish or discard it deliberately; commit.
1. Mirror bootc's OWN ESP choice — check whether `cmd_stage` passes an ESP to
   bootc or only lets it discover one.
2. BitLocker = `proceed-with-note` (DECIDED above; do not reopen).
3. precheck read-only + leak-free BEFORE it runs on the real L16. You ARE on
   the L16; its ESP (nvme0n1p1, 600 MiB) is production. Use PATH shims and
   loopback for development, the real machine only for the final read-only
   `--explain`.
4. `precheck --explain`: every check evaluated, every verdict printed.
5. Both-ways gate as PATH shims in `tests/`.

### The 512 MiB number is moving under you

`initramfs-slim` is producing the slim initramfs this round (varB ≈ 100 MB
vs 375 MB today). Do not hardcode 374.3 MiB per deployment in the fit
check; read the sizes off the deployment being staged, and cite their
evidence file for the projected figure. Write the refusal text so it
prints the measured need and the measured free space, not a constant.

Battery was 58% and DISCHARGING at 13:42 — check
`/sys/class/power_supply/BAT*/status` before any loopback run over ten
minutes.

---

## PRODUCT DECISION MADE — 2026-09-21, by Andre. Merge the tip and read it.

`docs/apex-owns-its-esp.md`, landed on `roadmap/v2.2` as `bc5c3822`.

Andre, verbatim: *"windows side install should be like everything else with the
systemd-boot. maybe it should build a new esp for apex or something."*

This answers the FIRST of the two questions this card says are "Andre's, do not
solve them". It is now settled:

1. **No ostree/GRUB variant for Windows machines.** Every APEX machine boots
   systemd-boot from a UKI through the same `bootc install --composefs-backend
   --bootloader systemd` path.
2. **APEX builds its own ESP. Windows' ESP is read for facts and NEVER
   written.** Not "preferably not" — never. No flag makes it writable.

The 68.3 MiB-free measurement stops being a constraint to defeat and becomes
the reason not to borrow the partition at all: a Windows feature update that
grows `\EFI\Microsoft` would reclaim any slack squeezed into it, and the next
`apex update` would fail on a machine that worked the day before.

Already true on the tip, checked rather than assumed — do not re-derive:

- `bootc install to-filesystem` writes to the ESP **the caller mounts**; its
  own help says partitions "are prepared and mounted by an external tool or
  script".
- `apex-boot-migrate` already accepts `APEX_MIGRATE_ESP`.
- It already distinguishes `find_esp()` (the root disk's ESP) from
  `booted_esp_partuuid()` (the one firmware loaded from).

The work is ESP **creation** and preferring APEX's own, not inventing ESP
selection.

**The second product decision — whether the tool may retype a basic-data
partition itself — is STILL Andre's and is unchanged.**

`initramfs-slim` is not retired: the 512 MiB ceiling still binds wherever the
ESP is already APEX's own, which is the L16 and every existing install.

---

## ROUND 39 CONTINUATION — written by the orchestrator, 2026-09-21 17:20 AWST

The round-38 agent was killed by a **session usage limit at 14:59 AWST**
(`ROADMAP/state/autoresume.log`: `resume session ended (exit 1)`). It was never
messaged. You are a FRESH agent and this card is your whole inheritance —
everything above stands unless this section contradicts it, and where it
contradicts it, this section wins.

**Hard deadline: this orchestrator runs under `timeout 4h` and dies at about
21:08 AWST.** Commit and push small and often. Update this card after every
commit and whenever NEXT changes — a card that is only correct at the end is
worth nothing, which is the entire reason this directory exists.

**Write `## LANDABLE` at the top of this card, with the sha, the moment your
branch is ready to merge onto `roadmap/v2.2`.** The orchestrator lands on that
signal and will not guess. If it is NOT landable, say why in one line —
"landing this would break X" is a finding, not a failure.
### NEXT — REWRITTEN for round 39. The old NEXT #1 is SUPERSEDED.

The old NEXT began *"Mirror bootc's OWN ESP choice, do not invent one."*
**That is no longer the brief.** Andre settled the question at 17:09 today
(the section immediately above this one, and
`ROADMAP/state/dispatch.json` → `_decisions.esp_ownership_2026_09_21`; landed
as `bc5c3822`). `bootc install to-filesystem` writes to the ESP **the caller
mounts** — so there is no bootc choice to mirror. APEX is the caller, and APEX
picks, and where there is no ESP of its own, APEX **creates** one. The work is
ESP *creation* and *preferring APEX's own*, not ESP *selection*, which this
tool already does (`find_esp()` vs `booted_esp_partuuid()`, and
`APEX_MIGRATE_ESP` already exists — do not reinvent any of the three).

In this order:

1. **Your one dirty file is unattributed.** ` M files/system/libexec/apex-boot-migrate`
   in `/var/tmp/apex-work/wt-migrate-preconditions`. `git diff` it and decide
   whether it belongs to the pre-decision design. Two commits (`89ff6803`,
   `754b6174`) are on `origin/task/migrate-preconditions` and are NOT landed —
   the orchestrator deliberately held them this round because 342 of their 370
   changed lines are "which ESP" logic written before the decision existed.
   **Your first real judgement is which of those 342 lines survive it.** Say so
   explicitly on this card; a revert with a reason is a good outcome here.
2. Re-read the refusal path against the decision. "The refusal path is the
   product" is still true, but the set of things worth refusing changed: a
   machine with no room in *Windows'* ESP is no longer a refusal, it is a
   machine that gets its own partition. A machine with no free space at all
   still is.
3. The decision names four things that **must be measured, not assumed** —
   copy them into this card as a checklist and mark them off:
   - firmware boots the intended one of two ESPs on a disk, from an explicit
     NVRAM entry;
   - Windows tolerates a second ESP across a feature update, a repair install,
     and `bcdboot`;
   - `bootupd` stays on the MOUNTED ESP for later `bootc` upgrades rather than
     re-discovering one — its runtime store must never point at Windows' ESP;
   - Windows' boot path byte-identical either side, GPT included, against a
     pristine fixture.
   Items 2-4 overlap `windows-installer-3`, which is live in parallel and has
   the Windows guest lab. Coordinate through the cards, not by duplicating the
   lab: say on this card which of the four you are taking.
4. `initramfs-slim` is live in parallel and is NOT retired by the decision —
   the 512 MiB ceiling still binds on the L16 and every existing install, which
   is exactly the machine your own measurements are about (600 MiB ESP,
   `need = 1170.8 MiB`, fails by ~2x). You consume its per-deployment number;
   do not re-derive it.
5. **katana caution.** The decision calls katana "the cheapest first proof"
   (its own disk carries an UNUSED 512 MiB `EFI-SYSTEM` at PARTUUID
   `99af3362` while Boot0000 points at the *Windows* ESP at `2ba9a2ea`).
   Before ANY read on katana, check it is idle — `ssh katana` and look for
   steam/gamescope; if a game is running, stop and come back later. **Write
   nothing to katana this round**, and remember its NVMe names reorder across
   reboots, so address partitions by PARTUUID and never by `nvme0n1`.
6. Still Andre's, unchanged, do not solve: whether the tool may retype a
   Windows basic-data partition itself instead of printing `set id=` and
   `gpt attributes=0x0` for the user.

### The contract (ROADMAP/state/README.md, short form)

Keep this card's `NEXT` / `DONE` / `IN PROGRESS` / `FOUND` / `BLOCKED ON`
sections current **as you go, never at the end**. `NEXT` is load-bearing: one
line, the exact next action, specific enough that a stranger could do it.
Everything else can be re-derived from git; the next action cannot.

### Constraints (non-negotiable)

- Never push `main`, never open a PR — final integration only.
- **Headless only.** Never open a window on Andre's desktop. Never run `qs -p`.
- No polkit and no keyring prompts (`sudo` / `--user`, never an agent helper).
- Never `pkill apex-agentd`.
- Do not interrupt gaming on katana.
- Scratch goes in `/var/lab-scratch/<your-slug>/`, NOT `/tmp` (tmpfs, 15 GB on
  29 GB RAM — a stdout-only Bash failure there is memory, not disk). The
  scratchpad is shared between agents: use your own subdirectory.
- Long builds run in the FOREGROUND or under `systemd-run --user`; a
  backgrounded `podman` gets SIGTERMed and still exits 0.

## SECOND PRODUCT DECISION ALSO MADE — 2026-09-21. Both are now settled.

Landed as `b137f03f`. Full text in `docs/apex-owns-its-esp.md`; read it, do not
work from this summary.

Andre delegated this one ("you decide"), so the reasoning is written out in the
doc rather than asserted.

**The tool changes a partition's type GUID and attributes ITSELF.** It does not
print `set id=` and `gpt attributes=0x0` for the user to retype into diskpart.
Handing a user raw diskpart is the MORE dangerous option: diskpart has no undo
and makes the *user* do the targeting (`select disk N`), while the tool has
already read the raw GPT and knows exactly which entry. Transcription is where
the accident lives.

**THE MECHANISM IS DELIBERATELY NOT DECIDED.** Neither candidate is measured.
A raw sector write to LBA 2–33 is narrow in blast radius, but it is UNVERIFIED
that Windows permits one on a LIVE SYSTEM DISK at all — the payload-write proof
was to a partition extent, a different protection regime — and Windows' cached
partition view is stale afterwards until `IOCTL_DISK_UPDATE_PROPERTIES`
(0x70140), which is not on the allowlist either. `GET_DRIVE_LAYOUT_EX` → change
one entry → `SET_DRIVE_LAYOUT_EX` is wide in API but narrow in intent, and
Windows maintains its own state and the backup GPT. **Which is safer is one
guest boot to find out — that measurement is yours to make.** The doc lists six
invariants the result must satisfy either way.

**PCR 5 — a precondition for the ESP decision too, not just this one.** TCG
assigns PCR 5 to the GPT partition table. Where BitLocker's profile binds it,
ANY GPT change forces a recovery prompt on the next Windows boot, including
creating APEX's own ESP. It is read, never assumed: `manage-bde -protectors
-get C:`. **The `bitlocker-discover` job runs `-status` and `-protectors
-disable` and does NOT read the profile — adding that is the first thing it
needs.** This repo mentions PCR 0, 7 and 11 about 240 times and PCR 5 zero
times, so there is no prior work to lean on here.

**The section 0 write-API gate sharpens, it does not weaken.** It must still
fail both ways afterwards. One binary or a default-plus-write-build is the
implementation's call. Deleting an assertion to get a job green is not.

`ARCHITECTURE.md`'s "Into the shared Windows ESP" section is now marked
SUPERSEDED — it describes the thing `bc5c3822` forbids.

**Still open and still Andre's:** firmware writes
(`SetFirmwareEnvironmentVariable`). ARCHITECTURE.md's "Into the firmware"
section plans it; the denylist forbids it today. It is its own decision and was
deliberately NOT folded into this one.
