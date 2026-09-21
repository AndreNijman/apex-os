# kernel-publish — the kernel tier is published, and `core` consumes it by digest

Unit: `kernel-publish` · branch `task/kernel-publish` · 2026-09-21

## What was broken

`Containerfile.core` consumes the kernel tier as `FROM ${APEX_KERNEL_IMAGE}`.
Nothing published an image for that line to name, and the ARG was declared where
it could not be seen. Two separate defects, one symptom:

```
Error: determining starting point for build: no FROM statement found
```

Run `35552604603` on `roadmap/v2.2` died that way after the `toolbuilder` stage.
So did every image build in the project. The message names neither the kernel
nor the ARG.

| defect | fixed by |
|---|---|
| `kernel-build.yml` built `localhost/apex-kernel:ci`, asserted the BTF verdict and **threw the image away** | `544143e1` — it now pushes |
| `ARG APEX_KERNEL_IMAGE` was declared inside the `toolbuilder` stage, where it is invisible to every `FROM` including its own | `3dc80629` (kernel-build-2's patch, carried) |
| `Containerfile.core` defaulted to `localhost/apex-kernel:local`, an image only a machine that has hand-built a kernel has | `72bab38d` |
| nothing could notice any of the three without a 45-minute build | `2cd11c6f` — `tests/check-kernel-image-pin.sh`, plus `build-image.yml` and `pr-validation.yml`; `b981e348` wired the third entry point, `build-local.sh`; `b505b747` closed the one seam mutation testing found |

## What is published

| | |
|---|---|
| immutable tag | `kernel-7.2.6-cachyos1.apex1.fc43.x86_64-544143e` |
| digest | `sha256:2ff544dd021478dbd36dab4efb1ce43ad9cc858f16cd07a271f01693822972d6` |
| floating `:kernel` moved | no — `task/kernel-publish` is not `main` or `roadmap/**` |
| producing run | [`35557283953`](https://github.com/AndreNijman/apex-os/actions/runs/35557283953) |
| kernel version | `7.2.6-cachyos1.apex1.fc43.x86_64` |

`Containerfile.core` now opens with

```dockerfile
ARG APEX_KERNEL_IMAGE=ghcr.io/andrenijman/apex-os@sha256:2ff544dd021478dbd36dab4efb1ce43ad9cc858f16cd07a271f01693822972d6
```

A **digest**, not the floating `:kernel` tag. The kernel is the most
security-sensitive thing in the image and `core` refuses one whose manifest does
not say `btf_scx=usable`; a name that can be repointed after that check is worth
much less than the bytes that were checked.

Read back independently of the run that made it, from this machine, with no
credentials at all:

```
$ skopeo inspect --no-creds --format '{{.Digest}}' docker://ghcr.io/andrenijman/apex-os@sha256:2ff544dd021478dbd36dab4efb1ce43ad9cc858f16cd07a271f01693822972d6
sha256:2ff544dd021478dbd36dab4efb1ce43ad9cc858f16cd07a271f01693822972d6
```

(Incidentally: `ghcr.io/andrenijman/apex-os` is **public**. Several comments in
`build-image.yml` say the packages are private and pass credentials on that
basis. Harmless, but stale.)

## Why ONE push and a registry copy, not two pushes

Two `podman push`es of the same local image can produce two different manifest
digests: compression happens at push time, so the digest only exists afterwards.
`build-image.yml` documents the same property at length, having measured it (0
blobs skipped, ever, pushing out of containers-storage). Two pushes would leave
two digests for one kernel and no answer to which one `core` pinned. So:
one push to `kernel-<kver>-<sha7>` with `--digestfile`, and the floating
`:kernel` name made afterwards by an in-registry `skopeo copy`, which knows its
digests up front and moves no blobs.

## The gate, and both directions of it

`tests/check-kernel-image-pin.sh` is static, offline and takes milliseconds. It
runs in `build-image.yml`'s `changes` job, in `pr-validation.yml`, and in
`build-local.sh` — the three ways this repository gets built.

Eleven mutants, each a single change to a throwaway copy of the four files the
gate reads. Re-run on 2026-09-21 against the tree as PINNED, with a harness
(`/var/lab-scratch/kernel-publish/r39/mutate39.sh`) that `diff`s every mutant
against the source and refuses to report an exit code if nothing changed — the
round-2 harness took the live worktree as its base, so pinning the digest moved
that base under it and a mutation written against the old `localhost` line would
have silently no-opped and read as green.

| mutant | gate |
|---|---|
| the pinned tree, unmodified | **exit 0**, 13/13 ok, "the kernel pin is sound" |
| reverted to `localhost/apex-kernel:local` | exit 1 — the defect that stopped every build |
| `ghcr.io/andrenijman/apex-os:kernel` — a tag, not a digest | exit 1 |
| the same digest on `ghcr.io/attacker/apex-os` | exit 1 |
| `ARG APEX_KERNEL_IMAGE=` with no value | exit 1 |
| the digest truncated to 40 hex characters | exit 1 |
| **the pin wrapped in double quotes** | **exit 0 — THE ONE THAT GOT THROUGH.** Fixed; see below |
| the ARG moved below the first `FROM` | exit 1, naming both line numbers |
| `--digestfile` removed from `kernel-build.yml` | exit 1 |
| `build-local.sh` drops its `--build-arg` override | exit 1 |
| the `btf_scx=usable` refusal removed | exit 1 |

### The mutant that got through, and what it cost to leave open

The gate stripped an optional pair of quotes before judging the value, on the
correct ground that the Dockerfile parser accepts them. `build-image.yml`'s
resolve step does not: it recovers the reference with an anchored
`sed -nE 's|^ARG APEX_KERNEL_IMAGE=(ghcr\.io/…@sha256:[0-9a-f]{64})$|\1|p'`.

So a quoted pin built fine locally, passed the millisecond gate whose entire
purpose is to catch this class before a build is spent, and then failed in CI
with *"is not a `ghcr.io/andrenijman/apex-os@sha256:<64 hex>` digest
reference"* — a message that accuses the digest, which is well formed, rather
than the quotes, which are the fault. Two parsers reading one line disagreed.

Closed by `b505b747`: the stricter parser wins, and the gate prints the bare
line to write instead. Re-mutated after the fix — exit 1, naming the quotes.
Eleven of eleven now behave.

`build-image.yml`'s "Resolve the kernel tier and prove it is reachable" step was
proven the same way without spending a CI run, by running its body verbatim
(`/var/lab-scratch/kernel-publish/resolve-probe.sh`):

- the old `localhost` default → the regex matches nothing, the `::error::` pair
  prints with the offending line, exit 1.
- a well-formed but unpublished digest → **all five** attempts fail with
  `manifest unknown`, the loop does not die early under `set -e`, the final
  `::error::` fires, exit 1. (Probed separately, because it is not obvious:
  `[ "$_try" -lt 5 ] && sleep …` as the last command of a loop body does NOT
  trip errexit — the failing command is not the last of the `&&` list.)
- a published digest → `resolved on attempt 1`, exit 0.

## The proof that `core` builds

CI run [`35582968952`](https://github.com/AndreNijman/apex-os/actions/runs/35582968952),
dispatched 2026-09-21 17:23 AWST from `task/kernel-publish` @ `72bab38d` with
`force_core=true` — a full, cache-bypassing core rebuild. **`core` completed
green at 17:58 AWST, in 35 minutes.** `changes: success`, `rust: success`,
`core: success`.

This is the first time `core` has built in CI in this program.

### The pinned image is what the build actually consumed

Not inferred from a green tick — read out of the build log
(`/var/lab-scratch/kernel-publish/r39/ci-core-job.clean.log`, 7,856 lines):

```
resolved on attempt 1
[2/3] STEP 1/1: FROM ghcr.io/andrenijman/apex-os@sha256:2ff544dd021478dbd36dab4efb1ce43ad9cc858f16cd07a271f01693822972d6 AS kernel-rpms
[3/3] STEP 7/69: COPY --from=kernel-rpms /rpms     /tmp/apex-kernel-rpms
[3/3] STEP 8/69: COPY --from=kernel-rpms /manifest /tmp/apex-kernel-manifest
```

`FROM ${APEX_KERNEL_IMAGE}` — the one line whose failure stopped every image
build in the project — expanded to the pinned digest and resolved on the first
attempt.

### The cross-tier contract held

```
+ test -d /tmp/apex-kernel-rpms
btf_scx=usable
+ BTF=usable
kernel BTF verdict from the build that made these RPMs: usable
kernel image was built from this commit's kernel/kernel.pin
installed: kernel-cachyos-7.2.6-cachyos1.apex1.fc43.x86_64
installed: kernel-cachyos-core-7.2.6-cachyos1.apex1.fc43.x86_64
installed: kernel-cachyos-modules-7.2.6-cachyos1.apex1.fc43.x86_64
installed: kernel-cachyos-devel-matched-7.2.6-cachyos1.apex1.fc43.x86_64
kernel 7.2.6-cachyos1.apex1.fc43.x86_64 installed, from APEX's own build
```

All three assertions fired and passed: the RPMs arrived, the manifest vouches
`btf_scx=usable`, and the `cmp` identity check confirms the published image was
built from **this commit's** `kernel/kernel.pin` — so the digest is not merely
reachable, it is the right kernel for this tree. Every kernel package installed
came from the copied files, none from a repository.

Secure Boot came out of it intact too: `SECURE BOOT: kernel
7.2.6-cachyos1.apex1.fc43.x86_64 signed with the APEX MOK` and `SECURE BOOT: 14
out-of-tree modules signed with the APEX MOK`.

### The predicted akmods failure did NOT reproduce — a finding for `kernel-akmods`

This card predicted `core` would go red ~35-40 minutes in at
`akmods --force --kernels 7.2.6-cachyos1.apex1.fc43.x86_64 --kmod nvidia`,
because `kernel-build-2`'s **local** core build died exactly there. It did not:

```
=== akmods: building nvidia for 7.2.6-cachyos1.apex1.fc43.x86_64 ===
built: /var/cache/akmods/nvidia/kmod-nvidia-7.2.6-cachyos1.apex1.fc43.x86_64-580.178.04-1.fc43.x86_64.rpm
=== akmods: building xone for 7.2.6-cachyos1.apex1.fc43.x86_64 ===
built: /var/cache/akmods/xone/kmod-xone-7.2.6-cachyos1.apex1.fc43.x86_64-1000.0.0.git.1442.85e53359-2.fc43.x86_64.rpm
=== akmods: building xpadneo/…
built: /var/cache/akmods/xpadneo/kmod-xpadneo-7.2.6-cachyos1.apex1.fc43.x86_64-0.10.2-1.fc43.x86_64.rpm
```

nvidia **580.178.04 against CachyOS 7.2.6 builds fine on GitHub's runner**, and
this run does NOT carry `kernel-akmods`' fix (`027f6fe8` is on their branch, not
on `72bab38d`). So the failure is environment-specific to the local build, not a
property of the kernel/driver pair. That belongs to `kernel-akmods`; it is
recorded on their card as a second data point, alongside their own reproducer
passing this morning.

## Bounds held

The runner's isolation is unchanged. `kernel-build.yml`'s `kernel` job takes
`packages: write` for the length of one job of a workflow no pull request can
trigger; the runner user stays unprivileged, podman there stays rootless, the
authfile is written under `RUNNER_TEMP` and removed in an `always()` step which
then asserts the file is gone. Nothing on katana was configured, relaxed or
installed. `Containerfile.kernel` was not edited — that is `kernel-build-2`'s
ground, and the akmods/nvidia failure downstream of this work belongs to
`kernel-akmods`.
