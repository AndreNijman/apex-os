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

That is silence #1. There is a **second** silence: `akmods` prints its own
`[FAILED]` / "see ... for details" marker before returning 1 and that is not in
the log either, so akmods is redirecting its own output somewhere. Both have to
be answered; the reproducer is instrumented for both.

## NEXT

- Build the minimal reproducer in `/var/lab-scratch/kernel-akmods`:
  fedora-bootc:43 + APEX kernel RPMs + `akmods`/`akmod-nvidia` only, with the
  akmods call wrapped in `if ! ...` and every file under `/var/cache/akmods`
  enumerated and catted. Skips the 35-minute desktop transaction entirely.

## BLOCKED ON

- nothing
