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

### Probe round 2 (run 35447488777) — SWAP IS NOT WORTH ADDING

`gml10-swap32` — GOMEMLIMIT=10GiB plus a 32 GB swapfile at `/swapfile.probe`
(total swap 35 GB) — **rc=0, 698 s, peak RSS 14.69 GiB, 9830 packages.** The
`/swapfile.probe` path fixed round 1's `Text file busy`, so the arm genuinely
ran this time.

Compare it with round 1's plain `gomemlimit` arm, and the conclusion is the
opposite of what was expected:

| | `gomemlimit` (3 GB swap) | `gml10-swap32` (35 GB swap) |
|---|---|---|
| peak RSS | 14.65 GiB | 14.69 GiB |
| `mem_avail` floor | 188 MB | **204 MB** |
| peak `swap_used` | 973 MB | **1010 MB** |
| elapsed | 904 s | 698 s |

**Adding 32 GB of swap bought 16 MB of headroom.** Only ~1 GB of it was ever
touched — the same ~1 GB the stock 3 GB swapfile already absorbed in round 1.
The stock swap was never close to exhausted under GOMEMLIMIT, so the extra
32 GB is insurance against a spike that does not happen while the limit is set.

**Decision: do NOT add a swapfile to the SBOM step.** It is disk, a `sudo`
mkswap/swapon, and extra step surface for no measured margin. If this ever does
need more headroom, the lever to reach for is a larger runner, not swap. (The
elapsed difference is not evidence of anything — these are separate VMs with
different registry-fetch luck.)

### AND THE LIMIT'S VALUE IS NOT A LEVER EITHER — do not tune it

`gml4` — GOMEMLIMIT=**4**GiB — **rc=0, 765 s, peak RSS 14.80 GiB**, 9830
packages, `mem_avail` floor 326 MB. Across every successful arm:

Every arm that has ever succeeded, across both rounds:

| GOMEMLIMIT | source | peak RSS | elapsed | `mem_avail` floor |
|---|---|---|---|---|
| 4 GiB | `registry:` | **14.80 GiB** | 765 s | 326 MB |
| 6 GiB | `registry:` | **14.80 GiB** | 831 s | 117 MB |
| 10 GiB | `registry:` | 14.65 GiB | 904 s | 188 MB |
| 10 GiB + 32 GB swap | `registry:` | 14.69 GiB | 698 s | 204 MB |
| 10 GiB | `oci-dir:` | 14.72 GiB | 735 s | 125 MB |

**Cutting the limit from 10 GiB to 4 GiB did not lower peak RSS — it raised it
slightly.** Every successful arm lands at 14.65–14.80 GiB, ~92% of the box,
whatever the number says. So the "overshoot" is not heap the limit can reclaim,
and **the value is nearly irrelevant: what matters is that GOMEMLIMIT is set at
all.** Setting it appears to keep the Go GC continuously active instead of
letting GOGC=100 double the heap into the wall.

Practical consequence, and the reason this is written down: **if this step fails
again, lowering the number will not help.** Five arms say so.

**And read the margin honestly: it is inside the noise.** The `mem_avail` floors
of five passing arms are 117, 125, 188, 204 and 326 MB — a ~200 MB spread on a
~200 MB margin, i.e. a confidence interval that includes zero. Nothing measured
beats this configuration, so it is the right thing to land; but **the
`mem_avail` floor printed by build `35447611644` is the number that decides
whether this holds or whether probe 3 is needed.** Read it.

A note on the obvious escape hatch: **a larger runner may not be available
here.** `AndreNijman/apex-os` is a public repo owned by a **User**, not an
organisation; GitHub's larger runners are a paid, per-account feature and
nothing in this repo uses one (every job is `ubuntu-24.04`). Do not plan around
it without checking it exists. The smaller-predicate route below is the lever
that is definitely available.

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
  `workflow_dispatch` on `task/sbom-oom`, commit **`a40cf827`**), started
  14:04 UTC. **It verifies the GOMEMLIMIT-only fix.** If a later commit lands on
  this branch (e.g. the swap upgrade), this run does **not** cover it — dispatch
  another. Checked: `build-image.yml` is `concurrency: apex-image-publish` with
  **`cancel-in-progress: false`**, so a second dispatch QUEUES behind this one
  rather than killing it. Dispatching again is safe; it just waits.
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

1. **Verification build `35447611644`.**

   **What proves THIS unit's fix is not a green step.** It is these two lines in
   the *Generate and attest the SBOM* step:

   ```
   syft catalogued the image in <N>s
   	Maximum resident set size (kbytes): <N>
   ```

   If those appear, syft survived and the diagnosis below is confirmed in a real
   build — regardless of what the step does afterwards. Read the `[sbom sample]`
   lines for the `mem_avail` floor; that is the margin in CI, which is tighter
   than the probe's because the runner is also holding podman storage.
   A green step is a stronger result and a **different** unit's success; see the
   `cosign attest` warning below, which is the likely next failure.

   If syft survived, `task/sbom-oom` is ready to land on `roadmap/v2.2`.
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
     what drives the RSS this card is about, so the two problems **may** share a
     cause. They do not yet share a measured fix, and the distinction matters:
     - The concretely measured remedy in the bluefin PR is **`jq`
       post-processing** to strip the file/relationship arrays out of the
       finished SBOM. That shrinks the predicate and does **nothing** for RSS,
       because syft has already built the structure by then.
     - Cutting the work at source — the file-metadata / file-cataloguer knobs —
       would plausibly help both. **UNVERIFIED: the exact env var name
       (`SYFT_FILE_METADATA_SELECTION`) came out of a web search summary, not
       out of syft's own config. Read `syft config` / the 1.52.0 docs before
       relying on it.** Either way it drops file-level detail, not packages, so
       it is not the "restrict the cataloguers" trade the comment block rightly
       refuses. Worth a probe arm of its own.

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
