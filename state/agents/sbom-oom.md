# sbom-oom — the only thing between roadmap/v2.2 and a green build-image

items: none (a guard, like `build-verify` was) — but it gates unit `final`
repo: apex-os
worktree: **make your own** — `/var/tmp/apex-work/wt-sbom-oom`, off
`roadmap/v2.2` (now `2e04fbcb`), branch `task/sbom-oom`
scratch: `/var/tmp/apex-work/scratch-sbom-oom/` (per-agent)

## What is settled, measured by the round-33 `build-verify` agent

CI **35433705393** on `roadmap/v2.2` @ `7f647470`:

| job | result |
|---|---|
| `rust`, `changes`, `core`, `base` | **success** |
| `image` | **failure**, at *Generate and attest the SBOM* |
| `qcow2`, `installer-iso` | skipped |

**Everything APEX passed inside `image`** — build, the Secure Boot chain for the
kernel AND the modules, payload split, push, cosign signing. The per-SHA tag is
live: `ghcr.io/andrenijman/apex-os:apex-7f647470…` → `sha256:be3bdd0c6384…`.
Steps after the SBOM are `skipped`, so the promote/tag-consistency assertions
are **unproved, not red**.

The failure, read out of the log:

```
line 64: 25574 Killed  SYFT_PARALLELISM=4 timeout 2700 syft "registry:$REF" …
##[error]syft exited 137 after 209s
```

**137 is SIGKILL.** Not 124 — the `timeout` arm still had 2,491 seconds left.
Not 143. syft was cataloguing a ~13 GB image with ~14.9 GB of memory free.

**The roadmap's own record of this was WRONG and has been corrected.** Round
33's orchestrator read run 35415266422's `The runner has received a shutdown
signal` as GitHub infrastructure and wrote that into `dispatch.json` and into
the `build-verify` card. It is not: `bd0c41ce` carried the identical wrapper, so
the earlier run had the same inputs and lost the whole VM — the same exhaustion
at a worse severity. **Every input here is APEX's.**

## The thing nobody has noticed yet, and it is the shape of the fix

Read the step's own comment block (`build-image.yml` ~1383-1428). It records a
history of this step dying, and `SYFT_PARALLELISM=4` was added deliberately
because the default is one worker and the image gained two Electron trees.

But look at what changed with it. **At parallelism 1 syft ran the FULL fifteen
minutes and was killed by the timeout** — which is why the timeout was raised to
45. **At parallelism 4 it dies in 209 seconds.** The raise did not fix a slow
step; it traded a timeout for a much faster memory death. That is the hypothesis
to test first, and it is cheap to test.

## Calibration — one claim is inferred, not measured

*OOM killer* specifically is inferred from the SHAPE (SIGKILL, large image,
bounded memory), **not read from a kernel message**. Confirm it or refute it
before fixing on it: a GitHub runner will let you `sudo dmesg` in the same step,
and a `free -m` sampled every few seconds alongside syft turns a guess into a
measurement. If it turns out not to be memory, the whole fix changes — say so
rather than making the numbers fit.

## NEXT

1. **Confirm the cause on the runner, in one cheap run**, before changing the
   fix: sample `free -m` beside syft and dump `sudo dmesg | tail` in the failure
   arm. A step that dies saying *which* resource ran out is worth more than one
   that is merely fixed.
2. **Then fix it.** Candidate directions, in the order they look right — argue
   your choice on this card rather than taking the first:
   - `SYFT_PARALLELISM=2` or back to `1`, keeping the 45-minute timeout that was
     already sized for the slow path.
   - **Give syft a disk-backed source instead of `registry:`.** The runner has
     ~86 GB free after the prune and ~15 GB of memory: `skopeo copy` to a local
     OCI layout and catalogue that, so the layers live on disk rather than in
     syft's cache. This is the direction that actually matches the constraint.
   - Do **NOT** restrict the cataloguers. The step's comment already argues this
     and it is right: the two Electron trees are precisely where a CVE question
     lands, and an RPM-only SBOM would answer with silence while looking
     authoritative.
3. **Fix the error text while you are there.** `build-image.yml:1436` prints
   `(143 = killed)` and the code actually observed is **137**. 143 is SIGTERM,
   137 is SIGKILL — "asked to stop" versus "the kernel killed it", which is the
   whole diagnosis. Print the signal name.
4. **Verify with one dispatched build**, `gh workflow run build-image.yml --ref
   roadmap/v2.2`. It is ~1 hour and a non-main dispatch moves no tag anything
   tracks (`PUBLISH` guard at `build-image.yml:170`). Watch it to completion.

## The deadline, and what to do about it

**The orchestrator runs under `timeout 4h` from 16:58 AWST, so everything stops
~20:58.** One verification build may not fit. Commit and push the diagnosis and
the fix as you go; if the build is still running when you are killed, the branch
and this card carry it and the next round watches the run. **A correct diagnosis
with an unverified fix is a good outcome; an unrecorded one is not.**

## Two tooling facts from the agent that just finished here

- The Bash tool's 600 s cap **silently backgrounds** a longer command. What
  worked for watching CI was `Monitor` at 3600000 ms with a bounded
  `until … completed` loop.
- For the build itself: a **detached** `systemd-run --user` unit (no `--pty`, no
  `--wait`) holding a `sleep:idle` inhibitor. A backgrounded podman is SIGTERMed,
  truncates its log and still exits 0.

## Rules

- Headless only. Never open a window on Andre's desktop. Never run `qs -p`.
- No polkit or keyring prompts: `sudo` or `--user`.
- Never `pkill apex-agentd`. Do not touch katana or the phone — other agents own
  both.
- Never push `main`; never open a PR. Push only `task/sbom-oom`.
- A Containerfile assertion that cannot pass has cost this repo five days of
  image builds; anything you assert must be run, not reasoned about.
- **Write this card as you go, never at the end.** `NEXT` is load-bearing.

## DONE

- nothing yet.

## IN PROGRESS

- nothing yet.

## FOUND

- (build-verify, round 33) `check-shellcheck-coverage.sh` discovers `tests/`,
  `files/` and `android/tools/` only — **repo-root scripts are linted by
  nobody**, the same hole as its own header one directory up. Not this unit's to
  close, but it is real and it is written down here so it is not lost.
- (build-verify, round 33) the niri `Error:` at base STEP 139 is an intentional
  negative control, not a failure. Do not misread it in the log.

## BLOCKED ON

- nothing.
