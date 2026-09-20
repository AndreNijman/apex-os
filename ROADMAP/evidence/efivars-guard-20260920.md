# The efivars guard — what was built, and how it was proven, 2026-09-20

> **SUPERSEDED THE SAME NIGHT, AND WRONG ABOUT ITS CENTRAL CLAIM.** Everything
> below describes the `--tmpfs /sys/firmware/efi/efivars` mask as the layer
> that prevents the 2026-09-20 breakage. **It prevents nothing.** `bootc` takes
> `--pid=host` and re-enters the HOST's mount namespace for the bootloader
> step, so a tmpfs inside the container is irrelevant to it; and an unmasked
> privileged container shows *zero* entries under `/sys/firmware/efi/efivars`
> anyway, so the host's efivarfs was never in the container's view to be
> masked. At 21:53:26 the same evening a loopback install ran **with the mask
> applied and its promise printed** and moved `Boot0000` off the real ESP — four hours after
> this unit landed as "the fix". `nvram-guard`, the layer this document calls
> secondary, is what caught it.
>
> The prevention is `bootc install --generic-image`. See
> `ROADMAP/evidence/efivars-guard-2-20260920.md` — unit `efivars-guard-2` —
> for what replaced this, and for the two things this document got factually
> wrong: neither incident was a `--via-loopback` command, and section 0's
> "there was no caller to fix" missed the caller that caused both of them
> (`installer/apex-install`, on a `to-filesystem` install against a loop
> device it had attached itself).
>
> The account below is kept because the mutation harness, the binary-file and
> empty-`git ls-files` traps in section 4, and the nvram-guard design are all
> still correct and still in force.

Unit `efivars-guard`. Follow-up 1 of `BOOT-BREAKAGE-2026-09-20.md`:

> *"The installer test path must mask efivars. Any `bootc install to-disk
> --via-loopback` run needs `--tmpfs /sys/firmware/efi/efivars`. Worth asserting
> it in whatever wraps these runs rather than relying on each caller, and worth
> a pre/post `efibootmgr -v` diff in the lab harness so a repeat is caught in
> seconds instead of at the next power-on."*

Nothing in this unit wrote an EFI variable, started a container, or ran
`bootc install`. `efibootmgr` was invoked read-only, with `-v` and nothing else.

## 0. There was no caller to fix. That is the finding.

`grep -rn 'via-loopback'` over `apex-os` at `roadmap/v2.2` (`4c2478fc`) matches
**nothing outside prose** — the only hits in the whole tree are the two command
transcripts in `ROADMAP/evidence/sdboot-image-20260920-lab.md`. Every one of
the eight loopback installs that night was typed at a shell. `installer/`'s
`bootc install to-disk --wipe` is a real install onto real hardware from the
live ISO, which *must* write NVRAM and is explicitly out of scope.

So "make the existing callers use it" is not a refactor. The deliverable is
the path that did not exist: a wrapper that cannot launch without the mask, a
gate that keeps any future scripted caller on it, and a second layer that
catches a run that went around both.

## 1. What a caller does now

```
tests/lab/bootc-install-lab [--dry-run] [--size S] [--filesystem FS]
                            [--bootloader NAME] [--podman-arg ARG]... [--out DIR]
                            IMAGE TARGET.img [-- BOOTC_ARG...]
```

The caller never types a podman argument list. The wrapper builds it, always
inserting the mask, and runs the whole thing under `tests/lab/nvram-guard`.

```
$ tests/lab/bootc-install-lab --dry-run localhost/apex-os:daily \
      /var/lab-scratch/efivars-guard/demo.img

bootc-install-lab --dry-run: nothing was launched. The command is:

tests/lab/nvram-guard --label bootc-install-lab:demo.img -- podman run --rm \
  --privileged --pid=host --security-opt label=type:unconfined_t \
  -v /var/lib/containers:/var/lib/containers -v /dev:/dev \
  --tmpfs /sys/firmware/efi/efivars \
  -v /var/lab-scratch/efivars-guard:/work localhost/apex-os:daily \
  bootc install to-disk --via-loopback --generic-image --wipe \
  --filesystem ext4 /work/demo.img

efivars-mask-present: ok (--tmpfs /sys/firmware/efi/efivars is in the argv)
```

## 2. What happens if they forget

They cannot forget the mask — it is not theirs to pass. What they *can* do is
edit the wrapper. Delete the one line that injects the mask and the named
assertion refuses, before anything is launched and before the target file is
created:

```
$ /var/lab-scratch/efivars-guard/mut/bootc-install-lab --size 1M \
      localhost/apex-os:daily /var/lab-scratch/efivars-guard/mutant-demo.img

!!! bootc-install-lab ASSERTION FAILED [efivars-mask-present]
    The podman argv about to run carries no tmpfs over
    /sys/firmware/efi/efivars, so a --privileged container would see this
    machine's real UEFI boot variables read-write.
    Nothing was launched. This is the 2026-09-20 breakage, refused.
    argv: run --rm --privileged --pid=host … -v /dev:/dev -v …:/work
          localhost/apex-os:daily bootc install to-disk --via-loopback …
rc=6

$ ls /var/lab-scratch/efivars-guard/mutant-demo.img
No such file or directory
```

The other refusals, all named, all before launch:

| refusal | fires on |
| --- | --- |
| `caller-unmasks-efivars` | `--podman-arg` mounting anything at `/sys`, `/sys/firmware`, `/sys/firmware/efi` or the efivars leaf — a tmpfs over the leaf is undone by a bind of any parent, and `-v /sys:/sys` is the realistic mistake |
| `target-is-a-device` | a `/dev/...` or block-device target; real hardware is `installer/apex-install`'s job |
| `target-is-in-ram` | a target under `/tmp`, `/dev/shm`, or any tmpfs/ramfs — the sdboot lab's first runs put 20 GiB images in a 15 GB tmpfs |

A mount that is *not* a parent of efivars (`-v /sys/fs/cgroup:/sys/fs/cgroup`)
passes and still carries the mask; a guard nobody can use gets bypassed.

## 3. The second layer, on the real machine, read-only

`tests/lab/nvram-guard -- <command>` snapshots **two independent sources**
before and after: `efibootmgr -v`, and a sha256 of every `Boot*` variable in
efivarfs (`Boot0000…`, `BootOrder`, `BootCurrent`, `BootNext`). Run against
this laptop's real firmware:

```
$ tests/lab/nvram-guard --label evidence --out … -- true
nvram-guard[evidence]: before: 29 Boot* variables, 26 entries
nvram-guard[evidence]: verdict: verified — boot variables identical before and after (command exited 0)
rc=0

before/efibootmgr.txt  9f389cc6c3008ba48ebeaad3519c02bca704c47e43f1f3adbb5cb522e4e3b3c4
after/efibootmgr.txt   9f389cc6c3008ba48ebeaad3519c02bca704c47e43f1f3adbb5cb522e4e3b3c4
before/efivars.sha256  1cea01293ab86720c715bb85463b7347b55556d35c2a5b423ff946ae857044de
after/efivars.sha256   1cea01293ab86720c715bb85463b7347b55556d35c2a5b423ff946ae857044de

BootCurrent: 0000
BootOrder: 0000,0020,001D,001E,001F,0021,0022,0023,0024,0025,0004
Boot0000* APEX-OS	HD(1,GPT,1c417de2-5766-455f-9318-198610885424,0x800,0x12c000)/\EFI\fedora\shimx64.efi
```

`1c417de2-…` is the repaired entry from Andre's live-USB fix — the real
`nvme0n1p1`, not the `ee26313c-…` that existed on no disk. This unit left it
exactly where it found it.

Three verdicts, never a boolean, following `tests/chaos/lib.sh` and
`apexd/apex/src/verify.rs`:

* `verified` — both snapshots real, both identical.
* `nvram-changed` — they differ. Unified diff printed, exit **3**, whatever the
  wrapped command returned.
* `could-not-snapshot` — UEFI is present but `efibootmgr -v` gave nothing or has
  no `BootOrder:` line, or efivarfs yielded no `Boot*`. The command is **not
  run**, exit **4**. An empty snapshot compared against another empty snapshot
  is "permission denied reported as absence" with a boot path underneath it.
* `could-not-run` — no `/sys/firmware/efi` at all. The command runs, its exit
  status is propagated, and the guard says plainly that it measured nothing. It
  never prints "unchanged" about something it never read.

## 4. How both directions were proven without reproducing the damage

`tests/test-bootc-install-guard.sh`, **52 assertions, 0 failures**, wired into
`pr-validation.yml`'s `static` job. `podman` and `efibootmgr` are stubs on
`PATH`; the firmware root is a fixture tree behind `APEX_NVRAM_EFI_ROOT`. The
assertion under test is about *launch arguments*, which need no launch.

| claim | how it is proven, and by whom |
| --- | --- |
| the mask reaches podman | read out of the **stub podman's recorded argv file**, not the wrapper's stdout |
| the assertion fires without it | a **copy** of the wrapper with the mask line deleted → rc 6, stderr names `efivars-mask-present`, `podman.argv` does not exist, no image file created |
| the mutation was real | `grep -c '# efivars-mask$'` is 1 on the shipped wrapper and 0 on the mutant — a `sed` that matched nothing would otherwise pass this |
| a caller cannot unmask it | four parent paths and a `--mount target=` form, each rc 5 and named; plus one allowed mount that must still carry the mask |
| the NVRAM diff inspects something | each of its two sources is moved **on its own** — changed efivarfs digest with static `efibootmgr -v`, then changed `efibootmgr -v` with static digests — and each alone must produce `nvram-changed` |
| the guard never writes NVRAM | every recorded `efibootmgr` argv is asserted to be exactly `-v` |
| an unmeasurable run is not a pass | empty `efibootmgr` output → rc 4, and a marker file proves the command **did not run** |
| a non-UEFI host is not a pass either | fixture root removed → command runs, its rc 7 is propagated, verdict `could-not-run`, and "verified" appears nowhere |
| the repo scan works | planted fixtures **three ways** — the 2026-09-20 command shape is caught, the same command with the mask is not, and a mask present only in a trailing comment is still caught — before the repo's own clean verdict is believed |
| the repo scan is not vacuous | the file list is captured and asserted against a floor (842 files today), and `installer/apex-install` — a file that really holds a `bootc install` line — must be inside it. Proven by running the suite from a copy with no `.git`: two named failures where the old version printed a green scan |

And the suite was proven to fail on a **real** regression, not only a copied
one: deleting the mask line from the shipped `tests/lab/bootc-install-lab` in
the working tree turns the run red with seven named failures, headed by
`the shipped wrapper has exactly one mask-injection line: want '1', got '0'`.
The line was restored from git immediately afterwards.

Two traps paid for here. `git ls-files` can come back **empty** — CI checkout
falls back to a tarball with no `.git`, which this repository has already been
bitten by — and an empty list made the scan report *"no tracked file runs an
unmasked `--via-loopback` install"* having read nothing at all. And `awk` over
every tracked file **aborts with a glibc malloc assertion** on `files/branding/plymouth/previews/preview-gold.gif` and
the wallpaper JPEG — and the aborted scan still printed *"no tracked file runs
an unmasked `--via-loopback` install"*. Binary files are now skipped
explicitly. A scanner that crashed on part of the tree — or read no tree at
all — and reported a clean result is this repository's signature defect, and
both versions of it were live in this suite for one run each.

A third: the scan accepted a mask that existed only in a trailing comment, and
exempted any line merely *mentioning* `bootc-install-lab`. Comments are cut
before both tests now, and the useless exemption is gone.

## 5. The residual gap, stated rather than implied

Nothing in a repository can stop somebody typing `podman run --privileged`
at a prompt. The honest claim is narrower and still worth having:

* every **scripted or documented** path to a loopback install now goes through
  the wrapper, and CI fails a PR that adds one that does not;
* a run that goes around the wrapper anyway is caught by `nvram-guard` **when
  the command returns**, instead of at the next power-on — provided the person
  wrapped it, which AGENTS.md boot-path rule 6 now asks for in one line.
