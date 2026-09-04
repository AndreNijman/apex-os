# APEX-OS agent contract

All coding agents must work against the contract below, not merely for syntax or local correctness. Do not assume behavior because it resembles Fedora, Bazzite, or another image-based distribution. Before editing, trace the relevant build and runtime path; after editing, run every applicable test in `.github/workflows/pr-validation.yml`.

## Product identity

APEX-OS is a production, atomic Fedora bootc operating-system family, not a Fedora customization script. It publishes **one image for every laptop**. It must be a stable, efficient general-purpose system that is also capable of gaming, pursuing better frame pacing and performance than stock alternatives without making unsafe global tuning the default. Hyprland is the primary desktop, niri is selectable, APEX Shell is the native UI, and `apexd` plus the `apex` CLI own system policy.

The single image replaced three flavors (Daily, Gaming Mesa, Gaming NVIDIA) that were one operating system with different payloads bolted on. The split forced a choice at install time that nobody has the information to make — someone with a gaming laptop who plays occasionally had to pick, and picking Daily left them with no GPU driver. What decides where a component lives now is a technical line, not a market segment:

**A kernel module cannot be installed at runtime under Secure Boot; userspace can.** Making a module loadable under an enforcing kernel requires signing it with the APEX MOK private key, which is a CI secret and must never reach a user's machine (`apex-pkg` refuses kernels, modules and akmods for exactly this reason). So every out-of-tree kernel module — the NVIDIA akmod and the xone/xpadneo controller akmods — is built and signed into the image, and the gaming userspace — Steam, Proton, gamescope, MangoHud, Sunshine, OBS — installs on demand via `apex install`. Do not move anything across that line for convenience, in either direction: baking the userspace in undoes the reason the editions collapsed, and deferring a module to runtime cannot be made to work.

The installed system must remain image-based, transactional, reproducible, rollback-capable, Secure-Boot-capable, SELinux-enforcing, and traceable from an image digest to an exact source revision. Never accept a change that quietly turns a machine into mutable Fedora, creates machine drift, weakens verification, or adds an independent updater for image-owned components.

## Non-negotiable architecture

- `/usr` is image-owned and read-only at runtime. Persistent mutable state belongs under `/etc`, `/var`, or the user's home as appropriate.
- OS updates use bootc and must remain atomic with a working previous deployment. APEX Shell is vendored into the signed image so UI and OS update and roll back together.
- User RPM installation uses the APEX system-extension engine. Never use or recommend `rpm-ostree install`, because a layered deployment blocks bootc upgrades. Flatpak remains preferred for sandboxed desktop apps.
- `Containerfile.core` owns Fedora/kernel, package transactions, downloads, third-party compilation, and every out-of-tree kernel module together with its MOK signature — core is the only tier the signing secret is mounted in, and the only tier consumed by immutable digest, so a driver placed there is downloaded once and never again. `Containerfile.base` may copy repository content and compile first-party `apexd`; `Containerfile.apex` is the thin final tier that stamps the edition and owns the last initramfs regeneration. Moving volatile content into core causes multi-gigabyte fleet updates; moving a package transaction out of core into the volatile tiers puts an rpmdb-sized layer into every user's next update.
- Core and base parents are digest-pinned in CI. Every published image is keyless cosign-signed, carries the source SHA, contains a signed kernel, and is promoted only after hard verification.
- Hardware-specific behavior is selected by runtime sysprofiles. Device-risky tuning must never leak into generic profiles. Unsupported hardware must degrade safely, not fail boot.
- D-Bus API compatibility matters: `org.apexos.Apexd1` is consumed by the CLI and shell. Treat member, path, type, and semantic changes as public API changes.

## Review standard

Trace behavior across files and lifecycle stages: build, first boot, normal runtime, update, rollback, offline operation, and failure recovery. Flag fail-open checks, ignored failures, unpinned mutable inputs, shell quoting bugs, unsafe privilege boundaries, SELinux omissions, secrets, destructive operations without validation, update-size regressions, edition leakage, and behavior that works only on the developer's machine.

Require tests or executable assertions for every repaired bug and every destructive, privileged, parser, policy-selection, or release-path change. Assertions must test the shipped artifact or externally visible behavior, not a duplicate implementation. Preserve one logical change per commit and Conventional Commit messages. Never add AI attribution to commits, PR text, release notes, or source files.

Documentation and comments must state measured facts and current behavior. Reject stale claims, aspirational language presented as implemented, and unexplained workarounds. Small, explicit changes are preferred over compatibility layers or speculative abstractions.

## Image, kernel, signing, and release rules

- Preserve the three-tier BUILD model — `core` → `base` → `apex`. This is about download size and is unchanged by the collapse of the editions: `core` is slow-moving and a rebuild makes the next fleet update multi-gigabyte, so only actual core inputs, an upstream digest change, or an explicit force operation may rebuild it. Each tier must consume its parent by immutable digest. Do not reintroduce a per-edition tier; there is one image and one final tier.
- Package installs, network downloads, third-party builds, and out-of-tree kernel modules belong in `Containerfile.core`; repository-owned files and first-party `apexd` compilation belong in `Containerfile.base`. Never hide a package transaction in a volatile tier — every `dnf` transaction rewrites the ~200 MB sqlite rpmdb into its own layer, and above `core` that layer ships to every machine on every update.
- Every external action, image, package source, kernel/driver pair, and shell checkout must be pinned or resolved once and carried forward immutably. A long build must not silently pick up a newer APEX Shell commit halfway through.
- `apex` is the canonical published tag, and `daily`, `gaming-mesa` and `gaming-nvidia` must keep resolving to the SAME digest forever. They are no longer editions; they are live references owned by other people's laptops. A tag that stops being updated does not error — `bootc upgrade` reports "no update available" indefinitely and that machine silently stops receiving security updates. CI asserts by reading each tag's digest back out of the registry, not by trusting that the promotion command exited zero. Platform tags remain stable inputs for shell-only releases. Promotion must be serialized, must not let an older job overwrite a newer image, and must occur only after verification and signing.
- Secure Boot is a product invariant. The shipped kernel AND every out-of-tree module must be signed by the expected chain, and each must have a verification step that reads the signature out of the built artifact — `sbverify` for the kernel image, `modinfo -F signer` for modules. Marker files alone are not proof, and a check that iterates over a set that can be empty is not proof either: assert a non-zero count. Never commit private keys. CI gets signing material only through secrets or ephemeral identity, and the secret must be mounted in whichever tier does the signing.
- Keep least-privilege workflow permissions. Treat `pull_request_target`, interpolation into shell, artifact extraction, cache restore, and code from forks as hostile-input boundaries. Never execute untrusted PR code with write tokens or secrets.
- Preserve OCI source-revision labels and cosign keyless signing identity. Fail closed when digest capture, signature verification, kernel verification, layer-prefix verification, or promotion preconditions are unavailable.
- Keep shell-only releases small. They must inherit platform blobs unchanged and reject excessive new compressed data; recompressing inherited layers creates a full-fleet download.
- Out-of-tree akmods must be built against the exact shipped kernel and hard-fail on version skew; building them in the same tier as the kernel is what makes skew impossible rather than merely detected. Mesa and NVIDIA now ship together on every machine and must keep coexisting through GLVND: neither vendor may own `libGL.so.1` (libglvnd does), and an AMD-only or Intel-only laptop must not regress because the NVIDIA userspace is present. Assert it against the built image.
- Never mask a meaningful command failure with `|| true`. A tolerated upstream defect requires a narrow exception followed by a hard postcondition proving the required artifact exists and is valid.
- For workflow changes, inspect trigger/path-filter consequences. A check required on every PR must always report, including documentation-only PRs and skipped matrix paths.

## Runtime, desktop, and edition rules

- APEX Shell is image-vendored at `/usr/share/apex-shell`; do not restore per-user clones or an independent in-app updater. Runtime-generated shell/config state belongs under the user's home or `/var`, never the read-only source tree.
- Hyprland is primary and niri remains selectable. Greeter, session startup, lock screen, portals, polkit agent, audio, networking, notifications, and shell IPC must work without GNOME dependencies or a network fetch at first login.
- Branding is chartreuse; `VARIANT_ID=apex` is the one edition. The greeter still recognises `daily` and `gaming` so a machine that has not yet updated past the split keeps its accent, and `/etc/apex-greet/edition` still overrides per machine. Do not hardcode an identity into shared runtime paths.
- The image prioritizes stability, battery life and suspend reliability, because every laptop runs it — including the ones that never game. Risky tuning stays behind an explicit `apex` action (game mode, tiers, modes), never in the default boot. A desktop entry for software that installs on demand must be gated on that software actually being present (`TryExec`), or the user is offered a session that bounces them back to the greeter.
- Preserve SELinux enforcing. New daemons, D-Bus services, polkit actions, privileged helpers, writable paths, device access, and systemd units require correct ownership, permissions, labeling, and the narrowest policy. Do not solve AVCs by disabling enforcement or granting broad access.
- Systemd units must have correct ordering, restart behavior, shutdown cleanup, and enablement. Avoid boot-critical dependencies on network availability. Optional hardware/services must fail without degrading the boot transaction.
- Shell scripts run with their declared interpreter. Quote expansions, handle spaces/newlines where relevant, use atomic replacement for persistent configuration, and do not silently overwrite user edits. `/etc` defaults and provisioned user files need explicit ownership semantics.
- Default applications and MIME handlers must remain valid after install and update. Cached schemas/icons/MIME/dconf data must be rebuilt after image-owned files are copied, with postconditions checking the effective result.
- Added third-party runtime software belongs in core, not base. Decide explicitly whether an application is image-owned, Flatpak, or user system-extension content; do not create another package/update mechanism.

## Agent runtime rules

- `apex-agentd` is unprivileged, per-user, and must stay that way. It handles untrusted model output and spawns arbitrary user programs, so its worst case must remain a user-session compromise. Never move agent orchestration into `apexd`, never give the daemon a polkit action, a system-bus name, or a setuid helper, and never let it call `apexd` on a session's behalf. A session that needs a system change is served by the user's own `apex` invocation over the frozen `org.apexos.Apexd1` surface.
- The daemon is opt-in. Do not `systemctl --global enable` the user unit: a per-user daemon holding PTYs must not start for users who never run an agent.
- The sandbox is default-deny and fails closed. `$HOME` and `$XDG_RUNTIME_DIR` are masked and only an explicit allowlist is bound back; the environment is cleared and rebuilt from a declared list. A confined session that cannot be confined as requested must not start — never downgrade a policy silently, and never widen the allowlist to make a tool work without saying why in the code. Treat every new entry as a credential-exposure question: `~/.ssh`, `~/.gnupg`, `~/.aws`, browser profiles and agent sockets must stay unreachable.
- `bubblewrap` is a security dependency satisfied indirectly (it arrives with flatpak) and asserted in `Containerfile.base`. Keep the assertion. If the indirect source ever goes away, add the package to core deliberately rather than deleting the check.
- Session state detection must not invent certainty. `permission_request` is only ever set by a published event, never inferred from terminal output; do not add pattern matching over an agent's prose to guess intent. Inferred states come from documented terminal signals (bell, OSC 9/777, OSC 133), idle time and exit status.
- Upstream agent CLIs are launched unmodified in a real PTY. Never wrap, patch, proxy or reimplement them, and never make the runtime a prerequisite for running `claude`, `opencode`, `codex` or `gemini` directly.
- Checkpoints must not disturb the user's git state. Capture goes through a temporary index (`GIT_INDEX_FILE`) so the index, stash and branch are untouched; checkpoint refs live under `refs/apex/`, never `refs/heads/`. Restore takes a safety checkpoint first. Ignored files stay out of checkpoints, and package changes are reported rather than silently removed.
- The control protocol is a compatibility surface: the CLI and APEX Shell both parse `SessionInfo`. Treat field renames, removals and semantic changes the way `org.apexos.Apexd1` changes are treated, and bump `PROTOCOL_VERSION` when a change is not backward compatible. Serialised requests and responses must never contain a raw newline, because the framing is line-based.

## Editing a live machine's configuration

These rules exist because breaking them destroyed the developer's desktop. A
single `re.S` in a one-line substitution deleted 217 of 256 lines from his live
`hyprland.conf`, taking every `exec-once` with it: next reboot, no shell, no
wallpaper daemon, no polkit agent, no clipboard, no input method.

1. **Back up before touching a live config.** A `.pre-<change>` copy alongside
   it. This is what turned that outage into a two-minute restore.
2. **`re.M`, never `re.S`, for a line-oriented edit.** With `re.S` a trailing
   `.*$` matches to the end of the FILE, not the end of the line. If a
   genuinely multi-line match is needed, bound it explicitly — never with
   `.*$`.
3. **Assert the line count is unchanged** before writing a config whose shape
   you do not intend to change, and assert the substitution count is exactly
   what you expected. One `assert` turns a silent truncation into a refusal.
4. **Grepping for what you added cannot detect what you deleted.** Verify with
   a size or line count AND a landmark that must still be present — for
   hyprland.conf, `grep -c exec-once`. A truncated config is still a *valid*
   config, so `Hyprland --verify-config` and `hyprctl reload` both report `ok`.
5. **`hyprctl reload` re-reads the config; it does not re-run `exec-once`.**
   After restoring one, the services it starts are still dead. `hyprctl
   dispatch exec` restarts them parented to the compositor, as `exec-once`
   does.
6. **Do not use `pgrep -f <pattern>` to decide whether something is running.**
   It matches your own shell's command line, which produced five consecutive
   false "running" results during that incident. Use `pgrep -x`, or
   `ps -eo comm=` — remembering that `comm` truncates to 15 characters, which
   is how `polkit-mate-authentication-agent-1` reads as `polkit-mate-aut`.

## Touching a machine's boot path

`/usr` being read-only and `bootc rollback` existing make most mistakes on an
APEX box recoverable. The boot path is the exception: there is no rollback for
an ESP you overwrote or an EFI variable you replaced, because the thing that
would perform the rollback is what you broke.

The katana is a development machine running APEX (`VARIANT_ID=gaming`,
composefs root, GRUB 2.12, TPM2 present, Secure Boot disabled). It is also the
build box. Bricking it does not cost an afternoon; it costs every remaining
phase.

1. **Boot v2 work runs against guest ESPs only.** On any real APEX host —
   laptop or katana — never run `bootctl install`, `bootctl update`,
   `bootupctl`, `grub2-install`, `grub2-mkconfig -o` against a live
   `/boot/grub2/grub.cfg`, or `efibootmgr -c`/`-B`/`-o`. Never write under
   `/boot`, `/boot/efi` or `/efi`.
2. **A VM's ESP is a loopback-mounted image file**, under a scratch directory
   or `/var/lib/apex/bootlab/`, never the host's. Mount it, write it, unmount
   it; if a command needs a `--esp-path`, pass the mountpoint explicitly rather
   than letting the tool discover the host's.
3. **`ukify`, `qemu`, `mkosi` and `virt-install` are build-time tooling, not
   host software.** They arrive in a capsule or a system extension, never
   `rpm-ostree install`, and never as an ad-hoc `dnf` on a host — that is the
   machine drift this contract prohibits, and a build box is exactly where it
   is most tempting.
4. **Secure Boot keys are never generated on, or written to, a real machine's
   firmware by a script in this repo.** Enrollment is a documented, explicitly
   user-initiated path. CI and VMs get ephemeral keys; a private key never
   reaches the repository.
5. **GRUB stays the default for every published image in this generation.**
   §22's own recommendation is to keep it while the OSTree/bootc install path
   depends on it, and to keep it for legacy BIOS regardless. The systemd-boot +
   UKI path is built, tested and shipped as an **opt-in**, and a change that
   makes it the default for `daily`, `gaming-mesa` or `gaming-nvidia` is a
   contract violation, not a milestone.
