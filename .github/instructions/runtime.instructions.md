---
applyTo: "files/**,config/**,Containerfile.daily,Containerfile.gaming,docs/packages.md"
---

# Runtime, desktop, and edition review

- APEX Shell is image-vendored at `/usr/share/apex-shell`; do not restore per-user clones or an independent in-app updater. Runtime-generated shell/config state belongs under the user's home or `/var`, never the read-only source tree.
- Hyprland is primary and niri remains selectable. Greeter, session startup, lock screen, portals, polkit agent, audio, networking, notifications, and shell IPC must work without GNOME dependencies or a network fetch at first login.
- Keep branding coherent by edition: chartreuse Daily, gold Gaming, mono only in neutral contexts. `VARIANT_ID` drives edition behavior. Do not hardcode one edition's identity into shared runtime paths.
- Daily prioritizes stability, battery life, suspend reliability, and general use. Gaming is ready on first boot and may carry Steam, controller support, gamescope/MangoHud, low-latency tuning, VR/streaming, and GPU-specific payloads. Risky Gaming policy must not leak into Daily or generic hardware.
- Preserve SELinux enforcing. New daemons, D-Bus services, polkit actions, privileged helpers, writable paths, device access, and systemd units require correct ownership, permissions, labeling, and the narrowest policy. Do not solve AVCs by disabling enforcement or granting broad access.
- Systemd units must have correct ordering, restart behavior, shutdown cleanup, and enablement. Avoid boot-critical dependencies on network availability. Optional hardware/services must fail without degrading the boot transaction.
- Shell scripts run with their declared interpreter. Quote expansions, handle spaces/newlines where relevant, use atomic replacement for persistent configuration, and do not silently overwrite user edits. `/etc` defaults and provisioned user files need explicit ownership semantics.
- Default applications and MIME handlers must remain valid after install and update. Cached schemas/icons/MIME/dconf data must be rebuilt after image-owned files are copied, with postconditions checking the effective result.
- Added third-party runtime software belongs in core, not base. Decide explicitly whether an application is image-owned, Flatpak, or user system-extension content; do not create another package/update mechanism.
