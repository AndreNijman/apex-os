#!/bin/bash
# ─────────────────────────────────────────────────────────────────────────────
#  99apex-unlock-hint — a dracut module whose whole job is to make the disk
#  unlock prompt able to explain itself.
#
#  Plymouth's message() was an empty function in both APEX themes, so
#  `plymouth message --text=...` was accepted and thrown away. That is fixed in
#  the themes; this is the other half — somebody has to CALL it, and the only
#  place that can, before the root filesystem exists, is the initramfs.
#
#  It is unconditional at BUILD time (check() returns 0) and conditional at RUN
#  time: the hint script exits silently on a machine whose command line has no
#  rd.luks.*, so an unencrypted install gains nothing on screen and pays one
#  process at boot.
# ─────────────────────────────────────────────────────────────────────────────

# dracut sources this file with $moddir, $initdir and $systemdsystemunitdir
# already set, and calls check()/depends()/install() itself — so shellcheck sees
# unassigned variables and dead functions where there are neither. Every dracut
# module in the tree has this shape.
# shellcheck disable=SC2154,SC2317
check() {
    return 0
}

depends() {
    echo "plymouth"
    return 0
}

install() {
    inst_simple "$moddir/apex-unlock-hint" /usr/bin/apex-unlock-hint
    inst_simple "$moddir/apex-unlock-hint.service" \
        "$systemdsystemunitdir/apex-unlock-hint.service"
    # The symlink is made by hand rather than with `systemctl --root enable`:
    # the unit's [Install] section would be the only thing deciding whether it
    # runs, and a WantedBy= that resolves to nothing is the silent-no-op shape
    # this repository keeps finding. A literal symlink either exists in the
    # initramfs or does not, and the image build asserts which.
    mkdir -p "$initdir/$systemdsystemunitdir/initrd.target.wants"
    ln -sf ../apex-unlock-hint.service \
        "$initdir/$systemdsystemunitdir/initrd.target.wants/apex-unlock-hint.service"

    # ── the per-machine keymap, for the day the command line is signed ──────
    # Same module because it is the same problem: an initramfs built once for
    # every machine has to be told this machine's keyboard layout before it
    # draws the passphrase prompt. Today that arrives as a kernel argument.
    # Inside a UKI the command line is part of the signed image, so the channel
    # becomes a systemd credential from the ESP — and a credential LOSES to the
    # /etc/vconsole.conf dracut bakes in, which is why a script is needed and
    # not just a .cred file. See apex-vconsole-credential's own header.
    #
    # sysinit.target.wants, NOT initrd.target.wants: systemd-vconsole-setup is
    # ordered Before=sysinit.target, so a unit pulled in by initrd.target would
    # run long after the keymap it is trying to set had already been loaded.
    inst_simple "$moddir/apex-vconsole-credential" /usr/bin/apex-vconsole-credential
    inst_simple "$moddir/apex-vconsole-credential.service" \
        "$systemdsystemunitdir/apex-vconsole-credential.service"
    mkdir -p "$initdir/$systemdsystemunitdir/sysinit.target.wants"
    ln -sf ../apex-vconsole-credential.service \
        "$initdir/$systemdsystemunitdir/sysinit.target.wants/apex-vconsole-credential.service"
    return 0
}
