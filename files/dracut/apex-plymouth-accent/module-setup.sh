#!/bin/bash
# The owner's matugen accent for the boot splash. See apex-plymouth-accent.
check() {
    return 0
}
depends() {
    echo "plymouth"
    return 0
}
install() {
    inst_multiple awk head
    inst_simple "$moddir/apex-plymouth-accent" /usr/bin/apex-plymouth-accent
    inst_simple "$moddir/apex-plymouth-accent.service" \
        "$systemdsystemunitdir/apex-plymouth-accent.service"
    mkdir -p "$initdir/$systemdsystemunitdir/initrd.target.wants"
    ln -sf ../apex-plymouth-accent.service \
        "$initdir/$systemdsystemunitdir/initrd.target.wants/apex-plymouth-accent.service"
    # plymouth's own module installs the theme's top-level files only; the
    # 24 accent variants live one directory down.
    local theme=/usr/share/plymouth/themes/apex-os-chartreuse
    local f
    for f in "$theme"/accent/*/*.png; do
        [ -f "$f" ] && inst_simple "$f"
    done
    return 0
}
