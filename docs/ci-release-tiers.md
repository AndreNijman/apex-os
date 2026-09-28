# CI release tiers

APEX images use two release paths because a shell change does not justify
rebuilding Mesa, NVIDIA modules, package transactions, or initramfs images.

## Platform builds

`.github/workflows/build-image.yml` owns the slow path:

1. `core` contains the kernel, slow-moving third-party dependencies, and every
   out-of-tree kernel module (NVIDIA, xone, xpadneo) together with its MOK
   signature. It is the only tier the signing secret is mounted in.
2. `base` contains APEX system services, shared OS configuration and the
   vendored APEX Shell. The workflow's "Pin apex-shell" step resolves the
   apex-shell branch with the same name as the apex-os ref (`main` for a `main`
   build) to one commit, and `base` vendors that commit on every run, including
   runs that reuse the published `core`.
3. `Containerfile.apex` stamps the edition (`VARIANT_ID=apex`), installs the boot
   splash and owns the final initramfs.
4. The one green image is promoted to `apex`, `daily`, `gaming-mesa` and
   `gaming-nvidia`, plus `edge`, plus moving and revision-pinned
   `platform-<name>` tags for each. A step then reads every tag's digest back
   out of the registry and fails the run if any differs.

There is no flavor matrix and no `target_platform` input: there is one image.

## Update channels

Four tags on that one image, and they are what `apex channel` moves a machine
between:

| tag | moved by | what it means |
|---|---|---|
| `edge` | every successful build of `main` | new work arrives first, and so do its faults |
| `beta` | `promote-channel.yml`, by hand | a build that has been on edge and looks sound |
| `candidate` | the same | a build being considered for stable |
| `stable` | the same | only builds that have been through the other three |

`edge` is promoted in the same step as `apex`, `daily`, `gaming-mesa` and
`gaming-nvidia`, because it is a name for what those four already are. The other
three stay out of that step on purpose: a channel that advanced on every build
would be edge with a different spelling, and a user who chose `stable` would be
taking edge's risk while believing they had opted out of it.

`.github/workflows/promote-channel.yml` moves the slow three. It is
`workflow_dispatch` only, shares this workflow's concurrency group, and refuses
before it writes anything unless both hold:

- the digest is cosign-signed under this repository's `build-image.yml` identity
  on `refs/heads/main`. A channel tag is what a machine's bootc origin points
  at, and pointing one at an unsigned or forked image would undo the supply
  chain with a pasted string;
- the digest is **already** on the channel above the target. `apex channel list`
  tells users stable carries "only builds that have run on the other channels
  first", and this check is what makes that sentence true.

The four migration tags keep their own meaning and keep moving on every build.
Other people's laptops track them, so treat them as live references and never as
aliases for `edge` to retire. A machine following one is on edge whether or not
it uses the word, and `apex channel status` says so in those terms.

Platform builds run on a push to `main` that touches `Containerfile*`, `files/`,
`apexd/`, `config/`, `kernel/` or `.github/`, on the weekly upstream refresh
(Monday 06:00 UTC), and on a manual dispatch. No tier uses a registry build
cache; the next section says why.

### The base job commits once, and uses no registry cache

Two separate things went wrong here. Both are recorded because the reasoning
that produced the wrong fixes looked strong at the time.

**`--cache-to` kills builds.** Two runs died mid-build with the same podman
error, `failed pushing cache ...: locating image with ID ...: image not known`,
at step 18 and step 44 of 120. The push happens inside `podman build`, so it
exits 125 and takes a half-finished build with it. The job no longer passes the
flag.

**Per-step commits are what made `base` unfinishable.** With `--layers=true`,
podman commits an image layer per Containerfile step. On this runner a commit
costs a dead-constant 3m28s no matter how small the step, and base produces 102
of its own layers: 5h54m, against a 6h job ceiling. Run 33866516076 hit exactly
that and was cancelled at 6h01m. The job now passes `--layers=false` and commits
once.

The price is small, and it was measured: base's own 102 layers total
**14.9 MiB** (largest 7.0 MiB). Squashing them means a change to any base file
redownloads ~15 MiB rather than only the layers that moved, which is noise
beside the 5.26 GiB core those machines already hold and never re-fetch. The
core/base split, which is what keeps `apex update` small, is untouched.

`--cache-from` went with it: with no intermediate layers there is nothing to
look up. Nothing reads or writes the build-cache repository now.

**Still unexplained**, and not worth a fourth theory: the 3m28s a commit costs
at all. It was 22s against the previous core and 3m28s against this one, for an
image 11% larger (101 layers / 4.73 GiB -> 107 / 5.26 GiB). Neither size nor
layer count accounts for that, the two measurements come from different runners
hours apart, and nobody has ruled out runner variability. The storage driver is
not the cause: the base job prints it and the runner reports `overlay`. The fix
above does not depend on knowing the cause: it removes 101 of the 102 commits,
which helps either way.

Collapsing three image builds into one does not change any of this, because the
cost is per layer, not per image.

## Shell releases

`.github/workflows/release-shell.yml` owns the fast path. It resolves an exact
40-character `apex-shell` commit, builds a final layer on the stable platform
image (`platform-apex`), verifies that every inherited layer digest is
unchanged, signs the result, then promotes every user-facing tag to that one
digest:

- `apex`
- `daily`
- `gaming-mesa`
- `gaming-nvidia`

All four come from one build. Three legs building identical content would
produce three different manifest digests and pull the tags apart again, so this
workflow has no matrix either. A shell release does not move `edge`; the next
`main` build does.

BuildKit pushes directly to GHCR for this path. The hosted runner's older Podman
cannot reliably preserve inherited `zstd:chunked` blob digests; recompressing
those layers would turn a small shell update into a full fleet download. The
digest-prefix gate blocks promotion if that ever happens. BuildKit regenerates
the OCI manifest, so inherited `zstd:chunked` seek-table annotations may be lost,
but bootc's whole-layer reuse remains intact because the blob digests are equal.

Both workflows share one non-cancelling publication lock, so a shell release
cannot overwrite a newer full platform build. A shell release also verifies the
platform's keyless signature and refuses to promote more than 150 MiB of new
compressed layers.

A stable platform must come from `build-image.yml` on `main`. The shell path
verifies that exact keyless workflow identity and also requires the platform's
signed-kernel marker, so feature-branch platform builds and unsigned development
images cannot reach the fleet this way.

`build-image.yml` writes `platform-apex` on every `main` build. If the tag is
missing, a shell release seeds it with an in-registry copy of the first full
platform image it finds (`platform-daily`, then `apex`, then `daily`), skipping
any image that is itself a shell release. That copy rebuilds and uploads no
blobs.

## Usage

For a shell-only release, dispatch `release-shell` and leave `shell_ref` blank
so it resolves `apex-shell/main`, or give it an exact commit. It has no flavor
input: one build moves all four user-facing tags.

Use `build-image` only when OS packages, drivers, kernel, initramfs, apexd, or
image-owned configuration changed. Its "Pin apex-shell" step pins the exact
shell SHA at the start of the run, so a long platform build cannot pick up a
different shell commit halfway through. Every `main` build, the Monday cron
included, ships the apex-shell `main` commit it pinned.
