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
text, 10 unit tests that run on Linux), `src/main.rs` gains `survey` and
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

1. Read `/var/lab-scratch/winlab/run2.log` — the first guest run of `survey` +
   `inspect` against the fixtures. Fix whatever it says and re-run with
   `windows-installer/lab/winlab run windows-installer/lab/jobs/survey`.
2. Run it again with `--swap` and assert the same partition GUID produces the
   same identity text when the NVMe/AHCI enumeration order changes.
3. `APEX_WINLAB_GUEST=1 tests/test-windows-installer.sh` is the full suite.
   Without KVM it reports `could-not-run` for the guest stage, never a pass.
4. Then priority 4 — payload deployment — with a synthetic payload: write a
   few MB of known SHA256 through the locked volume handle and `cmp` it back
   from the host against `fixture-a.raw`. Nothing real, nothing signed.
5. Product decision to surface, not to solve: `plan::assess` refuses Windows
   basic-data even when zeroed, which is exactly the partition a user makes by
   shrinking C:. The refusal carries a `diskpart set id=` remedy. Andre should
   decide whether the tool ever retypes a partition itself.

If this agent is dead, do not message it. Read this file, then continue in the
worktree above.
