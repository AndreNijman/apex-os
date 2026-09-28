#!/bin/bash
# Start the boot splash in the owner's accent. See rime-plymouth-theme.
#
# dracut sources this file with $moddir, $initdir and $systemdsystemunitdir
# already set, and calls check()/depends()/install() itself — so shellcheck sees
# unassigned variables and unreachable functions that are neither.
# shellcheck disable=SC2154,SC2317
check() {
    return 0
}
depends() {
    echo "plymouth"
    return 0
}
install() {
    inst_multiple awk head sed grep mount umount mkdir rmdir udevadm dirname
    instmods vfat nls_iso8859-1
    inst_simple "$moddir/rime-plymouth-theme" /usr/bin/rime-plymouth-theme
    inst_simple "$moddir/rime-plymouth-theme.service" \
        "$systemdsystemunitdir/rime-plymouth-theme.service"
    mkdir -p "$initdir/$systemdsystemunitdir/sysinit.target.wants"
    ln -sf ../rime-plymouth-theme.service \
        "$initdir/$systemdsystemunitdir/sysinit.target.wants/rime-plymouth-theme.service"
    # dracut's plymouth module installs only the DEFAULT theme; the 24 accent
    # themes have to be carried here or the choice has nothing to choose from.
    local t f
    for t in /usr/share/plymouth/themes/rime-os-accent-*; do
        [ -d "$t" ] || continue
        for f in "$t"/*; do inst_simple "$f"; done
    done
    return 0
}
