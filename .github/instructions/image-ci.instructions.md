---
applyTo: "Containerfile*,.github/workflows/**/*.yml,build-local.sh,files/scripts/**,kernel/**,signing/**"
---

# Image, kernel, signing, and release review

- Preserve the three-tier release model. `core` is slow-moving and a rebuild makes the next fleet update multi-gigabyte. Only actual core inputs, an upstream digest change, or an explicit force operation may rebuild it. Base and flavors must consume parents by immutable digest.
- Package installs, network downloads, and third-party builds belong in `Containerfile.core`; repository-owned files and first-party `apexd` compilation belong in `Containerfile.base`. Never hide a package transaction in the volatile tier.
- Every external action, image, package source, kernel/driver pair, and shell checkout must be pinned or resolved once and carried forward immutably. A long build must not silently pick up a newer APEX Shell commit halfway through.
- Preserve the exact published tags `daily`, `gaming-mesa`, and `gaming-nvidia`. Platform tags are stable inputs for shell-only releases. Promotion must be serialized, must not let an older job overwrite a newer image, and must occur only after verification and signing.
- Secure Boot is a product invariant. The shipped kernel and required out-of-tree modules must be signed by the expected chain; marker files alone are not proof. Never commit private keys. CI gets signing material only through secrets or ephemeral identity.
- Keep least-privilege workflow permissions. Treat `pull_request_target`, interpolation into shell, artifact extraction, cache restore, and code from forks as hostile-input boundaries. Never execute untrusted PR code with write tokens or secrets.
- Preserve OCI source-revision labels and cosign keyless signing identity. Fail closed when digest capture, signature verification, kernel verification, layer-prefix verification, or promotion preconditions are unavailable.
- Keep shell-only releases small. They must inherit platform blobs unchanged and reject excessive new compressed data; recompressing inherited layers creates a full-fleet download.
- NVIDIA akmods must be built against the exact shipped kernel and hard-fail on version skew. Mesa and NVIDIA flavor contents must not leak into one another. Daily-specific stability policy must not silently inherit Gaming experiments.
- Never mask a meaningful command failure with `|| true`. A tolerated upstream defect requires a narrow exception followed by a hard postcondition proving the required artifact exists and is valid.
- For workflow changes, inspect trigger/path-filter consequences. A check required on every PR must always report, including documentation-only PRs and skipped matrix paths.
