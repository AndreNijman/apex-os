# CI release tiers

APEX images use two release paths because a shell change does not justify
rebuilding Mesa, NVIDIA modules, package transactions, or initramfs images.

## Platform builds

`.github/workflows/build-image.yml` owns the slow path:

1. `core` contains the kernel, slow-moving third-party dependencies, and every
   out-of-tree kernel module (NVIDIA, xone, xpadneo) together with its MOK
   signature — this is the only tier the signing secret is mounted in.
2. `base` contains APEX system services and shared OS configuration.
3. `Containerfile.apex` stamps the edition and owns the final initramfs.
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
three are deliberately absent from that step: a channel that advanced on every
build would be edge with a different spelling, and a user who chose `stable`
would be taking edge's risk while believing they had opted out of it.

`.github/workflows/promote-channel.yml` moves the slow three. It is
`workflow_dispatch` only, shares this workflow's concurrency group, and refuses
before it writes anything unless both hold:

- the digest is cosign-signed under this repository's `build-image.yml` identity
  on `refs/heads/main` — a channel tag is what a machine's bootc origin points
  at, so pointing one at an unsigned or forked image is the supply chain undone
  by a pasted string;
- the digest is **already** on the channel above the target. `apex channel list`
  tells users stable carries "only builds that have run on the other channels
  first", and without this check that sentence is a hope.

The four migration tags keep their own meaning and keep moving on every build.
They are not deprecated aliases for `edge`; they are live references owned by
other people's laptops, and a machine following one is on edge whether or not it
uses the word. `apex channel status` says so in those terms.
Platform builds run for OS source changes and the weekly upstream refresh. The
reusable base tier uses a registry-backed Buildah cache with a 14-day lookup
lifetime.

### The base job commits once, and uses no registry cache

Two separate things went wrong here. Both are recorded because the reasoning
that produced the wrong fixes looked strong at the time.

**`--cache-to` kills builds.** Two runs died mid-build with the same podman
error, `failed pushing cache ...: locating image with ID ...: image not known`,
at step 18 and step 44 of 120. The push happens inside `podman build`, so it
exits 125 and takes a half-finished build with it. It is gone.

**Per-step commits are what made `base` unfinishable.** With `--layers=true`,
podman commits an image layer per Containerfile step. On this runner a commit
costs a dead-constant 3m28s no matter how small the step, and base produces 102
of its own layers: 5h54m, against a 6h job ceiling. Run 33866516076 hit exactly
that and was cancelled at 6h01m. The job now passes `--layers=false` and commits
once.

The price is small and was measured, not assumed: base's own 102 layers total
**14.9 MiB** (largest 7.0 MiB). Squashing them means a change to any base file
redownloads ~15 MiB rather than only the layers that moved — noise beside the
5.26 GiB core those machines already hold and never re-fetch. The core/base
split, which is what actually keeps `apex update` small, is untouched.

`--cache-from` went with it: with no intermediate layers there is nothing to
look up. Nothing reads or writes the build-cache repository now.

**What is still not explained**, and should not attract a fourth theory: why a
commit costs 3m28s at all. It was 22s against the previous core and 3m28s
against this one, for an image 11% larger (101 layers / 4.73 GiB -> 107 / 5.26
GiB). Neither size nor layer count accounts for that, the two measurements come
from different runners hours apart, and runner variability has not been ruled
out. The storage driver is not the answer — the base job prints it and the
runner reports `overlay`. The fix above deliberately does not depend on knowing
the cause: it removes 101 of the 102 commits, which helps regardless.

Collapsing three image builds into one does not change any of this, because the
cost is per layer, not per image.

## Shell releases

`.github/workflows/release-shell.yml` owns the fast path. It resolves an exact
40-character `apex-shell` commit, builds a final layer on the selected stable
platform image, verifies that every inherited layer digest is unchanged, signs
the result, then promotes every user-facing tag to that one digest:

- `apex`
- `daily`
- `gaming-mesa`
- `gaming-nvidia`

All four, from one build. Three legs building identical content would produce
three different manifest digests and pull the tags apart again, which is why the
matrix is gone here too.

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

Stable platforms must be built by `build-image.yml` on `main`. The shell path
verifies that exact keyless workflow identity and also requires the platform's
signed-kernel marker; feature-branch platform builds and unsigned development
images are intentionally not eligible for fleet promotion.

The first shell release initializes a missing `platform-<flavor>` tag by taking
an in-registry snapshot of the last green user image. No blobs are rebuilt or
uploaded for that migration.

## Usage

For a shell-only release, dispatch `release-shell` and normally leave
`shell_ref` blank so it resolves `apex-shell/main`. Select `all`, one flavor, or
`daily-and-nvidia`.

Use `build-image` only when OS packages, drivers, kernel, initramfs, apexd, or
image-owned configuration changed. The exact shell SHA is pinned at the start
of that workflow, so a long platform build cannot silently pick up a different
shell commit halfway through.
