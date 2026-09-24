#!/bin/bash
# Start the boot splash in the owner's accent. See apex-plymouth-theme.
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
    inst_simple "$moddir/apex-plymouth-theme" /usr/bin/apex-plymouth-theme
    inst_simple "$moddir/apex-plymouth-theme.service" \
        "$systemdsystemunitdir/apex-plymouth-theme.service"
    mkdir -p "$initdir/$systemdsystemunitdir/sysinit.target.wants"
    ln -sf ../apex-plymouth-theme.service \
        "$initdir/$systemdsystemunitdir/sysinit.target.wants/apex-plymouth-theme.service"
    # dracut's plymouth module installs only the DEFAULT theme; the 24 accent
    # themes have to be carried here or the choice has nothing to choose from.
    local t f
    for t in /usr/share/plymouth/themes/apex-os-accent-*; do
        [ -d "$t" ] || continue
        for f in "$t"/*; do inst_simple "$f"; done
    done
    return 0
}
