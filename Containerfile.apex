# ─────────────────────────────────────────────────────────────────────────
# APEX-OS — the image. One image, every laptop.
#
#   FROM apex-base → stamp the edition, set the boot splash, rebuild the
#   initramfs that ships. That is all this tier does.
#
# ═══ WHY THERE IS ONLY ONE OF THESE NOW ═══
#
# APEX used to publish three: `daily`, `gaming-mesa`, `gaming-nvidia`. They were
# the same operating system with different payloads bolted on, and the split
# forced a decision at INSTALL time that nobody has the information to make.
# Someone with a gaming laptop who plays occasionally had to pick, and picking
# Daily locked them out of the GPU driver their machine came with — recoverable
# only by a rebase, which is not a thing a normal person does. Three images also
# meant three CI legs, three sets of verification, and two of them idle on any
# given machine forever.
#
# The replacement rule is not "put everything in one image and hope". It is a
# hard technical line:
#
#     A kernel module cannot be installed at runtime under Secure Boot.
#     Userspace can.
#
# Signing a module the kernel will accept needs the APEX MOK private key, which
# is a CI secret and must never reach a user's machine — `apex-pkg` refuses
# kernels, kernel modules and akmods for exactly this reason. So:
#
#   ships in the image, built and signed in CI  |  installs on demand
#   ------------------------------------------- | ---------------------------
#   NVIDIA akmod + userspace driver             |  Steam, Proton
#   xone / xpadneo controller akmods            |  gamescope, MangoHud
#                                               |  Sunshine, OBS
#
#     sudo apex install steam gamescope mangohud gamemode
#
# and none of that touches the deployment — `apex install` builds a systemd
# system extension, so `bootc upgrade` keeps working (see docs/packages.md).
#
# Verified against the repositories this image actually enables, rather than
# assumed: steam resolves from RPMFusion nonfree; gamescope, mangohud, gamemode,
# obs-studio, steam-devices and protontricks from Fedora. Proton itself ships
# inside Steam. SUNSHINE IS THE EXCEPTION and is named as one — it is in no
# enabled repository, so it needs `apex repo enable-copr` or a downloaded .rpm
# (which `apex install` also takes). One command for the rest, two for Sunshine.
#
# Everything in the left-hand column lives in Containerfile.core, NOT here and
# NOT in the base. That is a download decision, and it is the reason a single
# image is affordable at all: the base consumes core as `FROM …@sha256:…`, so
# those layer descriptors are reused verbatim and bootc downloads none of them
# on a normal update. The NVIDIA driver is fetched once, by digest, forever.
#
# WHAT THIS TIER STILL EXISTS FOR: the Plymouth theme has to be set, and setting
# it requires the LAST initramfs regeneration of the build — after core's NVIDIA
# dracut configuration and after the base's content. That ordering is the tier.
# ─────────────────────────────────────────────────────────────────────────
ARG BASE=localhost/apex-base:latest
FROM ${BASE}

# ── Edition stamp ──────────────────────────────────────────────────────────
# There is one edition now, so it is named after the product rather than after
# one of the three things it replaced. VARIANT_ID=apex, not `daily`: keeping
# `daily` would have been a free ride through the greeter's existing branding
# switch, and it would also be a lie told by every machine's /etc/os-release —
# this image is what `:gaming-nvidia` resolves to as well.
#
# /etc/os-release is a symlink into /usr/lib/os-release, so append there. (The
# base deliberately leaves this unset.) apex-greet maps `apex` to the chartreuse
# accent; /etc/apex-greet/edition still overrides it per machine.
RUN set -eux; \
    printf 'VARIANT="APEX"\nVARIANT_ID=apex\n' >> /usr/lib/os-release; \
    grep -q '^VARIANT_ID=apex$' /usr/lib/os-release

# ── Gaming Mode: the session, without the payload ──────────────────────────
# gamescope and Steam are on-demand packages now, so this session cannot be
# offered unconditionally — a greeter entry that bounces the user straight back
# to the greeter is worse than no entry. `TryExec=/usr/bin/gamescope` is the
# desktop-entry spec's own mechanism for exactly this, and apex-greet's
# enumeration now honours it: the entry is INVISIBLE until `apex install
# gamescope steam` puts the binary in place, and appears by itself afterwards.
# That is why the session's own files ship here regardless — they are shell
# scripts and a sudoers rule, they cost nothing, and provisioning them at
# install time would mean writing to a read-only /usr.
#
# What the session gives, once its packages are present: gamescope presenting
# directly to KMS with no Hyprland, no bar and nothing composited over the game;
# `apex game start` on entry, which is what finally makes the CachyOS kernel's
# sched-ext support do something (scx_lavd) alongside the cpuset, IRQ steering,
# GPU clock locks and performance tier.
#
# scx_loader is NOT enabled here, and that is deliberate rather than an
# oversight: its unit is `Type=dbus` with `BusName=org.scx.Loader`, and
# /usr/share/dbus-1/system-services/org.scx.Loader.service names
# `SystemdService=scx_loader.service`. It is D-Bus activated, so the first
# scxctl call from apexd starts it. Enabling it would add a daemon to every
# boot on every laptop to save an activation that systemd already performs.
#
# REALTIME: this drop-in raises RLIMIT_RTPRIO through pam_limits, which greetd's
# PAM stack does run, and that part works — katana 2026-09-19 measured the soft
# limit at 20 in a real Gaming Mode session.
#
# It is NOT enough for gamescope's `--rt`, and the comment that used to sit here
# said it was. gamescope gates realtime on CAP_SYS_NICE, not on the rlimit, so
# with the drop-in installed and the flag passed it still logged "No CAP_SYS_NICE,
# falling back to regular-priority compute and threads" on every attempt. The
# session script now checks the capability and passes `--rt` only when it is
# actually reachable — see its own header for why nothing here grants it
# (setcap cannot write a read-only composefs /usr, and pam_cap would break
# Steam's bwrap).
COPY --chmod=0755 files/system/libexec/apex-gaming-session \
     /usr/libexec/apex-gaming-session
COPY --chmod=0755 files/system/libexec/apex-session-select \
     /usr/libexec/apex-session-select
COPY files/system/sudoers/apex-session-select \
     /etc/sudoers.d/040-apex-session-select
COPY files/system/limits/30-apex-gaming-rtprio.conf \
     /etc/security/limits.d/30-apex-gaming-rtprio.conf
COPY files/desktop/wayland-sessions/apex-gaming.desktop \
     /usr/share/wayland-sessions/apex-gaming.desktop
RUN set -eux; \
    # sudoers is parsed by a setuid binary: a syntax error or wrong mode is
    # refused at RUNTIME, i.e. a password prompt no compositor can answer.
    chown root:root /etc/sudoers.d/040-apex-session-select; \
    chmod 0440 /etc/sudoers.d/040-apex-session-select; \
    visudo -cf /etc/sudoers.d/040-apex-session-select; \
    bash -n /usr/libexec/apex-gaming-session; \
    bash -n /usr/libexec/apex-session-select; \
    # The gate that keeps the entry hidden until its packages exist. Without
    # this line the session is offered on every machine and fails on almost all
    # of them, which is the regression this whole arrangement exists to avoid.
    grep -q '^TryExec=/usr/bin/gamescope$' \
        /usr/share/wayland-sessions/apex-gaming.desktop \
        || { echo "FATAL: apex-gaming.desktop has no TryExec gate; it would be offered without gamescope"; exit 1; }; \
    # ...and the greeter must actually honour TryExec, or the gate is decorative.
    grep -q 'TryExec=' /usr/share/apex-greet/GreetContext.qml \
        || { echo "FATAL: apex-greet does not filter on TryExec"; exit 1; }; \
    # Prove it against the shipped artifact rather than trusting the two greps:
    # run the greeter's OWN enumeration and require apex-gaming to be absent
    # while gamescope is not installed.
    ! command -v gamescope >/dev/null \
        || { echo "FATAL: gamescope is baked in; it is meant to be on-demand"; exit 1; }; \
    ! command -v steam >/dev/null \
        || { echo "FATAL: steam is baked in; it is meant to be on-demand"; exit 1; }; \
    ENUM='for f in /usr/share/wayland-sessions/*.desktop; do [ -r "$f" ] || continue; id=$(basename "$f" .desktop); tryexec=$(sed -n "s/^TryExec=//p" "$f" | head -n1); if [ -n "$tryexec" ] && ! command -v "$tryexec" >/dev/null 2>&1; then continue; fi; exec=$(sed -n "s/^Exec=//p" "$f" | head -n1); [ -n "$exec" ] || continue; printf "%s\n" "$id"; done'; \
    got="$(sh -c "${ENUM}" | tr '\n' ' ')"; \
    echo "greeter would offer: ${got}"; \
    case "${got}" in *apex-gaming*) echo "FATAL: apex-gaming offered without gamescope"; exit 1 ;; esac; \
    case "${got}" in *hyprland*) ;; *) echo "FATAL: hyprland session missing"; exit 1 ;; esac; \
    case "${got}" in *niri*) ;; *) echo "FATAL: niri session missing"; exit 1 ;; esac; \
    # THE LOCKOUT GUARD, kept verbatim from the retired Containerfile.gaming.
    # Sessions are enumerated by a sorted glob and the greeter's index defaults
    # to 0, so `apex-gaming.desktop` sorts BEFORE hyprland/niri and would become
    # the default session the moment a user installs gamescope. That is precisely
    # how hyprland-uwsm.desktop locked users out (see Containerfile.base). The
    # greeter picks its default BY NAME, and this asserts that it still does.
    grep -q 'defaultSession: "hyprland"' /usr/share/apex-greet/GreetContext.qml \
        || { echo "FATAL: greeter no longer names its default session; apex-gaming would sort first and become the default"; exit 1; }; \
    grep -q 'ctx.defaultSession' /usr/share/apex-greet/GreetContext.qml; \
    # ── The picker's WORDING, swept over the complete set ──────────────────
    # Every session entry exists by now: niri and hyprland from Containerfile
    # .base, apex-labwc from the labwc stage, apex-gaming from the COPY above.
    # BASE-013's third criterion is that compositor names stay implementation
    # details for normal users, and `Name=` is the only line of these files a
    # user ever reads — the greeter renders it verbatim at every login. A
    # regression here is silent: the session still works, it just tells every
    # user which compositor project APEX vendored. The sweep is over the whole
    # directory rather than the files this repo owns, so the next package that
    # drops a session entry in is caught by the build instead of by a user.
    # `swept` is not decoration. A sweep over a directory that turned out to be
    # empty — a moved path, a COPY that landed a tier later — would print
    # nothing and exit 0, and this gate would be a build step that cannot fail.
    # The count is asserted afterwards, and the whole block is what
    # tests/test-apex-greet-sessions.sh lifts out and runs.
    swept=0; \
    for f in /usr/share/wayland-sessions/*.desktop; do \
        n="$(sed -n 's/^Name=//p' "$f" | head -n1 | tr 'A-Z' 'a-z')"; \
        [ -n "$n" ] || { echo "FATAL: $f has no Name="; exit 1; }; \
        case "$n" in \
            *labwc*|*hyprland*|*niri*|*sway*|*wlroots*|*openbox*|*gamescope*|*wayfire*|*river*) \
                echo "FATAL: $f shows users the compositor's name: ${n}"; exit 1 ;; \
        esac; \
        echo "session wording: $(basename "$f" .desktop) -> $(sed -n 's/^Name=//p' "$f" | head -n1)"; \
        swept=$((swept + 1)); \
    done; \
    [ "$swept" -ge 4 ] || { echo "FATAL: the wording sweep saw $swept session entries; it expected at least 4 (niri, hyprland, apex-labwc, apex-gaming)"; exit 1; }; \
    echo "session picker: $swept entries checked, none names a compositor"; \
    echo "gaming session: shipped, TryExec-gated, hidden until its packages are installed"

# ── The image carries its GPU drivers; assert it, here, at the end ─────────
# These are built in Containerfile.core, four tiers down and possibly weeks ago
# in a digest-pinned parent. A base rebuilt on the WRONG core is the exact
# failure this catches, and it is not hypothetical — a stale local core is how a
# validation build in this project reported green against an image that did not
# contain the change under test.
RUN set -eux; \
    KVER="$(cat /usr/lib/apex-kver)"; \
    MODDIR="/usr/lib/modules/${KVER}"; \
    OOT=""; for d in extra updates; do [ -d "${MODDIR}/${d}" ] && OOT="${OOT} ${MODDIR}/${d}"; done; \
    test -n "${OOT}" || { echo "FATAL: no out-of-tree modules in this image at all"; exit 1; }; \
    for pat in nvidia xone xpadneo; do \
        test "$(find ${OOT} -type f -name "*${pat}*.ko*" | wc -l)" -gt 0 \
            || { echo "FATAL: ${pat} module missing — this image was built on a core without it"; exit 1; }; \
    done; \
    test -e /usr/lib64/libGLX_nvidia.so.0; \
    test -e /usr/lib64/libGLX_mesa.so.0; \
    test -s /usr/lib/modprobe.d/50-apex-nvidia.conf; \
    echo "gpu: nvidia + controller modules present, mesa intact"

# ═══════════════════════════════════════════════════════════════════════════
# M2 — branding (Plymouth). The shared M2 concerns (APEX Shell first-boot
# provisioner, greeter session curation) + M3 (apexd) all live in the BASE.
# ═══════════════════════════════════════════════════════════════════════════

# ── Plymouth boot splash: apex-os-chartreuse ───────────────────────────────
# One image, one splash. Chartreuse is the general-purpose identity and the one
# that matches the greeter accent above; the gold theme was the Gaming edition's
# and has no edition left to belong to.
#
# Copy the theme, set it as the Plymouth default, then rebuild the CachyOS
# initramfs so the theme is actually present in the initrd bootc boots from
# (/usr/lib/modules/<kver>/initramfs.img). `plymouth-set-default-theme` WITHOUT
# -R only sets the default; we run dracut ourselves (mirroring core's Stage-1
# flags) because on bootc the initramfs is regenerated in-image, not by the
# tool's -R host path. The theme uses `ModuleName=script`, so the script plugin
# (plymouth-plugin-script) is required — it is installed in Containerfile.core.
#
# THIS IS THE LAST INITRAMFS REGENERATION IN THE BUILD, which is why core's
# 99-nvidia-dracut.conf has to be in place before it: that file is what puts
# nvidia/nvidia_modeset/nvidia_drm/nvidia_uvm INTO this initramfs. Asserted
# below against the produced image, not assumed from the config file.
#
# Keeping the theme switch in this tier rather than core is a download
# optimisation: every dnf transaction rewrites the ~200 MB sqlite rpmdb into its
# layer, so installing plymouth here would cost an rpmdb-sized layer on every
# build. With the engine in core, all this layer holds is a theme plus a
# --reproducible dracut run: unchanged theme + unchanged kernel yield a
# byte-identical initramfs and the layer digest stops moving.
COPY files/branding/plymouth/apex-os-chartreuse \
     /usr/share/plymouth/themes/apex-os-chartreuse
RUN set -eux; \
    command -v plymouth-set-default-theme >/dev/null \
        || { echo "FATAL: plymouth engine missing from core"; exit 1; }; \
    plymouth-set-default-theme apex-os-chartreuse; \
    KVER="$(cat /usr/lib/apex-kver)"; \
    dracut --force --no-hostonly --reproducible --zstd \
        --add plymouth --kver "${KVER}" \
        "/usr/lib/modules/${KVER}/initramfs.img"; \
    test -s "/usr/lib/modules/${KVER}/initramfs.img"; \
    # The initramfs is the artifact that boots, so it is the artifact checked.
    # `lsinitrd` lists what actually landed inside; a driver that is configured
    # but absent here is a black screen on a dGPU-attached panel.
    # Modules are listed by full path and are compressed, so match the path
    # segment and allow any suffix: `.../extra/nvidia/nvidia.ko.zst`. Verified
    # against a real initramfs before this was written, rather than pattern-
    # matching against an assumption about the filenames.
    lsinitrd "/usr/lib/modules/${KVER}/initramfs.img" \
        | grep -E '/nvidia[^/]*\.ko' \
        || { echo "FATAL: nvidia is not in the shipped initramfs — a dGPU-attached panel would stay black"; exit 1; }; \
    # ── the disk-unlock chain, checked in the artifact that boots ──────────
    # This is the LAST dracut run of the build (see the tier's header), so it
    # is the only place these can be checked: an assertion before it inspects
    # an initramfs that is about to be replaced.
    #
    # Every one of these is a machine that asks for a passphrase it cannot use:
    #   systemd-cryptsetup  — nothing opens the volume at all;
    #   the crypt module    — no rd.luks.* handling, so nothing tries;
    #   the tpm2 token .so  — a TPM-enrolled keyslot is unusable and the boot
    #                         falls back to the passphrase prompt silently;
    #   the keymaps         — every prompt is a `us` prompt, whatever the owner
    #                         chose, which is the stranding case for anyone
    #                         whose passphrase uses a key that moved.
    # The keymap check names a NON-us map on purpose: `us` is the kernel
    # built-in and would be there even if the whole keymap tree were missing.
    INITRD="/usr/lib/modules/${KVER}/initramfs.img"; \
    lsinitrd "$INITRD" | grep -qE 'usr/bin/systemd-cryptsetup' \
        || { echo "FATAL: systemd-cryptsetup is not in the shipped initramfs — an encrypted root could never be opened"; exit 1; }; \
    lsinitrd "$INITRD" -m | tr ' ' '\n' | grep -qx 'crypt' \
        || { echo "FATAL: dracut's crypt module is not in the shipped initramfs — rd.luks.* would be ignored"; exit 1; }; \
    lsinitrd "$INITRD" | grep -qE 'libcryptsetup-token-systemd-tpm2\.so' \
        || { echo "FATAL: the systemd-tpm2 token plugin is not in the shipped initramfs — a TPM keyslot would be unusable"; exit 1; }; \
    lsinitrd "$INITRD" | grep -qE 'kbd/keymaps/.*/de\.map' \
        || { echo "FATAL: the console keymaps are not in the shipped initramfs — every unlock prompt would be a US one"; exit 1; }; \
    # The hint module. A dracut module whose check() is never called, or whose
    # install() silently failed, leaves NO trace and no error — the build would
    # look identical. So both halves are named: the program, and the symlink
    # that is the only reason systemd ever runs it.
    lsinitrd "$INITRD" | grep -qE 'usr/bin/apex-unlock-hint' \
        || { echo "FATAL: apex-unlock-hint is not in the shipped initramfs — the unlock prompt could not say which keyboard it is using"; exit 1; }; \
    lsinitrd "$INITRD" | grep -qE 'initrd\.target\.wants/apex-unlock-hint\.service' \
        || { echo "FATAL: apex-unlock-hint.service is in the initramfs but nothing pulls it in — it would never run"; exit 1; }; \
    #
    # The per-machine keymap channel that survives a signed UKI. Same module,
    # same two halves, and the SAME trap in a sharper form: this unit must be
    # pulled in by sysinit.target.wants and not initrd.target.wants, because
    # systemd-vconsole-setup is ordered Before=sysinit.target and a unit that
    # ran at initrd.target would set a keymap that had already been loaded.
    # A wrong .wants directory here is a silent no-op that only shows up as a
    # locked-out owner, so the directory is named in the assertion.
    lsinitrd "$INITRD" | grep -qE 'usr/bin/apex-vconsole-credential' \
        || { echo "FATAL: apex-vconsole-credential is not in the shipped initramfs — a UKI machine could not be told its keyboard layout"; exit 1; }; \
    lsinitrd "$INITRD" | grep -qE 'sysinit\.target\.wants/apex-vconsole-credential\.service' \
        || { echo "FATAL: apex-vconsole-credential.service is not pulled in by sysinit.target.wants — it would run after the keymap was already loaded, or never"; exit 1; }; \
    echo "initramfs: plymouth theme + nvidia drivers + the disk-unlock chain present"

# Kernel args for the graphical splash. `quiet splash` per the theme's own
# README (files/branding/plymouth/README.md). NOTE: `quiet` raises the console
# loglevel, so it trims the verbose serial output core's 10-apex-serial.toml
# enables for CI/QEMU observability (warnings/errors still print). Drop `quiet`
# here if noisier boot logs are wanted. bootc merges all kargs.d files.
RUN set -eux; \
    mkdir -p /usr/lib/bootc/kargs.d; \
    printf 'kargs = ["quiet", "splash"]\n' \
        > /usr/lib/bootc/kargs.d/20-apex-plymouth.toml

# Informational lint.
RUN bootc container lint || true

ARG APEX_REVISION=dev
LABEL org.opencontainers.image.title="APEX-OS" \
      org.opencontainers.image.description="APEX-OS — one image for every laptop: NVIDIA + controller drivers built and signed in, gaming userspace on demand" \
      org.opencontainers.image.source="https://github.com/AndreNijman/apex-os" \
      org.opencontainers.image.revision="${APEX_REVISION}" \
      org.opencontainers.image.version="apex" \
      org.opencontainers.image.vendor="APEX-OS"
