# integrated-image — DONE. The image is built and qualified on katana.

Repo apex-os, branch **`task/integrated-image`** (tip `20e21d86`), cut off
`roadmap/v2.2` @ `97c9e8f2`. Its three commits are evidence files ONLY; the
image content is exactly `roadmap/v2.2`'s tree. Worktree
`/var/tmp/apex-work/wt-integrated-image`.

## The artifact

**Run 35461554871** — `workflow_dispatch` on `task/integrated-image` @
`97c9e8f2`, queued 2026-09-19T18:33:05Z, **`success` at 19:45Z, 1 h 12 m**.
https://github.com/AndreNijman/apex-os/actions/runs/35461554871

```
ghcr.io/andrenijman/apex-os:apex-97c9e8f25ee55593a97502505f51c6115ebbee7c
  digest sha256:55fc9e4ee9b2c1b27045cba5a594a40e97fb0d73c70170cfbe0003f5cd4d73b0
```

- Every job's STEPS were read, not just its conclusion: `core` 13/13 success,
  `image` 17/17 success. `installer-iso` and `qcow2` are `skipped` because
  their dispatch inputs default false — that is the "skipped counts as
  success" trap, checked rather than assumed.
- `Pinned apex-shell roadmap/v2.2 (apex-shell has no branch named
  task/integrated-image): 03d77f96…` — the ordered fallback (`12e9d454`)
  worked. Both halves are the same age. This is NOT a main-shell build.
- `not publishing from refs/heads/task/integrated-image: per-SHA tags written`
  — no floating tag moved, no machine but katana saw this image.
- `core rebuild: true (core sources changed)`, so katana's pull was the whole
  image, 6.3 GB.

## All four rows: CONFIRMED ON HARDWARE

Numbers are in `ROADMAP/evidence/integrated-image-20260920.md`; each unit's
own `closed` note in `queue.json` carries its own.

| row | discriminator, as it actually read |
|---|---|
| **pkg-update** | rebuild **53.8 s** vs **33 ms** the boot before; journal says `level 2 -> … level 3 — rebuilding`; `pkg_compat_level` 3; ext sha `eb3b8ba0` → `75290ae7`; 13 i686 ICDs; `comm -12` vs `rpm -qal` **empty** |
| **gaming-release** | `owner_pid : 7799`; after SIGKILL, apexd's `the session owner is gone (/proc/9688 is gone)` **1.9 s** later; `active : false`; cgroup removed; `--mangoapp` absent; **0** new mangoapp dumps (was 15 376), **0** Glfw lines (was 57 612), **171** journal lines (was 144 187) |
| **p2-b** | build compiled `apex-shell_de.qm`; on the machine the offscreen probe says `APEXI18N: installed …/apex-shell_de.qm` under `de_DE`, and honestly `no catalogue for C` under `C` |
| **coredump** | `MaxUse=256M` / `KeepFree=2G` live; neither line exists without the drop-in |

### Three things that are true and easy to misread
- **54 s is not "minutes".** katana's rpms were cached. 54 s vs 33 ms is still
  an unambiguous discriminator; the earlier wording was optimistic.
- **The extension is byte-different with an IDENTICAL file set** (0 newly
  carried, 0 lost). The resolved set of 223 did not move — the *level* did,
  which is the entire fix.
- **The run-book's §6.6 recipe does not test §6.6.** `systemctl restart
  greetd` left the session alive long enough to run its trap, which released
  game mode cleanly by the ordinary path. Only SIGKILLing the owner is
  "nothing can cooperate". **Amend the run-book.**

## TWO DEFECTS FOUND. Neither fixed, and that was deliberate.
The image under qualification was already built and on the machine; an engine
change now would invalidate the run it is embedded in.

1. **`apex install` mislabels every file it writes into `/etc`.**
   `relabel_tree` runs `setfiles` on the extraction tree's `/usr` and `/opt`
   only; `install_etc` copies into the live `/etc` with `cp -a`, which keeps
   the source label. All 7 paths in `etc.list` were wrong.
   `/etc/.pwd.lock` as `rpm_var_lib_t` breaks `DynamicUser=yes` for **six**
   units — `capsule@.service`, `rpm-ostreed`, `fwupd-refresh`,
   `rpm-ostree-countme`, `chrony-wait`, `wsdd` — **with no AVC logged**,
   because the denial is on a path PID 1 takes before any transition.
   **It recurred on the level-3 rebuild** with a *different* wrong type
   (`var_lib_t`, which happens to be permitted), so it fires every rebuild and
   breaks the machine only sometimes. `restorecon` fixes it; applied and
   verified twice. Shape of the fix: `relabel_tree` must cover `${root}/etc`,
   and `install_etc` must `restorecon` what it writes.
   `ROADMAP/evidence/katana-pwd-lock-selinux-20260920.md`.
2. **Gaming Mode has never loaded a sched-ext scheduler.**
   `apexd/apexd-core/src/syswriter.rs:739` runs `scxctl switch -s scx_lavd`;
   with nothing running, scxctl answers *"no scx scheduler running, use
   'start' instead of 'switch'"*. apexd logs that failure and the **next**
   line — which is also `apex game status`'s `notes` field — claims
   `sched-ext: scx_lavd for the session` as fact. Present on boots `-1` and
   `-2` too, so **not a regression**. Consequence for anyone re-running the
   run-book: **`sched_ext/state` is not a discriminator on katana**, exactly
   like the governor.

## katana, as it is left
- Booted `apex-97c9e8f2…`, digest `sha256:55fc9e4e…`.
- **Three** deployments. `bootc status`'s `rollback` slot now holds
  `apex-7f647470` (the rotated one); the **September 18 `apex-266dcc57`
  survives underneath it as a third deployment with `Pinned: yes`** — it was
  pinned *before* the switch so the rotation could not prune it. A stranger
  reading only `bootc status` will think it is gone. `ostree admin pin -u 2`
  to unpin.
- greetd **byte-identical** to `config.toml.orig-qual2` (`cmp`), `0`
  `initial_session`, `0` timers armed, `greetd` active, greeter live on tty1
  (sway + swaybg + apex-greet).
- `~/apex-pre-rebase-20260919/` intact, 4 files, 2.1 M. Nothing left mounted.
  `/var` 54 G free.
- `/etc` labels `restorecon`ed — **for now**. The next extension rebuild
  re-breaks them.
- **THE NVMe CONTROLLERS SWAPPED AGAIN across this reboot.** The APEX Micron
  `220534D1CB81` moved `nvme0n1` → `nvme1n1`; Windows SPCC `240023925111005`
  moved `nvme1n1` → `nvme0n1`. `/var` is on the APEX disk, verified by LABEL
  (`apex-root`) and serial. Never trust a device name here.
- Scratch, kept as the record:
  `/var/tmp/apex-work/scratch-integrated-image/` — `post-boot.sh`, `game.sh`,
  the pre/post extension file lists, `session-intgame.log`.

## Two traps this run paid for
- **`bootc switch` over ssh is SIGHUPed when the channel closes**, and a 600 s
  tool cap closes it. The 6.3 GB pull survived in the image store but nothing
  was staged and `bootc status` said `staged: none` — which reads exactly like
  a failed pull. Re-run it detached:
  `sudo systemd-run --unit=intimg-switch --collect
  --property=TimeoutStartSec=infinity --service-type=oneshot /usr/bin/bootc
  switch --transport registry <IMG>`. The second run found `No changes` and
  staged in 10 s.
- **`owner_pid` is only in `apex game status` while a session is ACTIVE**
  (`apexd/apexd/src/game.rs` ~457). Its absence on an idle machine is by
  design. The idle discriminator is
  `strings -a /usr/bin/apexd | grep -c 'the session owner is gone'`.

## NEXT — this unit has nothing left. What is left is ANDRE'S, in this order
1. **On the L16, before anything else, one read-only command:**
   `stat -c %C /etc/.pwd.lock` — want `passwd_file_t`. The L16 has run
   `apex install`. If it reads `rpm_var_lib_t`, its `rpm-ostreed` and APEX
   Capsules are already dead and nothing has said so; `sudo restorecon
   /etc/.pwd.lock` fixes it. **This agent did not run it: the L16 is off
   limits.**
2. **Andre's call, not an agent's:** whether the two defects above land on
   `roadmap/v2.2` first. Neither blocks the qualification, which stands. But
   the L16 rebuilds its clippy extension on the first boot after this image,
   and defect 1 fires on every rebuild.
3. If (2) is yes, **one more image build** — same dispatch, new tip.
4. **Boot the L16 onto the per-SHA tag** and qualify it. It will be the first
   machine to answer the one question katana could not: whether
   `systemd-sysext refresh` re-merging `/usr` disturbs a **live desktop**.
   Katana ran only the greeter. It will happen under Andre's working session;
   `Nice=10` and `IOSchedulingClass=idle` bound the cost, the `/usr` re-merge
   does not care about either.
5. **Then, and only then, the two merges, in THIS order:**
   **apex-shell `roadmap/v2.2` → `main` FIRST**, because
   `Containerfile.base` carries `ARG APEX_SHELL_REF=main` (lines 93 and 769) —
   land apex-os first and the next main build vendors an apex-shell months
   behind and dies in `check-labwc-keybinds`. **THEN apex-os `roadmap/v2.2` →
   `main`.** This agent opened no PR and merged nothing, by instruction.
