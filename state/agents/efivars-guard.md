# efivars-guard — follow-up 1 of BOOT-BREAKAGE-2026-09-20.md

items: none (this unit is in neither `roadmap.yaml` nor `dispatch.json`; it was
       dispatched directly off Andre's own write-up)
repo: apex-os
worktree: /var/tmp/apex-work/wt-efivars-guard
branch: task/efivars-guard — 3 commits, `4c2478fc..5e23536a`, all PUSHED, not landed
base: roadmap/v2.2 @ 4c2478fc
evidence: ROADMAP/evidence/efivars-guard-20260920.md
files owned: tests/lab/bootc-install-lab, tests/lab/nvram-guard,
             tests/test-bootc-install-guard.sh
files shared with other units: AGENTS.md (appended boot-path rule 6),
             .github/workflows/pr-validation.yml (one step added in `static`,
             immediately after the existing `test-boot-v2.sh` step)
touched of another unit's territory: **nothing**. No `installer/**`, no
             `files/scripts/boot-v2/**`, no `Containerfile.*`, no `android/**`,
             no `windows-installer/**`, no `kernel/**`.

## NEXT

Read this before picking it up cold.

1. **The unit is done and green, and nothing here is blocking.** 49 assertions,
   0 failures, wired into `pr-validation.yml`'s `static` job. The branch is
   pushed and wants landing into `roadmap/v2.2` by an integrator, as a MERGE —
   round 10 changed the mechanism and a cherry-pick will conflict.
2. **The finding that shaped it: there was no caller to fix.** `via-loopback`
   matches nothing executable anywhere in the tree at `4c2478fc` — every one of
   the eight loopback installs on the night of 2026-09-19/20 was typed at a
   shell, and the only trace is
   `ROADMAP/evidence/sdboot-image-20260920-lab.md`. So this is not a refactor
   of existing callers; it is the path that did not exist, plus a repo scan
   that keeps the next scripted caller on it.
3. **`sdboot-image` is the unit that will actually use this.** When it resumes
   it should launch its loopback installs as
   `tests/lab/bootc-install-lab IMAGE /var/lab-scratch/…/x.img -- <bootc args>`
   instead of typing `podman run`, and wrap its **bootlab container launch**
   (the host-side `podman run … localhost/apex-bootlab …`) in
   `tests/lab/nvram-guard -- …`. It was deliberately NOT edited here:
   `files/scripts/boot-v2/run-scenarios` runs *inside* the bootlab container,
   which has no host efivars to read, so a diff there would inspect nothing.
   The right layer is whoever launches the container, and that is a caller this
   unit does not own.
4. **What is still only a convention.** Nothing in a repository stops a person
   typing `sudo podman run --privileged …` at a prompt. AGENTS.md boot-path
   rule 6 asks for `nvram-guard --` around anything that touches a loopback
   install or boots a lab guest; that request is not enforceable by CI. If a
   stronger primitive is ever wanted, the one worth investigating is remounting
   the host's efivarfs read-only for the duration of a lab session — it was
   considered and NOT built here, because it changes host mount state globally
   and would make a genuine `bootc install` onto real hardware fail in a
   confusing way.
5. **Do not "improve" the guard by making the mask a default a caller can
   override.** The whole point is that it is not a caller's argument. The
   suite's mutation case exists to catch exactly that change.

## What a caller has to do now, and what happens if they forget

```
tests/lab/bootc-install-lab [--dry-run] [--size S] [--filesystem FS]
                            [--bootloader NAME] [--podman-arg ARG]... [--out DIR]
                            IMAGE TARGET.img [-- BOOTC_ARG...]
```

Run it under `sudo`; `podman` is resolved from `PATH` on purpose so the suite
can intercept it with a stub.

They cannot forget the mask, because they never pass it — the wrapper builds
the whole podman argv. The failure modes that remain are all refusals, by name,
before anything is launched and before the target file is created:

| exit | name | fires on |
| --- | --- | --- |
| 6 | `efivars-mask-present` | the assembled argv carries no tmpfs over `/sys/firmware/efi/efivars` — i.e. somebody edited the wrapper |
| 5 | `caller-unmasks-efivars` | a `--podman-arg` mounting anything at `/sys`, `/sys/firmware`, `/sys/firmware/efi` or the leaf. A tmpfs over the leaf is undone by a bind of any parent, so `-v /sys:/sys` is refused too |
| 5 | `target-is-a-device` | a `/dev/...` or block-device target. Real hardware is `installer/apex-install`'s job and that path must write NVRAM |
| 5 | `target-is-in-ram` | a target under `/tmp`, `/dev/shm` or any tmpfs/ramfs |
| 4 | `could-not-snapshot` | UEFI present but NVRAM unreadable — the command is **not run** |
| 3 | `nvram-changed` | the host's boot entries moved across the run. Diff printed |

## How both directions were proven, without reproducing the damage

`tests/test-bootc-install-guard.sh`. `podman` and `efibootmgr` are stubs on
`PATH`, the firmware root is a fixture tree behind `APEX_NVRAM_EFI_ROOT`, and
no `bootc install` runs, no container starts, no EFI variable is written and no
real NVRAM is read. The assertion under test is about *launch arguments*, which
need no launch.

* **passes when the mask is present** — proven from the **stub podman's
  recorded argv file**, not from the wrapper's own stdout.
* **fails when it is absent** — a copy of the wrapper with the single
  mask-injection line deleted exits 6, names `efivars-mask-present`, never
  invokes podman, and creates no image file. The mutation is itself verified
  (`grep -c '# efivars-mask$'`: 1 shipped, 0 mutant), because a `sed` that
  matched nothing would otherwise pass this.
* **the suite fails on a REAL regression too** — deleting that line from the
  shipped wrapper in the working tree turns the run red with seven named
  failures. Restored from git immediately.
* **each NVRAM source is load-bearing on its own** — the efivarfs digests and
  `efibootmgr -v` are moved independently and each alone must produce
  `nvram-changed`.
* **the repo scan is proven against planted fixtures both ways** before its
  clean verdict on the repository is believed, because there are zero loopback
  callers today and a scan that finds nothing proves nothing.

## Read-only evidence from this machine

`tests/lab/nvram-guard --label evidence -- true` against the L16's real
firmware: 29 `Boot*` variables, 26 entries, before and after byte-identical on
both sources, and

```
Boot0000* APEX-OS	HD(1,GPT,1c417de2-5766-455f-9318-198610885424,0x800,0x12c000)/\EFI\fedora\shimx64.efi
BootOrder: 0000,0020,001D,001E,001F,0021,0022,0023,0024,0025,0004
```

— which is Andre's repaired entry, untouched. `efibootmgr` was invoked with
`-v` and nothing else, and the suite asserts that from the recorded argv.

## One trap paid for here, worth keeping

`awk` over every tracked file **aborts with a glibc malloc assertion** on
`files/branding/plymouth/previews/preview-gold.gif` and the wallpaper JPEG —
and the aborted scan still printed *"no tracked file runs an unmasked
--via-loopback install"*. Binary files are now skipped explicitly. A scanner
that crashed on part of the tree and reported a clean result is this
repository's signature defect, and it was live for exactly one run.
