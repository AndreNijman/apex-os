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

### `--cache-to` is what kills these builds

The base job reads the cache and does not write it. That asymmetry is the
result of three failed runs, and it is worth knowing why before anyone
"fixes" it back.

Two runs died mid-build with the identical podman error:

    Error: failed pushing cache ...: reading blob sha256:...:
    locating image with ID "...": image not known

33828399604 at step 18 of 120, 33859688618 at step 44. `--cache-to` pushes
inside the build, so its failure exits 125 and takes a half-finished build with
it. `--cache-from` has never errored — a lookup that misses is free.

The theory in between — that `--cache-to` was too SLOW, at a measured constant
4m13s per layer, and 120 x 4m13s exceeded the 6h job ceiling — was wrong, and
removing both flags on the strength of it produced the worst run of the three
(33834836070: `base` ran 5h44m without finishing, cancelled). Re-measured on
33859688618, per-layer cost is about 22 seconds, which puts a full 120-layer
stage at roughly 45 minutes. The 4m13s belonged to one bad night, on which GHCR
also returned a 500.

Two things are still not understood, and should not be guessed at again:

* Why 33834836070 took 5h44m with no cache flags at all, when the same build
  with `--cache-from` runs at 22s/layer. It is not the storage driver — the
  base job now prints it and the runner reports `overlay`, graph root
  `/var/lib/containers/storage`.
* Whether `--cache-from` earns its place. On 33859688618 it produced ZERO
  hits, so it is currently costing a lookup per layer and returning nothing.
  It stays only because it has never broken a build; if a run is ever needed
  to prove it, measure it rather than reasoning about it.

Nothing writes the cache repository now, so its entries age out after the
14-day TTL and `--cache-from` becomes a no-op. That is a performance question,
not a correctness one, and it is the correct trade against a flag that has
killed two builds outright.

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
