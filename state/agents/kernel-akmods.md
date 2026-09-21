# kernel-akmods — nvidia akmod will not build against the APEX kernel

Dispatched round 37, 2026-09-21. Worktree `/var/tmp/apex-work/wt-kernel-akmods`,
branch `task/kernel-akmods`, cut from `origin/roadmap/v2.2`.

## WHY THIS UNIT EXISTS

`kernel-publish`'s card names this and explicitly disclaims it:

> `core` will probably still go red after my stage, in the akmods stage, and
> that is not this unit. [...] Somebody owns the akmods one; it is not
> kernel-publish and it is not kernel-build-2's card either.

Nobody owned it. Every `core` build dies here, roughly 35-40 minutes in, after
the whole desktop dnf transaction has already succeeded. Until this is fixed no
image ships, so nothing else on the roadmap reaches a machine.

## THE EVIDENCE YOU START FROM

`/var/lab-scratch/kernel-build-2/core-build.log`, line 1810 onward:

```
+ for k in ${APEX_AKMODS}
+ echo '=== akmods: building nvidia for 7.2.6-cachyos1.apex1.fc43.x86_64 ==='
+ akmods --force --kernels 7.2.6-cachyos1.apex1.fc43.x86_64 --kmod nvidia
Checking kmods exist for 7.2.6-cachyos1.apex1.fc43.x86_64 [  OK  ]
Building and installing nvidia-kmodEXIT=1
```

nvidia is `xorg-x11-drv-nvidia-kmodsrc 3:580.178.04-1.fc43` from
rpmfusion-nonfree-updates (log line 1153). The kernel is CachyOS
`7.2.6-cachyos1.apex1.fc43`.

`Containerfile.core` lines 602-616 are the akmods block. Line 612 is supposed to
dump `/var/cache/akmods/${k}/*.failed.log` on failure:

```
RPM="$(ls -1 /var/cache/akmods/${k}/kmod-${k}-*.rpm 2>/dev/null | head -1)"; \
[ -n "$RPM" ] || { ...; sed -n '1,80p' /var/cache/akmods/${k}/*.failed.log 2>/dev/null; exit 1; }
```

**That dump is not in the log.** Either the failed.log is not where the glob
looks, or `akmods` exited non-zero before writing one and `set -e` killed the
RUN before the guard ran. Establish which BEFORE theorising about the kmod
source — a build failure you cannot read is the first defect here, and it is
the reason this has cost two rounds already. See the memory note
"A gate that runs and inspects nothing": this is that family.

## WHAT DONE LOOKS LIKE

1. The real compiler error is captured and quoted, from the actual build, not
   inferred from version numbers.
2. `core` gets past the akmods stage. The bar is a real build, not a reasoned
   argument that it should now work.
3. Whatever fix you land is accompanied by a gate that fails BOTH ways: it must
   go red on a reintroduced defect and green on a clean tree. Prove both.

## ROUTES, RANKED — pick on evidence, not on this ordering

- **The failure is a genuine 580.178.04-vs-7.2.6 source incompatibility.**
  Then the options are a patch in the akmods build, a different nvidia stream
  (rpmfusion ships more than one; `-open` kmodsrc is a separate package), or
  pinning the kernel and driver to a pair that is known to compile. State the
  cost of each. Note that `p1-043-gpu-parity` and katana's RTX 3070 depend on
  nvidia actually working, so "drop nvidia" is not a route.
- **The failure is environmental** — missing kernel-devel for the CachyOS
  kernel, a `/usr/src/kernels/${KVER}` tree the kernel tier does not ship, or
  `akmods` looking at the running kernel rather than `--kernels`. This is the
  likeliest class given that the kernel tier is new and this is the first time
  core ever built against it.
- **The failure is the guard itself** and a real rpm was produced under a name
  the `ls` glob misses.

## BOUNDS

- Build in a container, on this machine or katana. Katana's runner has the GPU
  but you do NOT need a GPU to compile a kmod.
- Do not touch `Containerfile.kernel` — that is kernel-build-2/kernel-publish
  ground. If the fix belongs there, say so in this card and stop.
- Never `bootc install` without `--generic-image`, and always through
  `tests/lab/bootc-install-lab`. Two host NVRAM rewrites came from ignoring this.
- Do not land onto `roadmap/v2.2` yourself and do not push `main` or open a PR.
- Write this card as you go, not at the end.

## FINDINGS AS THEY LAND

### 2026-09-21 — why the failure log could not be read (confirmed by reading, not guessed)

The akmods RUN opens with `set -eux`. The loop body is:

```
akmods --force --kernels "${KVER}" --kmod "${k}"; \
RPM="$(ls -1 ...)"; \
test -n "${RPM}" && test -f "${RPM}" \
    || { echo "FATAL: ..."; sed -n '1,80p' /var/cache/akmods/${k}/*.failed.log ...; exit 1; }
```

`akmods` is an unguarded simple command. Under `set -e` its non-zero exit
terminates the shell **immediately** — the `RPM=` assignment, the `test`, and
therefore the `sed` that dumps `*.failed.log` never execute. The dump is
unreachable by construction on the only path that would ever want it. Same
family as the memory note "A gate that runs and inspects nothing" and
"errexit skips `!` commands".

That is silence #1, and it is real.

### 2026-09-21 — CORRECTION: the "second silence" as first written was wrong

An earlier version of this card said akmods "is redirecting its own output
somewhere". Read the script, it does not. `/usr/sbin/akmods` 0.6.2:

- `akmods_echo 1 2 --failure` calls `echo_failure`, which writes `[FAILED]` to
  **inherited stdout**.
- `akmods_echo 2 1 "Building rpms failed; see …failed.log for details"` writes
  to **inherited stderr**.
- The only thing akmods redirects is `akmodsbuild`'s own output, into
  `/var/cache/akmods/<kmod>/.last.log`.

And the container's stderr demonstrably reached `core-build.log`: the
`+ akmods --force …` lines in it are `set -x` traces, which bash writes to
fd 2. The wrapper was `{ ./build-local.sh core; ec=$?; } >> log 2>&1`
(journal, unit `kernel-build-2-core`), so both streams were captured.

So neither marker is missing because it was redirected. **Neither marker was
ever emitted.** Add the third absence nobody had noticed: a failed
`podman build` always prints
`Error: building at STEP "RUN …": while running runtime: exit status N`, and
that is not in the log either. The log's final line has no trailing newline.

Measured facts about that run, from `journalctl --user -u kernel-build-2-core`:
started 10:30:46, `Main process exited, code=exited, status=1/FAILURE` at
10:46:27 — **15 min 41 s, a clean exit 1, not a signal, not an OOM kill**. The
card's "35-40 minutes in" is wrong; akmods had been running about five minutes.

### 2026-09-21 — the reproducer is free: the pre-akmods layer is still in root's storage

No need to rebuild anything from `fedora-bootc:43`. The failing build's last
committed layer is **`b45ec0aeb90a`**, and `podman history` on it ends at
`ARG APEX_AKMODS …` — i.e. it is exactly the state entering the akmods RUN
(`[3/3] STEP 19/69` in `core-build.log` line 646). Kernel installed,
`/usr/src/kernels/${KVER}` present, RPMFusion not yet enabled.

`/var/lab-scratch/kernel-akmods/repro/Containerfile.repro` is
`FROM b45ec0aeb90a` + the dnf half as one cached RUN + the akmods call as a
second RUN, guarded with `rc=0; akmods … || rc=$?` and dumping `.last.log`,
`*.failed.log` and `/var/log/akmods/akmods.log`. Built with
`--isolation=chroot` to match the real build exactly. Also checked before
theorising: `kernel-cachyos-devel` does Provide
`kernel-devel-uname-r = 7.2.6-cachyos1.apex1.fc43.x86_64`, so the kmodtool
BuildRequires is satisfiable.

## NEXT

- Read the reproducer result out of `/var/lab-scratch/kernel-akmods/repro/repro.log`.

## BLOCKED ON

- nothing

---

## ROUND 38 CONTINUATION — written by the orchestrator, 2026-09-21 13:55 AWST

The round-37 agent died at the 12:06 shutdown. Everything above stands; this
is what the orchestrator established from disk before dispatching you.

### The reproducer PASSED — read that before anything else

`/var/lab-scratch/kernel-akmods/repro/repro.log` (154 KB, finished 12:06:04
AWST, `PODMAN_EXIT=0`) ends:

```
2026/09/21 04:05:25 akmodsbuild: Wrote: /tmp/akmodsbuild.wLJkZxvX/RPMS/x86_64/kmod-nvidia-7.2.6-cachyos1.apex1.fc43.x86_64-580.178.04-1.fc43.x86_64.rpm
2026/09/21 04:05:31 akmods: Successful.
REPRO: DONE rc=0
```

`akmodsbuild` ran 04:03:49Z → 04:05:25Z (96 s) and dnf installed the kmod.
Built from layer `b45ec0aeb90a` with `--isolation=chroot`, image kept as
`localhost/kakmods-repro:1` (`03b3161dc189`) in root storage. So nvidia
580.178.04 DOES compile against 7.2.6-cachyos1.apex1, kernel-devel IS
present, and the top-ranked route in "ROUTES, RANKED" is dead. Do not
change the driver stream or pin a different pair.

### So what killed the real build? Facts, not yet a verdict

- Unit `kernel-build-2-core` (systemd-run, user manager): started 10:30:46,
  `Main process exited, code=exited, status=1/FAILURE` at 10:46:27 (+08).
- Log ends at line 1815, `Building and installing nvidia-kmodEXIT=1`, NO
  trailing newline, no akmods `[ OK ]`/`[FAILED]` marker, no
  `Error: building at STEP …` from podman. Three absences.
- The orchestrator checked `journalctl -b -1 --since 10:40 --until 10:50`
  for suspend/hypridle/OOM entries: NOTHING. Idle suspend did not do this.
- The literal `EXIT=1` is printed by the wrapper — find which one (the
  systemd-run `bash -c` in `journalctl --user -u kernel-build-2-core`, or
  build-local.sh). Whatever prints it saw exit 1 from `sudo podman build`.
- A `podman build` that dies without its own `Error:` line is the
  discriminating fact. Check `journalctl -b -1 -k | grep -iE 'oom|killed|
  segfault'` and `journalctl -b -1 _COMM=podman` around 10:46 before
  theorising about akmods at all. Also list what differs between repro and
  the real RUN: the real one does dnf + akmods in ONE layer, has
  `test -d /usr/src/kernels/${KVER}` first, loops over `${APEX_AKMODS}`
  (nvidia was the first iteration), and ran ~5 min at akmods vs 96 s here.

### Your four dirty files are a real gate — commit them first

`git status` in the worktree: `M .github/workflows/build-image.yml` (+14),
`M .github/workflows/pr-validation.yml` (+27),
`?? files/scripts/check-run-recovery-reachable`,
`?? tests/test-run-recovery-reachable.sh`. That is the "recovery block
unreachable under set -e" checker and its mutation harness — the general
form of silence #1. They exist on disk and in refs/wip only. Run the
harness, prove both ways, commit as one commit, push. The branch
`task/kernel-akmods` now exists on origin (at 4031d43f, upstream set by the
orchestrator). The worktree is 6 commits behind `origin/roadmap/v2.2`
(now `71bc2177`: luks-boot + windows-installer-3 merges, no overlap) —
merge the tip before committing.

Then make the akmods RUN's failure path actually reachable in
`Containerfile.core` (~lines 596-616): `rc=0; akmods … || rc=$?` shape,
dump `.last.log` and `*.failed.log`, and let your new gate flag the old
shape. kernel-publish pins `Containerfile.core:93` this round — different
hunk, but merge their branch or the tip before you push if it has landed.

### Coordination

- `kernel-publish` dispatches a CI `core` build this round against the
  published kernel digest. Its card (`agents/kernel-publish.md`) carries
  the run id the minute it exists. That run reaches the akmods stage
  WITHOUT your fix; its result is your second data point. Cards only, no
  messaging.
- One heavy podman build at a time on this laptop. Battery was 58% and
  DISCHARGING at 13:42 — read `/sys/class/power_supply/BAT*/status` before
  anything longer than ten minutes. Long runs via `systemd-run --user`
  with a literal `EXIT_CODE=` line appended, never `nohup &`.

### NEXT (supersedes the NEXT above)

1. Run `tests/test-run-recovery-reachable.sh`; commit the four files; push.
2. Settle HOW the 10:46 build died (podman killed vs akmods exit 1) from
   the previous boot's journal and the wrapper. Write the verdict here.
3. Make the akmods failure path reachable in Containerfile.core; gate it.
4. Real `core` build (`build-local.sh core` under `systemd-run --user`,
   log in `/var/lab-scratch/kernel-akmods/`), quote the akmods outcome.
   `localhost/apex-kernel:local` was present in root storage at 10:30.

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
__BODY_

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
