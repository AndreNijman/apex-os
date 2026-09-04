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
   `gaming-nvidia`, plus moving and revision-pinned `platform-<name>` tags for
   each. A step then reads every tag's digest back out of the registry and fails
   the run if any differs.

There is no flavor matrix and no `target_platform` input: there is one image.
Platform builds run for OS source changes and the weekly upstream refresh.

No tier uses a registry build cache. The base tier used to, with a 14-day
lookup lifetime, and it was removed after being measured: `--cache-to` cost a
constant 4m13s per layer against layers whose own commands ran in one to two
seconds. Across base's 120 layers that is 8h26m, against the 6h GitHub job
limit — which the workflow does not override with `timeout-minutes`, so the 6h
default is the ceiling. No build using that cache could finish, and the
arithmetic settles it on its own.

Rebuilding the same Containerfile against the same pinned core parent without
the flags measured 13.3s per layer — but on the Katana (20 cores, local overlay
store, no registry push), not on a runner, so treat that as an order-of-
magnitude sanity check and not as a same-environment counterfactual.

The build-image workflow carries the per-layer timings; do not re-add the flags
without re-measuring on a runner. Note also that collapsing three image builds
into one does not change that arithmetic, because the cost was per layer, not
per image.

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
