# sbom-oom — the only thing between roadmap/v2.2 and a green build-image

items: none (a guard, like `build-verify` was) — but it gates unit `final`
repo: apex-os
worktree: `/var/tmp/apex-work/wt-sbom-oom`, branch `task/sbom-oom` off
`roadmap/v2.2` (`2e04fbcb`)
scratch: `/var/tmp/apex-work/scratch-sbom-oom/` (per-agent)

## THE ANSWER, in one line

**`GOMEMLIMIT` is the whole fix.** Every probe arm without it died of memory
exhaustion; both arms with it produced an identical, complete SBOM. Source
location and parallelism changed nothing. Measured over seven arms, below.

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

## FOUND — probe run 35437908023, seven arms, read 2026-09-19

All seven ran the same syft **1.52.0** against the same digest
`sha256:be3bdd0c6384…` on the same `ubuntu-24.04` runner class (16 GB RAM,
3 GB swap, 86 GB disk free). One variable separates every success from every
failure.

| arm | GOMEMLIMIT | source | par | result |
|---|---|---|---|---|
| `repro` | unset | `registry:` | 4 | **died** — 15866M used / 123M avail / swap 3071M full |
| `registry-par1` | unset | `registry:` | 1 | **died** — 15918M used / 70M avail / swap 3065M |
| `ocidir` | unset | `oci-dir:` | 4 | **died** — 15967M used / 21M avail / swap 3071M full |
| `ocidir-decompressed` | unset | `oci-dir:` (`--dest-decompress`) | 4 | **died** — 15832M used / swap 3059M; rc 143 |
| `gomemlimit` | **10GiB** | `registry:` | 4 | **rc=0**, 904 s, peak RSS **14.65 GiB**, 9830 packages |
| `ocidir-gomemlimit` | **10GiB** | `oci-dir:` | 4 | **rc=0**, 735 s, peak RSS **14.72 GiB**, 9830 packages |
| `swap32` | — | — | — | **never ran syft** — probe bug, see below |

Both surviving arms emitted a 166 MB spdx-json with **9830 packages** and both
Electron trees present (`@anthropic-ai/claude-code`,
`@anthropic-ai/claude-code-linux-x64`, `chatgpt`, `electron`). The SBOM is
complete; nothing was restricted to get it.

### Three claims this measurement kills

1. **The source is not the problem.** `ocidir` (layers already on local disk,
   an OCI layout, no registry read during the catalogue) died exactly like
   `registry:`. `ocidir-decompressed` — the zstd hedge — died too. The card's
   earlier preferred fix, moving to a disk-backed layout, addresses something
   that is not the cause. It is 170 s faster and that is all it buys.
2. **Parallelism is not the problem.** `registry-par1` died the same way at one
   worker. So the history in the step's comment block — *"at parallelism 1 syft
   ran the full fifteen minutes and was killed by the timeout"* — was **also a
   memory death**, not a slow catalogue. Raising the timeout to 45 minutes never
   addressed anything; lowering parallelism would not either.
3. **It is the live data structure, not the transport.** In `gomemlimit`, `/tmp`
   stops growing at 14715M at 10:43:16 — every byte is already on disk — and
   memory then spikes another 4 GB to 15.8 GB before settling to a flat ~11.2 GB
   for the remaining nine minutes. The spike is the filetree squash / MIME pass
   over the file count, matching anchore/syft#2159. A full Fedora bootc plus two
   Electron trees is a very large file count.

### The OOM killer: still not confirmed, and now less likely

`vmstat oom_kill` was **0 before and after** in every arm that lived to read it,
and `dmesg`/`journalctl` were clean in both survivors (`NO OOM LINES IN DMESG`,
`NO OOM LINES IN JOURNAL`). The four dying arms did **not** get a SIGKILL — they
drove the VM into swap-thrash until the host stopped getting a heartbeat and
cancelled the job (`The runner has received a shutdown signal`).

So the two manifestations are:

- **probe VMs**: livelock → host cancels → `shutdown signal`. No OOM kill.
- **CI run 35433705393**: bash reported `25574 Killed` / exit 137, a real
  SIGKILL of a named pid — the kernel OOM killer winning the race the probe VMs
  lost.

Same cause, two severities. **Do not upgrade "the OOM killer fired" to a fact
without a kernel message.** The instrumentation landed in `build-image.yml`
reads `oom_kill` and dumps dmesg and the journal on failure, so the next CI
death settles it either way.

### The margin is thin, and the belt was never tested

Both survivors peaked at **14.65–14.72 GiB RSS on a 16 GB box**, with
`mem_avail` bottoming at **188 MB** and **125 MB** respectively. `GOMEMLIMIT` is
a *soft* limit: syft blew through the 10 GiB setting by ~4.7 GiB. Two successes
at ~1% headroom is a fix for today's image and a flake the moment the image
gains packages.

The hedge for exactly this — `swap32` — **never ran**. It died in 0 s on
`fallocate: fallocate failed: Text file busy`: it tried to `fallocate -l 32G`
the runner's **already-active** `/swapfile`. That is a probe bug, not a result
about swap. Probe 2 fixes it by allocating a *new* file at `/swapfile.probe`.

### Two defects found in the fix that was waiting here

- The predecessor's uncommitted `build-image.yml` draft referenced
  **`$SYFT_GOMEMLIMIT` and `$SYFT_SOURCE`, neither of which is defined anywhere
  in the file**. Under the step's `set -euo pipefail` that is an unbound-variable
  abort on the syft line. It was an unfinished edit, not a working fix. Both are
  now inlined.
- **syft was unpinned.** The step installs from `install.sh` on `main` — i.e.
  whatever is latest — while the fix is measured against 1.52.0. Now pinned,
  with the re-measure note in the comment.

### Carried forward from build-verify, round 33 (not this unit's to fix)

- `check-shellcheck-coverage.sh` discovers `tests/`, `files/` and
  `android/tools/` only — **repo-root scripts are linted by nobody**.
- The niri `Error:` at base STEP 139 is an intentional negative control. Do not
  misread it in the log.

## DONE

- Worktree on `task/sbom-oom` off `2e04fbcb`.
- Read the failing log of 35433705393 directly: after the prune the runner had
  **86 GB disk free** and **14915 MB memory available**; syft 1.52.0 was
  **Killed** 209 s in. Disk cannot be it.
- Measured the image: **113 layers, 7.26 GB compressed**, largest layer 832 MB,
  every layer `application/vnd.oci.image.layer.v1.tar+zstd`.
- Built `.github/workflows/sbom-probe.yml` and ran it — run **35437908023**,
  seven arms, all read. Table above.
- Confirmed `build-image.yml` has `workflow_dispatch` and that a dispatch from a
  non-main ref is **publish-guarded** (it builds, signs and verifies without
  moving `:apex`, `:daily`, `:gaming-*`, `:core` or `:base`). The verification
  path this card assumes therefore exists and is safe from `task/sbom-oom`.

## IN PROGRESS — run ids, read these first

- **Probe round 2: run `35447488777`** (workflow `sbom-probe`, branch
  `task/sbom-oom`, commit `531b6c96`), started 14:01 UTC. Four arms about the
  **margin**, not the cause: `gml10-swap32` (the candidate — the working limit
  plus 32 GB of real swap at `/swapfile.probe`, which is the round-1 `swap32`
  bug fixed), `swap32-only` (control: is the limit needed once swap exists?),
  `gml6` and `gml4` (does a lower limit move peak RSS, or is the ~4.7 GiB
  overshoot non-heap and immovable?). Expect ~15 min per arm, all parallel.
  Read it with
  `gh api repos/AndreNijman/apex-os/actions/jobs/<id>/logs` per job id from
  `gh run view 35447488777 --json jobs`.
- **Verification build: run `35447611644`** (workflow `build-image`,
  `workflow_dispatch` on `task/sbom-oom`, commit `a40cf827`), started 14:04 UTC.
  ~1 hour. This is the one that matters. **A skipped job counts as success in
  this repo** — do not read a green `image` job as proof. Confirm the step
  *Generate and attest the SBOM* actually ran and printed
  `syft catalogued the image in …s` plus a `Maximum resident set size` line:

  ```
  gh run view 35447611644 --repo AndreNijman/apex-os --json jobs \
    -q '.jobs[] | "\(.name) \(.conclusion)"'
  gh run view --job <image job id> --log | grep -E 'catalogued|Maximum resident|SBOM packages|sbom sample|::error'
  ```

  A dispatch from a non-main ref is publish-guarded: it builds, signs and
  verifies without moving `:apex`, `:daily`, `:gaming-*`, `:core` or `:base`.

## NEXT

**The fix is committed and pushed — `a40cf827` on `task/sbom-oom`. Nothing here
needs re-deriving.** Two runs are in flight; read them in this order.

1. **Verification build `35447611644`.** If the SBOM step ran and passed, this
   unit is done: say so, and `task/sbom-oom` is ready to land on `roadmap/v2.2`.
   It carries four commits over `2e04fbcb` — `0386666a`, `87aa264e`, `531b6c96`
   (the probe, built up over two rounds) and `a40cf827` (the fix). **Delete
   `.github/workflows/sbom-probe.yml` before or as part of the landing**: it is
   a temporary measurement, its header says so, and it triggers on pushes to
   `task/sbom-oom` only, so it is inert elsewhere but should not outlive the
   question it answered.
   If the SBOM step failed again, the step now prints exactly what ran out —
   `oom_kill` delta, dmesg, the journal, peak RSS and the named signal. Read
   those before changing anything. Do **not** respond by lowering GOMEMLIMIT or
   restricting cataloguers; see the arms below.

   **Watch for a NEW failure mode one line further on, which nothing has ever
   reached — and it is likely.** The SBOM syft produces is **166 MB** of
   spdx-json. The next statement after the package-count assertion is
   `cosign attest --yes --predicate /tmp/sbom.spdx.json --type spdxjson`, and no
   build has ever got that far: every previous run died inside syft.

   Researched rather than guessed, 2026-09-19: the **public Rekor instance has
   an undocumented request-body size limit** and returns **HTTP 413** above it
   (sigstore/rekor#2808); `cosign attest` uploads the *entire* predicate to the
   transparency log, not just a digest of it (sigstore/cosign#3599). For scale,
   GitHub's own `actions/attest` hardcodes a 16 MB predicate cap
   (projectbluefin/actions#484). **APEX's predicate is 166 MB — an order of
   magnitude over the nearest documented ceiling.** Another bootc desktop
   project hit exactly this and made the SBOM attestation best-effort
   (daytwo-bootc-workstation-base#19).

   If the step fails at `cosign attest` rather than at syft, **that is progress,
   not a regression of this fix.** Do not undo the GOMEMLIMIT change in response
   to it. The remedies are a different problem from this unit's:
   - `cosign attest --no-upload` (keep the attestation on the registry, skip
     the tlog) — but check what `trust.rs` and
     `tests/test-apex-trust-enforcement.sh` require before doing it, because the
     verify step immediately below runs `cosign verify-attestation`.
   - **Shrink the predicate**, which is the better lead and likely helps the
     memory too. That 166 MB is not 9830 packages' worth of metadata: syft
     emits package-to-file relationships by default, which on an image with
     ~200k files produces 100k+ relationship edges. The same file-level work is
     what drives the RSS this card is about. `SYFT_FILE_METADATA_SELECTION` /
     the file cataloguer knobs cut the relationship explosion **without dropping
     a single package**, so it is not the "restrict the cataloguers" trade the
     comment block rightly refuses. Worth a probe arm of its own.

2. **Probe round 2 `35447488777`** tells you what to do if the margin turns out
   too thin. Expected readings:
   - `gml10-swap32` passing with GBs of headroom → add the swapfile to the SBOM
     step as a second commit. This is the likely upgrade.
   - `swap32-only` passing → swap alone is sufficient and GOMEMLIMIT is belt
     over braces; simplify if you like, but the limit costs nothing.
   - `swap32-only` dying → GOMEMLIMIT is load-bearing and swap is only margin.
   - `gml6`/`gml4` peaking at ~14.7 GiB anyway → **a lower limit is not a
     lever**; record that so nobody tries it. If they peak lower, a tighter
     limit is the cheaper margin than swap.

3. If both look good and the build is green, update `dispatch.json` /
   roadmap status for this unit and unblock unit `final`, which this gates.

**Do not re-run the round-1 arms.** `repro`, `ocidir`, `ocidir-decompressed`
and `registry-par1` have their answer and it is in the table above.

## The deadline, and what to do about it

**A correct diagnosis with an unverified fix is a good outcome; an unrecorded
one is not.** Commit and push the diagnosis and the fix as you go; if a build is
still running when you are killed, the branch and this card carry it and the
next round watches the run.

## Tooling facts from the three agents that have been here

- The Bash tool's 600 s cap **silently backgrounds** a longer command. For
  watching CI use `Monitor` at 3600000 ms with a bounded `until … completed`
  loop.
- `gh run view --log` **refuses while a run is in progress**;
  `gh api repos/AndreNijman/apex-os/actions/jobs/<id>/logs` does not. Get job
  ids from `gh run view <id> --json jobs`.
- For a local build: a **detached** `systemd-run --user` unit (no `--pty`, no
  `--wait`) holding a `sleep:idle` inhibitor. A backgrounded podman is SIGTERMed,
  truncates its log and still exits 0.
- A **skipped job counts as success** in this repo's workflows. Confirm a step
  ran, not that a job was green.

## Rules

- Headless only. Never open a window on Andre's desktop. Never run `qs -p`.
- No polkit or keyring prompts: `sudo` or `--user`.
- Never `pkill apex-agentd`. Do not touch katana or the phone — other agents own
  both.
- Never push `main`; never open a PR. Push only `task/sbom-oom`.
- `files/system/libexec/apex-pkg` is owned by another unit this round.
- A Containerfile assertion that cannot pass has cost this repo five days of
  image builds; anything you assert must be run, not reasoned about.
- **Write this card as you go, never at the end.** `NEXT` is load-bearing.

## BLOCKED ON

- nothing.
