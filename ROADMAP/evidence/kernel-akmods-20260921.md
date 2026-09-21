# kernel-akmods — the akmods stage, 2026-09-21

Unit: `kernel-akmods`. Rounds 37-39. Branch `task/kernel-akmods`, landed onto
`roadmap/v2.2` as merge `259d8976`.

The unit was dispatched because every `core` image build died at
`akmods --force --kernels … --kmod nvidia` and **the build log contained no
compiler error**, so nobody could say why. Until `core` built, no image
shipped.

## The runs

| | RED | GREEN |
|---|---|---|
| unit | `kernel-build-2-core` | `kernel-akmods-core` |
| tree | `76aa2b95` | `0cc68901` |
| started | 2026-09-21 10:30:46 AWST | 2026-09-21 17:27:20 AWST |
| ended | 10:46:27, `status=1/FAILURE` | 18:06:54, `Result=success`, `EXIT_CODE=0` |
| wall | 15 min 41 s | 39 min 34 s |
| log | `/var/lab-scratch/kernel-build-2/core-build.log` (1815 lines) | `/var/lab-scratch/kernel-akmods/core-build.log` (6853 lines) + `core-build.err.log` (stderr only) |
| result | died mid-akmods | `localhost/apex-os-core:latest` = `afe03dcdf173`, 14 out-of-tree modules signed |

### The red run's last bytes, read with `cat -A`

```
Checking·kmods·exist·for·7.2.6-cachyos1.apex1.fc43.x86_64·[··OK··]␍␊
Building·and·installing·nvidia-kmodEXIT=1␊
```

akmods wrote its status *label* and never wrote the `[··OK··]`/`[FAILED]` that
always follows it on that same line. `grep -c Error` over the whole red log is
**0** — podman never printed its own `Error: building at STEP …` either.

### The green run's same lines

```
=== akmods: building nvidia for 7.2.6-cachyos1.apex1.fc43.x86_64 ===
+ akmods_rc=0
+ akmods --force --kernels 7.2.6-cachyos1.apex1.fc43.x86_64 --kmod nvidia
Checking kmods exist for 7.2.6-cachyos1.apex1.fc43.x86_64 [  OK  ]
Building and installing nvidia-kmod [  OK  ]
+ '[' 0 -ne 0 ']'
+ RPM=/var/cache/akmods/nvidia/kmod-nvidia-7.2.6-cachyos1.apex1.fc43.x86_64-580.178.04-1.fc43.x86_64.rpm
built: /var/cache/akmods/nvidia/kmod-nvidia-7.2.6-cachyos1.apex1.fc43.x86_64-580.178.04-1.fc43.x86_64.rpm
```

`xone` (`1000.0.0.git.1442.85e53359-2.fc43`) and `xpadneo` (`0.10.2-1.fc43`)
built and installed the same way. All three are in the 14 signed modules.

## THE VERDICT: (b), and the distinction matters

**The fix did NOT change this outcome. It made the failure readable. Those are
different claims and only the second one is proven.**

A green build after a change is not evidence the change caused the green. Here
the causal question is not merely unresolved — for this hunk it is *decidable*,
and the answer is that the fix **cannot** have flipped the result:

1. The change is confined to the `akmods_rc != 0` branch. The invocation is
   byte-identical before and after —
   `akmods --force --kernels "${KVER}" --kmod "${k}"` — with only the handling
   of a non-zero return altered. Nothing about akmods' argv, environment,
   inputs or working directory moved.
2. The green log proves the branch was evaluated and skipped: `+ '[' 0 -ne 0 ']'`
   appears, and nothing from the dump does. `FATAL: akmods` occurs exactly once
   in the whole 6853-line log, on line 1071, which is podman's echo of the RUN
   body at `STEP 19/69` — not an execution of it.
3. So on the success path the added statements are `akmods_rc=0` and a false
   test. Two no-ops. A build that failed with the old shape would have failed
   with the new one too — it would merely have *said why*.

**akmods itself returned 0 in the green run and never reported a status in the
red one.** The inputs to that RUN were identical across both:

- Same kernel image: `localhost/apex-kernel:local`, id `a889708fcfd2…`, created
  **2026-09-20 14:13:06 UTC** — before *both* runs, and unchanged since.
- Same driver: `akmod-nvidia 3:580.178.04-1.fc43` from rpmfusion-nonfree-updates
  in both logs.
- Same kernel version `7.2.6-cachyos1.apex1.fc43.x86_64`, same
  `--isolation=chroot`, same `sudo podman build` from `build-local.sh`.
- The only `Containerfile.core` change inside that RUN between the two trees is
  this hunk (42 commits separate the trees; the other Containerfile.core change
  is kernel-publish relocating `ARG APEX_KERNEL_IMAGE` above the first FROM,
  which `build-local.sh` overrides identically in both runs).

Identical inputs, opposite outcomes. **The 10:46 failure was therefore not
deterministic and not a property of the driver/kernel pair.** Round 38's
reproducer independently agrees: built `FROM b45ec0aeb90a` — the red build's
*own* pre-akmods layer — it produced
`kmod-nvidia-7.2.6-cachyos1.apex1.fc43.x86_64-580.178.04-1.fc43` in 96 s,
`rc=0`.

### What is still NOT known

The actual cause of the 10:46 death. Ruled out by evidence, not by argument:
the driver/kernel source pair (reproducer), missing kernel-devel (reproducer),
a wrong RPM filename (route 3 — the glob matched fine in the green run), OOM /
kill / segfault / thermal / suspend (journal of boot `833dc05b`, the correct
boot — round 38 checked `-b -1`, which began at 14:55 and could not contain a
10:46 event, so that check was vacuous and has been redone).

The surviving lead, recorded as correlation and nothing more: the machine was
not quiet during the red run. Other roadmap agents were driving heavy *root*
podman against the same `/var/lib/containers` — including a
`podman run --rm --privileged --pid=host -v /var/lib/containers:/var/lib/containers`
bootc loopback install at 10:31 — with a `umount` at 10:46:00 and
`apex-boot-windows --check` at 10:46:23, four seconds before the death. The
green run was deliberately the only heavy podman on the machine.

### What would tell them apart

Re-run tree `76aa2b95` with only this hunk cherry-picked onto it — so a failure
is readable — **while** reproducing the concurrency: a second privileged
`--pid=host` container sharing `/var/lib/containers`. Green under load falsifies
the concurrency theory; red now prints the cause instead of hiding it. That
experiment is worth doing only if the stage fails again; `core` builds today.

## Absence #3: what the split-stream capture did and did not settle

The green run wrote stdout and stderr to separate files on purpose, to test the
round-38 finding that a failing `podman build` printed no
`Error: building at STEP …`.

**Settled:** the *mechanism* is sound and the earlier reasoning about it was
right. `core-build.err.log` is 348 KB / 4859 lines of genuinely
stderr-only content, ending in its own `EXIT_CODE=0`. Had podman emitted that
line it would have landed there. Round 38's related worry — that the missing
line was a redirect — is also disproven directly: `build_core()` in
`build-local.sh` invokes a bare `sudo podman build` with no pipe, no `$(…)`
capture and no per-stream redirect, so both fds inherit the wrapper's log.
(The only two `Error` hits in the stderr file are the package `perl-Error`.)

**Not settled, and honestly unanswerable from a green build:** whether a
*failing* podman build in this configuration omits that line. A successful
build has no `Error: building at STEP` to print, so its absence here is
expected and carries no information. **The question cannot be answered without
reproducing the failure.** The instrument is now in place to answer it the
moment the stage goes red again — which is the most this run could establish.

## What did land, and is proven both ways

`files/scripts/check-run-recovery-reachable` + `tests/test-run-recovery-reachable.sh`,
wired into `build-image.yml` and `pr-validation.yml`.

The defect class: under `set -e`, an unguarded call to a tool that logs to a
*file* rather than to stdout (`akmods`, `akmodsbuild`, `dkms`, `rpmbuild`)
terminates the RUN before any recovery block can read that file. The dump that
`Containerfile.core` had written was unreachable by construction — reachable
only on the path where akmods *succeeded* and produced a surprising filename.

Both directions demonstrated, not asserted:

- **RED:** on the tree as inherited the harness was **26 passed / 2 failed**,
  and both failures were the live defect, the checker naming
  `Containerfile.core:593` and quoting the `sed` it proves is dead code.
- **GREEN:** with the fix, **28 passed / 0 failed** and the checker exits 0 on
  all five Containerfiles.
- The harness itself fails both ways by construction: a checker that always
  exits 1 fails its GREEN cases (including `akmods` appearing as a *path* in a
  `sed` argument, which a naive `grep -q akmods` would flag); one that always
  exits 0 fails its RED cases, including the exact shape that shipped.
- At image-build scale: the green run shows the guard evaluated and skipped,
  so the fix is inert on success. A local simulation of the old and new loop
  bodies shows the old printing only the truncated status line and the new
  printing the compiler error.
- Survived `task/initramfs-slim` rewriting 167 lines of `Containerfile.core`:
  re-verified 28/0 on the merge result.

## Bottom line

`core` builds. The image blocker is cleared and the akmods stage is green with
nvidia, xone and xpadneo all built and signed. The landed change is a
**durability improvement** that did not cause this pass, and the root cause of
the 10:46 failure remains formally open — most consistent with a transient
environmental fault, since identical inputs produced opposite outcomes. If it
recurs, the log will say why instead of stopping mid-word.
