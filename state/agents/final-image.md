# final-image — DONE. The image is built from the v2.2 tip and both fixes are
# confirmed on katana.

repo: apex-os · branch **`task/final-image`** (tip `40ebf4cb`), cut from
`roadmap/v2.2` @ `661a9d80`. Its two commits are an evidence file ONLY; the
image content is exactly `roadmap/v2.2`'s tree.
worktree: `/var/tmp/apex-work/wt-final-image`
evidence: `ROADMAP/evidence/final-image-20260920.md` (tracked, on the branch)
P0-001 recorded `partial` with `set-status.py`; prior evidence read out with
`yaml.safe_load` first and carried forward **whole** — verified afterwards by
`prior in evidence` → True, 17 458 → 21 560 chars, and a per-item diff showing
P0-001 as the only row that moved, 128 tasks still parsing,
`global_agent_rules` intact.

## THE ARTIFACT

**Run 35469530380** — `workflow_dispatch` on `task/final-image` @ `661a9d80`,
queued 2026-09-19T21:08:57Z, **`success` at 22:29Z, 1 h 20 m 01 s**.
<https://github.com/AndreNijman/apex-os/actions/runs/35469530380>

```
ghcr.io/andrenijman/apex-os:apex-661a9d80d2239d848676d97e1e633d3f7325e853
  digest sha256:61f7935c259fa90362c09158fd355f32230e0ab21925340c3c2acf4f879bffcc
```

`:daily-661a9d80…` resolves to the same digest. Both strings were read out of
the `image` job's promote step, not constructed.

- Every job's STEPS were read, not its conclusion: `core` 12/12, `base` 13/13,
  `image` 17/17. `installer-iso` and `qcow2` are `skipped` because their
  dispatch inputs default false — the "skipped counts as success" trap, checked.
- `Pinned apex-shell roadmap/v2.2 (apex-shell has no branch named
  task/final-image): 03d77f96…` — the ordered fallback worked, and it is the
  **same shell sha `apex-97c9e8f2` carried**. Not a main-shell build.
- `not publishing from refs/heads/task/final-image: per-SHA tags written` — **no
  floating tag moved**, so the L16 cannot take this image by accident.
- `core rebuild: true (core sources changed)` is a `dorny/paths-filter`
  artefact of a first dispatch on a new branch (no base to diff), not a real
  core change — the diff touches no `Containerfile*` and no `kernel/**`. Cost:
  ~45 min of build and a whole-image pull. The previous image run did the same.

**Log bytes contain ANSI escapes, so plain `grep` calls the file binary and
prints NOTHING.** Use `grep -a`. That silence reads exactly like a real absence.
`gh run view --log` refuses while a run is in progress;
`gh api repos/AndreNijman/apex-os/actions/jobs/<id>/logs` does not.
Job ids: rust 105967845288, changes 105967845376, installer-iso 105967845750,
core 105967873637, base 105973854384, image 105975942716.

## BOTH FIXES: CONFIRMED ON HARDWARE

| fix | how it reads on katana |
|---|---|
| **`/etc` labelling** | a real `sudo apex install` wrote **56** `/etc` paths, **55 match the loaded policy exactly**, 40 of them wanting a non-`etc_t` type. `DynamicUser=yes` probe `result: success`; `rpm-ostreed` and `capsule@` both `active`/`Result=success`; `systemctl --failed` empty; `/etc/.pwd.lock` `passwd_file_t`. The unattended path (`apex-sysext-rebuild.service`, forced level 2→3) wrote **44 paths, 0 mismatched, no AVC**. |
| **sched-ext verb** | `apex game status` reports what it read from the kernel. `scx_state : not loaded` with a **true** reason, **zero** `no scx scheduler running` refusals this boot, and the `switch` retry branch fired on hardware. |

The single "mismatch" is `/etc/ld.so.cache`: type correct, only the SELinux
**user** field differs, written by `ldconfig` *after* the relabel pass, named by
no tracked list. Not the fix's population.

**The machine was made to fail first**, so the pass is not just the clean
baseline — and it failed by accident, which makes it better evidence. See
evidence §2.4–§2.6.

**Why `not loaded` is a pass, and it is not the `SCX_SETTLE` budget.** No
sched-ext scheduler can attach on this image at all: the kernel's BTF was built
with `pahole < 1.26`, so every `scx_*` scheduler dies on
`func_proto incompatible with vmlinux` / `failed to load BPF skeleton`. The
kernel is **`7.2.6-cachyos1.fc43.x86_64`** and `rpm -q kernel` says *not
installed* — it is the CachyOS kernel this repo bakes from `kernel/**`, which is
where the BTF is generated and where the follow-up has to land.
`scx_loader` retries five times and gives up. `state` never read `enabling` and
`scx_state` never read `unknown`, so raising `SCX_SETTLE` would fix nothing.
**This is a separate, unfiled item.** `root/ops` is therefore still unread on
any machine, and `lavd` is still an unchecked expectation.

## The one new finding, and it is not a defect in any path APEX takes

`restorecon` hands its path list to the kernel as a **file**, and whether it can
open that file depends on the domain `apex-pkg` runs in. Measured three ways:

| how apex-pkg is reached | domain | `mktemp` type | `restorecon -F -i -f` |
|---|---|---|---|
| `apex-sysext-rebuild.service` (ExecStart is the `bin_t` script) | `unconfined_service_t` | `tmp_t` | works |
| a unit whose ExecStart is `bash -c "…"` | `initrc_t` | `initrc_tmp_t` | **denied** |
| interactive `sudo apex install` | `unconfined_t` | `user_tmp_t` | works |

`restorecon` execs `setfiles` and transitions to `setfiles_t`, which cannot read
an `initrc_tmp_t` file. **Every caller in the tree was enumerated, not sampled**:
the rebuild unit execs the `bin_t` script directly; `ops.rs:563` uses
`Command::new(PKG_ENGINE).args()`; `recover.rs:211` and `blueprint.rs:1113` pass
argv arrays; `apexd` itself runs `unconfined_service_t` (`ps -eo label` on the
machine). None goes through a shell. `apex-boot-health`, `apex-env` and the
flatpak unit name `apex-pkg` in comments only.

The fragility is real all the same: a future caller reaching `apex-pkg` through
a shell inside a unit loses the relabel and the install still exits 0.

**The hardening is one line and it was measured, not guessed:**
`printf '%s\n' … | restorecon -F -i -f -` succeeds from the denied domain,
because stdin needs no `open()`. **Deliberately not made** — an engine change
now would invalidate the image this evidence is about.

## katana, as it is left

Booted `apex-661a9d80…` / `sha256:61f7935c…`. Three deployments: booted,
`apex-97c9e8f2` as rollback, **September 18 `apex-266dcc57` still `Pinned: yes`**
underneath (a stranger reading only `bootc status` will think it is gone).
`apex-7f647470` was pruned by the rotation — unpinned, expected.

- `efibootmgr -v` **byte-identical** by `cmp`. Nothing written to the Windows
  disk. `Boot0000` still on the Windows disk's ESP.
- **The NVMe controllers swapped again — the third time.** APEX Micron
  `220534D1CB81` is `nvme0n1` NOW (it was `nvme1n1` before this reboot);
  Windows SPCC `240023925111005` is `nvme1n1`. `/var` verified by LABEL
  `apex-root` and serial. Never trust a device name here.
- greetd `cmp`-identical to `config.toml.orig-qual2`, `0` `initial_session`,
  active, no timer armed. **greetd was never edited** — §6.8 is headless and no
  dead-man timer was needed.
- `~/apex-pre-rebase-20260919/` intact, 4 files, 2.1 M.
- Package set back to its original eight requests; 223 packages, 512 MB
  extension. `cronie`, `vim-enhanced`, `nginx` were added for the measurement
  and removed — **removals verified by `ls`, not by the ctime diff, which cannot
  see a deletion**. Every removed file gone, no image-owned `/etc` file taken
  with them, `etc.list` back to the same 11 entries as baseline with **0
  mismatched**. The removal pass leaves empty directories behind; all seven were
  `rmdir`'d after `rpm -qf` confirmed no package owned them.
- `systemctl --failed` empty, `/var` 51 G free, all transient probe units
  collected, scratch removed. `/etc` clean — and unlike last round, **nothing
  was `restorecon`ed by hand to make it so**; the last thing to touch it was
  `apex remove`.

## NEXT — for a stranger

**Nothing in this unit is outstanding.** The branch is pushed, the evidence is
committed, P0-001 is recorded. What remains is Andre's, in this order:

1. **Merge apex-shell `roadmap/v2.2` → `main` FIRST.** `Containerfile.base` line
   93 carries `ARG APEX_SHELL_REF=main`; the wrong order vendors a months-old
   shell and dies in `check-labwc-keybinds`.
2. **Then merge apex-os `roadmap/v2.2` → `main`.** That is the `final` unit and
   it is Andre's call. This unit opened no PR and merged nothing.
3. **Wait for the `main` build-image run.** The floating tags only move on a
   `main` build — that is why the L16 could not have taken this image.
4. `sudo apex update` on the L16, then reboot.
5. Two-line check on the L16 afterwards: `stat -c %C /etc/.pwd.lock` wants
   `passwd_file_t`, and
   `systemd-run --wait --collect --property=DynamicUser=yes --unit=du /usr/bin/id`
   must say `result: success`.

Two things Andre will hit on that path and should expect rather than debug:

* **The `main` build will rebuild `core`,** because paths-filter diffs against a
  `main` that is 137 commits behind. His `apex update` is a ~6 GB pull, not tens
  of MiB. `docs/update-cost.md` records the tension.
* **`apex-sysext-rebuild.service` will rebuild the extension on the L16's first
  boot** if that machine carries one: its `state.json` predates
  `pkg_compat_level`, and `// 0` never equals 3. That is the unattended path
  measured clean in evidence §2.6 — it re-downloads over the network and takes
  longer than katana's cached 54 s. Let it finish; it is not a hang. And reboot
  cleanly, or the staged update is discarded.

Open, and none of it belongs to this unit:

* **The `pahole < 1.26` kernel BTF.** No sched-ext scheduler can load on any
  APEX image until the kernel's BTF is regenerated. Needs its own item.
* **`restorecon` on stdin**, the hardening above. One line, with the measurement
  already in the evidence.
* **`install_etc` never relabels a directory it did not create** — the right
  rule, with a consequence: a directory created by a run whose relabel failed is
  never self-healed by a later install. Seen on the three cron directories the
  broken run of §2.4 left as `etc_t`.
* **P0-001's remaining acceptance rows**, unchanged: fresh install, the rollback
  reboot (now finally meaningful — the rollback slot holds a genuinely different
  image), per-compositor workflow passes, multi-monitor. **Secure Boot is still
  disabled on katana.**
* **§6.6** (a Gaming Mode session destroyed without cooperation) was not re-run;
  it needs a real session and a greetd edit.
* Row C's other half — the named "game mode STOPS a scheduler it found rather
  than restoring it" limitation — is unreachable until a scheduler can attach.

**Memory vault was unreachable the whole session** (`claude-memory` MCP,
CONNECT_TIMEOUT), so no journal entry was appended and no note was written to
it. Three things belong there when it is back: the `pahole < 1.26` BTF finding,
the `setfiles_t` / `initrc_tmp_t` domain sensitivity, and the third NVMe swap.
