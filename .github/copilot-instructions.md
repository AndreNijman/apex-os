# APEX-OS contract

Review against the contract below, not merely for syntax or local correctness. Report a concrete inline finding whenever a change can violate an invariant. Do not approve an assumption because it resembles Fedora, Bazzite, or another image-based distribution.

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
