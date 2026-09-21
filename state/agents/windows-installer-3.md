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

---

## windows-installer-3b — FRESH continuation, 2026-09-21 (a new agent, prior one gone)

The card above lags its own commits (the "agent cards lag their commits" trap).
Verified against `git`, not assumed:

1. **The "single highest-value next edit" is ALREADY DONE.** The plan.rs remedy
   text now says BOTH `set id={LINUX_FILESYSTEM}` AND
   `gpt attributes=0x0000000000000000`, landed as commit `ccacf128`, and a unit
   test in `plan.rs` asserts the remedy contains `gpt attributes=0x0000000000000000`.
   `cargo test` = 14 passed. Nothing to do here; reporting it as pre-existing.
2. **The payload-write job was WRITTEN but NEVER RUN.** `lab/jobs/payload-write/run.ps1`
   exists (commit `6828591e`) and is thorough, but there is no
   `/var/lab-scratch/winlab/payload-write.log` — it never touched a guest.
   This round's real work: RUN it, one boot, and verify host-side.
3. **`bl-discover` (bitlocker-discover) was interrupted mid-boot last round.**
   `bl-discover.log` is 351 bytes cut off at "boot 1 of at most 2", and the last
   `winlab run` left `guest-normal.txt` = "(no result.txt from boot 1)" (the
   card's claim that survey restored it is FALSE on disk right now). The
   bitlocker-discover job is committed-but-UNPROVEN; the orchestrator must not
   treat it as verified. It is out of scope this round (2 boots, and it is the
   thing that died last time).

### PLAN this round (scope A, confirmed with advisor)
- Rebuild the exe (`winlab build`) — the 10:24 exe predates ccacf128; behaviour
  identical (remedy text only) but rebuild so the claim is clean. Do NOT rebuild
  fixtures: `fixture-a.raw` @ 10:20 is the pristine backing file and the
  host-side comparison baseline.
- Run `payload-write` foreground, one boot, tee to payload-write.log.
- Host-verify from a COPY of run-fixture-a.qcow2 (survey restore rm -f's it):
  qemu-img map (depth==0 = the guest's dirty set), sha256 the 4MiB at p1.Offset
  vs the guest's PAYLOAD-SHA256, cmp p2/p3 first sectors vs pristine.
- Restore guest-normal.txt via a real survey run; verify `grep survey-complete`.
- Run tests/test-windows-installer.sh (no APEX_WINLAB_GUEST) to prove stages 0-3
  green => section 0 write-API gate UNCHANGED.
- Tighten ARCHITECTURE.md Exclusivity with what the guest actually measured.
- Evidence -> ROADMAP/evidence/ (lands with the branch).

### The write-path gate question, answered up front
The .exe still does not write; payload deployment is proven from the PowerShell
side of the lab, exactly as the hard constraint requires. Section 0 of
`tests/test-windows-installer.sh` is UNCHANGED. A real .exe write path is a
future round that must redesign that gate and prove it fails both ways; and note
the card's proposed "default build lacks WriteFile" redesign would still ship a
read-only app, so it does not by itself satisfy "the app does the whole stack" —
that needs a write-through-one-audited-function safety model, a separate round.

---

## ROUND 38 CONTINUATION — orchestrator, 2026-09-21 13:55 AWST

The 3b agent died at the 12:06 shutdown mid host-verification. Read off
disk, not inferred:

| artefact (`/var/lab-scratch/winlab/`) | mtime | meaning |
|---|---|---|
| `apex-windows-installer.exe` | 12:02 | rebuilt after ccacf128, as planned |
| `payload-write.log` (9.4k) | 12:04 | **RAN and PASSED**: `=== JOB COMPLETE ===`, `APEXLAB-RUN-EXIT 0`, `STATUS PASS`, firmware variables `IDENTICAL` |
| `guest-normal.txt` (8.8k) | 12:04 | restored — `survey-complete` present |
| `payload-write-fixture-a.qcow2` (11M) | 12:04 | the copy of `run-fixture-a.qcow2` for host-side verification |
| `overlay-map.json` (8.9k), `overlay-map.err` (0 B) | 12:06 | `qemu-img map` done; **the sha256 / cmp steps and the evidence file were never reached** |
| `fixture-a.raw` / `fixture-b.raw` | 10:20 | pristine baselines, untouched by three guest runs |

Tree clean at `e38e0e42`; nothing committed since `6828591e`.

### What the orchestrator did

Landed `task/windows-installer-3` into `roadmap/v2.2` as **`71bc2177`**,
after `tests/test-windows-installer.sh` on the branch read
`13 passed, 0 failed, 1 could-not-run` (stage 4 needs a guest). The merge
message states that payload-write's HOST-SIDE verification and the
bitlocker-discover job are UNPROVEN. Your evidence file is what changes
that; until it exists nobody may cite payload-write as verified.

### NEXT (supersedes the 3b PLAN)

1. Merge `origin/roadmap/v2.2` (71bc2177 — your own branch, plus luks-boot).
2. Finish host-side verification from `payload-write-fixture-a.qcow2`
   (do not re-copy from `run-fixture-a.qcow2` — a survey run rewrites it):
   `overlay-map.json` depth==0 extents vs the partition offsets from the
   survey; sha256 of the 4 MiB at APEX-TARGET-A's offset vs the
   `PAYLOAD-SHA256` the guest printed in `payload-write.log`; `cmp` of the
   GPT, partition 2 and partition 3 first sectors against pristine
   `fixture-a.raw` (confirm its mtime is still 10:20 first). Record the
   `ERROR_ACCESS_DENIED` results for p2 (live NTFS) and p3 (lettered RAW)
   from the guest log — that is the "Windows itself is the backstop" claim,
   measured for the first time.
3. `ROADMAP/evidence/windows-installer-3-20260921.md` (apex-os repo).
4. Tighten `ARCHITECTURE.md`'s Exclusivity section with what was measured.
5. Commit, push.
6. If time: `bitlocker-discover` (2 boots; it is the job that died last
   time at "boot 1 of at most 2"). `systemd-run --user`, one guest at a
   time, then restore `guest-normal.txt` with a survey run and verify
   `grep survey-complete`.
7. Priorities 5/6 (ESP transaction + undo): design in ARCHITECTURE.md only.

Constraints unchanged: no write path in the .exe (section 0 gate stays as
it is); the two product decisions remain Andre's. Battery was 58% and
DISCHARGING at 13:42 — a Windows guest boot is ~2 min but check
`/sys/class/power_supply/BAT*/status` first; `/var` has 305 G free.

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
### NEXT — rewritten for round 39

**Round 38 landed your branch as merge `71bc2177`.** Four increments are on
`roadmap/v2.2`: the qcow2 fixture overlays, the two-command diskpart remedy
(`set id=` AND `gpt attributes=0x0`), the second-boot lab jobs, and the
`--check-passphrase` work that rode along. `tests/test-windows-installer.sh` on
the branch: 13 passed, 0 failed, 1 could-not-run. Do not redo any of it.

**One of the two product decisions your card reserves for Andre is now
SETTLED** — see the section immediately above, and
`ROADMAP/state/dispatch.json` → `_decisions.esp_ownership_2026_09_21` (landed
as `bc5c3822`, `docs/apex-owns-its-esp.md`). Windows-side installs boot
systemd-boot like every other machine, and **APEX builds its own ESP; Windows'
ESP is read for facts and NEVER written — not "preferably not", never, and no
flag makes it writable.** The 68.3 MiB-free number stops being a constraint to
defeat. The other decision — whether the tool may retype a basic-data partition
itself — is STILL Andre's and is unchanged; do not solve it.

In this order:

1. **Two things from round 38 are UNPROVEN and the merge message says so.**
   The payload-write job PASSED inside the guest at 12:04, but its **host-side
   byte verification was cut off by the shutdown and no evidence file exists**;
   and `bitlocker-discover` has never run to completion. Finish both before
   starting anything new. Host-side verification is the half that matters:
   `qemu-img convert` / `dd skip=` the `run-fixture-a.qcow2` overlay at the
   written offset, sha256 it against the known payload, and confirm the GPT,
   partition 2 and partition 3 extents are **byte-identical** to the pristine
   `fixture-a.raw`.
2. Still unmeasured and still the sharpest claim in `ARCHITECTURE.md`:
   "Windows itself is the backstop" — the expected `ERROR_ACCESS_DENIED` on a
   write into partition 2's live NTFS extent and into the lettered RAW
   partition 3. A backstop nobody has seen refuse is not a backstop.
3. Do NOT add a write path to the Rust binary. `tests/test-windows-installer.sh`
   section 0 is a real write-API allowlist gate, not a formality.
4. **Now that the ESP question is settled, priorities 5/6 (ESP transaction +
   undo) have a target they did not have before.** The decision names four
   things that must be MEASURED, not assumed; two of them are yours because you
   are the only unit with a real Windows guest: **Windows tolerates a second
   ESP across a feature update, a repair install, and `bcdboot`**, and
   **Windows' boot path is byte-identical either side, GPT included, against a
   pristine fixture**. Take them in the lab. `migrate-preconditions` is live in
   parallel and has been told to say on its card which of the four it takes —
   coordinate through the cards, do not duplicate the guest.
5. **katana caution.** The decision calls katana the cheapest first proof of
   the design (an unused 512 MiB `EFI-SYSTEM` at PARTUUID `99af3362`, while
   Boot0000 points at the *Windows* ESP at `2ba9a2ea`). Before any read on
   katana, `ssh katana` and check it is idle — if steam or gamescope is
   running, stop and come back later. **Write nothing to katana this round.**
   A `bootc install` has rewritten host NVRAM twice on this program; keep it in
   the guest.

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
