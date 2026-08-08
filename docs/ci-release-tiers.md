# CI release tiers

APEX images use two release paths because a shell change does not justify
rebuilding Mesa, NVIDIA modules, package transactions, or initramfs images.

## Platform builds

`.github/workflows/build-image.yml` owns the slow path:

1. `core` contains the kernel and slow-moving third-party dependencies.
2. `base` contains APEX system services and shared OS configuration.
3. The flavor matrix builds Daily, Gaming Mesa, and Gaming NVIDIA.
4. Each green flavor is also promoted to moving and revision-pinned
   `platform-<flavor>` tags.

Platform builds run for OS source changes and the weekly upstream refresh. A
manual run can target one flavor or Daily plus Gaming NVIDIA. The reusable base
tier uses a registry-backed Buildah cache with a 14-day lookup lifetime.

## Shell releases

`.github/workflows/release-shell.yml` owns the fast path. It resolves an exact
40-character `apex-shell` commit, builds a final layer on the selected stable
platform images, verifies that every inherited layer digest is unchanged, signs
the result, then promotes the existing user-facing tags:

- `daily`
- `gaming-mesa`
- `gaming-nvidia`

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
