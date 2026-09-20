# windows-installer-2

Branch `task/windows-installer-2`, worktree `/var/tmp/apex-work/wt-winst2`,
based on `roadmap/v2.2` @ `602a8376`. Pushed after every commit.

Large artefacts live in `/var/lab-scratch/winlab/` (never `/tmp` — it is RAM
here). None of them are repository files; `windows-installer/lab/winlab` builds
them all from scratch.

## STATE — 2026-09-21 00:10 AWST

**A real Windows guest exists and installs itself headlessly in 2.5 minutes.**

- `/var/lab-scratch/winlab/ws2022-eval.iso` — Windows Server 2022 Evaluation,
  5044094976 bytes, from `https://go.microsoft.com/fwlink/p/?LinkID=2195280`.
- `apex-winsetup.iso` — remastered: `autounattend.xml` + `apexlab-agent.ps1` in
  the root, EFI El Torito image swapped for `efisys_noprompt.bin`,
  `install.wim` split into `install.swm`/`install2.swm`.
- `golden.raw` (40 G sparse) + `golden-VARS.fd` — Windows Server 2022 Standard
  Core on GPT: 100 MB ESP, 16 MB MSR, 40 GB NTFS. The varstore holds
  `Boot0005 "Windows Boot Manager"` and `BootOrder: 0005,0003,0000,0001,0004`.
  That varstore is the before-baseline for every "Windows entry untouched"
  claim.
- `fixture-a.raw` (AHCI, `APEX-FIXTURE-A` / `FIXA00000001`): 17 GiB Linux-type
  zeroed; 1 GiB NTFS; 17 GiB basic-data zeroed.
  `fixture-b.raw` (NVMe, `FIXB00000002`): 17 GiB Linux-type zeroed.

Architecture decided and written up in `windows-installer/ARCHITECTURE.md`:
**stage a payload, complete on first boot**, targeting the **ostree + GRUB**
backend because the composefs path needs ~1.1 GiB of ESP and a stock Windows
ESP is 100 MB.

Code: `src/windows.rs` (hand-declared kernel32 FFI, structures parsed by byte
offset), `src/plan.rs` (platform-neutral eligibility rules + the confirmation
text, 14 unit tests that run on Linux), `src/main.rs` gains `survey` and
`inspect`. Cross-builds to a PE32+ binary.

## Traps already paid for — do not rediscover these

1. `xorriso -indev` on the Microsoft ISO sees **one** file, `README.TXT`.
   Everything real is in the UDF volume and libisofs reads no UDF. Extract
   with `7z`, author with `xorriso -as mkisofs`.
2. `install.wim` is 4.34 GB — over the ISO-9660 file limit, and Windows' CDFS
   reads no multi-extent files. `wimsplit` it.
3. The install CD is detached after Setup's first phase, so the lab agent
   cannot be copied from it at first logon. It rides in on the APEXLAB
   transport volume, found by label.
4. PowerShell in this guest is **5.1**. No ternary, no `??`.
5. `podman -v dir:Z` gives the directory the calling container's private MCS
   category pair, so starting a second container against the same work
   directory takes write access away from the first — seen as qemu failing to
   unlock its own qcow2. Use `:z`.
6. An apostrophe inside a single-quoted container script (`Setup's`) silently
   terminates the string and turns the rest of the function body into code
   that the outer shell runs. `bash -n` does **not** catch it: the result is
   still valid syntax.
7. `File::metadata().len()` is 0 on a `\\.\PhysicalDriveN` handle.

## NEXT

1. **Fixture disks are attached raw, with no overlay.** The system disk gets a
   qcow2 overlay per run; the fixtures do not. The first job that writes
   anything — priority 4, or a `diskpart` test — permanently mutates
   `fixture-a.raw` and every later run silently measures a different disk.
   Give them overlays in `cmd_run` before writing anything.

2. **The `diskpart set id=` remedy has never been exercised, and may be a dead
   end.** `plan::assess` refuses Windows basic-data and tells the user to
   retype the partition themselves. But every Windows-made partition on the
   golden carries GPT attribute bit 63 (`0x8000000000000000`), and `assess`
   also refuses any partition with attributes set. If `set id=` leaves the
   attribute in place, the remedy leads straight into a second refusal. A job
   that runs the remedy on a fixture overlay and re-surveys settles it in one
   guest boot.

3. **Priority 4 — payload deployment — with a synthetic payload only.** Write
   a few MB of known SHA256 through `\\.\PhysicalDriveN` at the verified
   offset, then `cmp` it back from the host against `fixture-a.raw`. Read
   `ARCHITECTURE.md`'s "Exclusivity" section first: there is no volume to lock,
   so the write path's safety is offset validation plus a volume re-enumeration
   immediately before each write, plus Windows' own refusal to write through a
   `PhysicalDrive` handle into a region a mounted volume owns.

4. **Priorities 5 and 6 — the ESP transaction and undo — have not started.**
   The design is in `ARCHITECTURE.md`. The firmware before/after diff the lab
   already performs is the assertion they have to satisfy.

5. **Two product decisions to put to Andre, not to solve here.**
   - The composefs path needs ~1.1 GiB of ESP; a stock Windows ESP has
     **68.3 MiB free, measured**. Either Windows-side installs stay on
     ostree/GRUB, or the installer has to create a second ESP — which is a GPT
     modification, and avoiding one is the point of the whole design.
   - Should the tool ever retype a basic-data partition itself? Today it
     refuses and hands the user a `diskpart` command.

6. The GUI does not exist. The confirmation text does, and is unit-tested; the
   screen that shows it does not.

If this agent is dead, do not message it. Read this file, then continue in the
worktree above.
