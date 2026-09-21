# windows-installer-3 — continuation of windows-installer-2

items: none directly (queue id `windows-installer` is closed; this is a direct continuation)
repo: apex-os
worktree: /var/tmp/apex-work/wt-windows-installer-3
branch: task/windows-installer-3, cut fresh from roadmap/v2.2 @ 76aa2b95 (2026-09-21)

windows-installer-2's own branch already LANDED in round 35 (a real Windows
guest, headless install, 3 defects found) — do not redo its work. Read
ROADMAP/state/agents/windows-installer-2.md for the full history, the
Architecture doc reference, and the nine numbered traps already paid for.

## NEXT
1. Priority 4 (payload deployment), synthetic payload only. Write a job
   (`windows-installer/lab/jobs/payload-write/run.ps1`, following the
   diskpart-remedy job's pattern) that opens `\\.\PhysicalDrive<fixture-a's
   number>` from PowerShell (a `[System.IO.FileStream]` with
   `FileOptions.WriteThrough`, sector-aligned offset/length, never touch
   `.Length` — trap 7 in windows-installer-2.md), writes a few MB of known
   SHA256 at the verified offset of the eligible Linux-typed partition
   (APEX-TARGET-A, partition 1), re-enumerates volumes immediately before the
   write per ARCHITECTURE.md's Exclusivity section, and ALSO attempts (and
   expects `ERROR_ACCESS_DENIED`) a write into partition 2's live NTFS extent
   and into the lettered RAW partition 3 — that is the "Windows itself is the
   backstop" claim in ARCHITECTURE.md, currently unmeasured. Verify from the
   HOST side: `qemu-img convert`/`dd skip=` the `run-fixture-a.qcow2` overlay
   at the same offset and sha256 it, and confirm the OTHER extents (GPT,
   partition 2, partition 3) are byte-identical to the pristine
   `fixture-a.raw`. Do NOT add a write path to the Rust binary — see FOUND:
   doing so breaks `tests/test-windows-installer.sh`'s write-API allowlist
   gate (section 0), which is a real gate, not a formality; a write path
   belongs in the .exe only as a follow-on that redesigns that gate.
2. If time remains: priorities 5/6 (ESP transaction + undo), design in
   ARCHITECTURE.md, not started.

Two product decisions stay Andre's, do not solve them: whether Windows-side
installs stay on ostree/GRUB (composefs needs ~1.1GiB ESP, stock Windows ESP
has 68.3MiB free), and whether the tool should ever retype a basic-data
partition itself.

## DONE
- **Item 1 — fixture overlays.** `windows-installer/lab/winlab`'s `cmd_run`
  now creates `run-fixture-a.qcow2` / `run-fixture-b.qcow2` (qcow2-over-raw,
  same treatment as `run.qcow2`/golden.raw) and points qemu at the overlays,
  never at `fixture-a.raw`/`fixture-b.raw` directly. Added a missing
  `need "$DIR/fixture-b.raw" fixtures` check (only `-a` was checked before).
  Commit `f1fc1cdb`, pushed.
  **The fixtures were already mutated before this fix landed**: `fixture-a.raw`'s
  mtime matched the last `windows-installer-2` guest run exactly (Windows
  dirties any NTFS volume it mounts, `$LogFile`/dirty bit, merely by looking at
  it). Rebuilt both fixtures from scratch with `winlab fixtures` (~8s, sparse)
  and rebuilt `apex-windows-installer.exe` with `winlab build` before the first
  guest boot on this branch, so item 2's guest boot measured a clean fixture
  against current source. **Verified empirically after item 2's guest boot**:
  `fixture-a.raw`/`fixture-b.raw` mtimes are still the fixtures-rebuild time,
  unchanged by two subsequent guest runs — the overlay is doing its job.
- **Item 2 — the diskpart `set id=` remedy, tested and answered, one guest
  boot, ~79s.** New job `windows-installer/lab/jobs/diskpart-remedy/run.ps1`.
  **Verdict: the remedy text as it exists today is incomplete for any
  partition that carries the attribute bit real Windows partitions carry —
  confirmed, not suspected.** Full phase sequence (see
  `/var/lab-scratch/winlab/diskpart-remedy.log` for the raw transcript):
  - **Phase 0 (baseline)**: fixture-a's "Blank basic" partition (17 GiB,
    zeroed, Windows basic data) is refused today as `REFUSED (in use by
    Windows)` — Windows auto-mounts it at F:\ as "unrecognised filesystem" the
    moment it boots. This is a DIFFERENT rule than the one the diskpart
    remedy text is attached to (`assess()` checks ownership/claims before
    type), so a user who shrinks C: and leaves the new volume unformatted
    never even sees the "Windows-owned partition type" refusal or its
    diskpart remedy — they see "choose a partition Windows is not using",
    which is a dead end for every basic-data partition, since Windows letters
    all of them. **Worth a docs/UX note, not solved here.**
  - **Phase 1**: running the exact remedy (`diskpart` → `select disk` →
    `select partition` → `set id=0fc63daf-...`) on the fixture AS SHIPPED
    (attributes=0x0) works immediately, no rescan needed: Windows drops the
    volume object the instant the type GUID changes (`windows-claims: none`),
    and `plan::assess` returns `ALLOWED`. This generalizes ARCHITECTURE.md's
    "no volume object for Linux-filesystem partitions" claim from
    creation-time to a LIVE retype, which had never been measured before.
  - **Phase 2**: reverted the partition to basic-data and forced the exact
    attribute bit measured on golden.raw's own ESP/MSR/C: partitions
    (`Set-Partition -NoDefaultDriveLetter $true` → reads back as
    `attributes 0x8000000000000000`, confirming that bit IS
    `GPT_BASIC_DATA_ATTRIBUTE_NO_DEFAULT_DRIVE_LETTER`). The partition was
    STILL claimed (`mounted at F:\`) — the attribute suppresses future
    auto-assignment, it does not retroactively strip an existing letter.
  - **Phase 3 — the trap, confirmed**: retyped to Linux filesystem AGAIN with
    the attribute still forced on. The ownership claim clears exactly as in
    phase 1 (`windows-claims: none`) — but now `plan::assess` returns
    `REFUSED (partition attributes set)`. **`diskpart set id=` changes only
    the type-GUID field; it does not touch the GPT attributes field, so a
    real Windows-made basic-data partition (which the golden image proves
    carries this bit on every partition Windows itself created) walks
    straight from one refusal into a second one, exactly as
    windows-installer-2.md predicted.**
  - **Phase 4**: a second diskpart command in the same elevated session,
    `gpt attributes=0x0000000000000000` (also run against the same selected
    partition), clears the bit; final survey: `attrs=0x0 … claims=[none] …
    verdict=ALLOWED`. So a complete remedy needs TWO diskpart commands, not
    one — `set id=` alone is not sufficient, but a real fix exists and is
    still entirely the user's own act in an elevated diskpart, so it does not
    trip Andre's "must not retype for the user" constraint.
  - Sanity-checked in the same boot: partition 1 (APEX-TARGET-A), partition 2
    (Windows data / NTFS), and all of fixture-b were unaffected by the whole
    sequence. Firmware variables identical before/after (no boot entry
    touched, as expected — this test never went near golden.raw).
  - **Consequence for `plan.rs`'s remedy text**: it should say
    `set id=<guid>` **and** `gpt attributes=0x0000000000000000`, not just the
    first. That is a one-line code change (the `format!` in the
    `WINDOWS_BASIC_DATA` arm of `assess()`) but I did not make it — ran out of
    round time after item 2's write-up and wanted item 3 to get a full pass
    rather than leaving both half-done. **This is the single highest-value
    next edit**, now that it is proven correct rather than assumed.
  - `guest-normal.txt` was transiently overwritten by this job's own run
    (same file name, `cmd_run` always writes the non-swap result there) —
    re-ran `windows-installer/lab/jobs/survey` once afterward (PASS, ~106s) to
    restore it to the state the existing evidence/suite expects.

## FOUND
- `tests/test-windows-installer.sh` section 0 is a real, working gate: it
  denylists `WriteFile`/`GENERIC_WRITE`/`FILE_WRITE_DATA`/etc. by name across
  `windows-installer/src/` AND allowlists every declared IOCTL/FSCTL hex
  constant. Adding a disk-write path to the Rust binary breaks this
  immediately and on purpose. Priority 3 (payload deployment) has to be
  proven from the PowerShell side of the lab, not by adding writes to the
  .exe — a write path in the binary is real future work that has to
  redesign this gate at the same time (e.g. assert the *default build*'s PE
  import table lacks `WriteFile`, so a future `--enable-write` build could
  still exist and be checked separately), not weaken it to get one test job
  running.
- One guest boot answers several sequential diskpart questions fine as long
  as each phase re-surveys with the compiled `.exe` rather than trusting
  PowerShell's own `Get-Partition` properties — `plan::assess` is what has to
  be satisfied, and it reads raw GPT bytes via IOCTL that PowerShell's
  boolean convenience properties (`NoDefaultDriveLetter`, `IsHidden`, …)
  don't expose directly.
- Machine had another agent (`wt-sdboot-xbootldr`, a `bootc install
  to-disk --via-loopback` run) going at the same time both guest boots in
  this round ran. `free -h` showed 17-19 GiB available throughout (only ~10-12
  GiB used out of 29 GiB) — no contention in practice, but noting it since the
  instructions ask for it. Ran one guest at a time, as required regardless.

## BLOCKED ON
- nothing
