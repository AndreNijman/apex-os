# APEX-OS agent contract

All coding agents must work against the contract below, not merely for syntax or local correctness. Do not assume behavior because it resembles Fedora, Bazzite, or another image-based distribution. Before editing, trace the relevant build and runtime path; after editing, run every applicable test in `.github/workflows/pr-validation.yml`.

## Product identity

APEX-OS is a production, atomic Fedora bootc operating-system family, not a Fedora customization script. It has one shared base and three published flavors: Daily, Gaming Mesa, and Gaming NVIDIA. Daily must be a stable, efficient general-purpose system; Gaming must be game-ready at first boot and pursue better frame pacing and performance than stock alternatives without making unsafe global tuning the default. Hyprland is the primary desktop, niri is selectable, APEX Shell is the native UI, and `apexd` plus the `apex` CLI own system policy.

The installed system must remain image-based, transactional, reproducible, rollback-capable, Secure-Boot-capable, SELinux-enforcing, and traceable from an image digest to an exact source revision. Never accept a change that quietly turns a machine into mutable Fedora, creates machine drift, weakens verification, or adds an independent updater for image-owned components.

## Non-negotiable architecture

- `/usr` is image-owned and read-only at runtime. Persistent mutable state belongs under `/etc`, `/var`, or the user's home as appropriate.
- OS updates use bootc and must remain atomic with a working previous deployment. APEX Shell is vendored into the signed image so UI and OS update and roll back together.
- User RPM installation uses the APEX system-extension engine. Never use or recommend `rpm-ostree install`, because a layered deployment blocks bootc upgrades. Flatpak remains preferred for sandboxed desktop apps.
- `Containerfile.core` owns Fedora/kernel, package transactions, downloads, and third-party compilation. `Containerfile.base` may copy repository content and compile first-party `apexd`; moving volatile content into core causes multi-gigabyte fleet updates.
- Core and base parents are digest-pinned in CI. Every published image is keyless cosign-signed, carries the source SHA, contains a signed kernel, and is promoted only after hard verification.
- Hardware-specific behavior is selected by runtime sysprofiles. Device-risky tuning must never leak into generic profiles. Unsupported hardware must degrade safely, not fail boot.
- D-Bus API compatibility matters: `org.apexos.Apexd1` is consumed by the CLI and shell. Treat member, path, type, and semantic changes as public API changes.

## Review standard

Trace behavior across files and lifecycle stages: build, first boot, normal runtime, update, rollback, offline operation, and failure recovery. Flag fail-open checks, ignored failures, unpinned mutable inputs, shell quoting bugs, unsafe privilege boundaries, SELinux omissions, secrets, destructive operations without validation, update-size regressions, edition leakage, and behavior that works only on the developer's machine.

Require tests or executable assertions for every repaired bug and every destructive, privileged, parser, policy-selection, or release-path change. Assertions must test the shipped artifact or externally visible behavior, not a duplicate implementation. Preserve one logical change per commit and Conventional Commit messages. Never add AI attribution to commits, PR text, release notes, or source files.

Documentation and comments must state measured facts and current behavior. Reject stale claims, aspirational language presented as implemented, and unexplained workarounds. Small, explicit changes are preferred over compatibility layers or speculative abstractions.

## Image, kernel, signing, and release rules

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

## Runtime, desktop, and edition rules

- APEX Shell is image-vendored at `/usr/share/apex-shell`; do not restore per-user clones or an independent in-app updater. Runtime-generated shell/config state belongs under the user's home or `/var`, never the read-only source tree.
- Hyprland is primary and niri remains selectable. Greeter, session startup, lock screen, portals, polkit agent, audio, networking, notifications, and shell IPC must work without GNOME dependencies or a network fetch at first login.
- Keep branding coherent by edition: chartreuse Daily, gold Gaming, mono only in neutral contexts. `VARIANT_ID` drives edition behavior. Do not hardcode one edition's identity into shared runtime paths.
- Daily prioritizes stability, battery life, suspend reliability, and general use. Gaming is ready on first boot and may carry Steam, controller support, gamescope/MangoHud, low-latency tuning, VR/streaming, and GPU-specific payloads. Risky Gaming policy must not leak into Daily or generic hardware.
- Preserve SELinux enforcing. New daemons, D-Bus services, polkit actions, privileged helpers, writable paths, device access, and systemd units require correct ownership, permissions, labeling, and the narrowest policy. Do not solve AVCs by disabling enforcement or granting broad access.
- Systemd units must have correct ordering, restart behavior, shutdown cleanup, and enablement. Avoid boot-critical dependencies on network availability. Optional hardware/services must fail without degrading the boot transaction.
- Shell scripts run with their declared interpreter. Quote expansions, handle spaces/newlines where relevant, use atomic replacement for persistent configuration, and do not silently overwrite user edits. `/etc` defaults and provisioned user files need explicit ownership semantics.
- Default applications and MIME handlers must remain valid after install and update. Cached schemas/icons/MIME/dconf data must be rebuilt after image-owned files are copied, with postconditions checking the effective result.
- Added third-party runtime software belongs in core, not base. Decide explicitly whether an application is image-owned, Flatpak, or user system-extension content; do not create another package/update mechanism.
