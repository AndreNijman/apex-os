# CI release tiers

APEX images use two release paths because a shell change does not justify
rebuilding Mesa, NVIDIA modules, package transactions, or initramfs images.

## Platform builds

`.github/workflows/build-image.yml` owns the slow path:

1. `core` contains the kernel and slow-moving third-party dependencies.
2. `base` contains APEX system services and shared OS configuration.
3. The flavor matrix builds Daily, Gaming Mesa, and Gaming NVIDIA.
4. Each green flavor is also promoted to `platform-<flavor>`.

Platform builds run for OS source changes and the weekly upstream refresh. A
manual run can target one flavor or Daily plus Gaming NVIDIA. Registry-backed
Buildah caches are split per tier and expire from lookup after 14 days.

## Shell releases

`.github/workflows/release-shell.yml` owns the fast path. It resolves an exact
40-character `apex-shell` commit, builds a final layer on the selected stable
platform images, verifies that every inherited layer digest is unchanged, then
promotes and signs the existing user-facing tags:

- `daily`
- `gaming-mesa`
- `gaming-nvidia`

BuildKit pushes directly to GHCR for this path. The hosted runner's older Podman
cannot reliably preserve inherited `zstd:chunked` descriptors; recompressing
those layers would turn a small shell update into a full fleet download. The
digest-prefix gate blocks promotion if that ever happens.

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
