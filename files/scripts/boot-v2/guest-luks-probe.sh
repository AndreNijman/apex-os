#!/bin/sh
# ─────────────────────────────────────────────────────────────────────────────
#  guest-luks-probe.sh — runs INSIDE the real APEX initramfs, at dracut's
#  pre-mount hook point, and reports what the TPM did or refused to do.
#
#  It is a repository file rather than a heredoc inside run-scenarios so that
#  ShellCheck and `sh -n` can see it, and so the three LUKS scenarios share one
#  implementation: they differ only in the UKI's signed command line and in
#  which key signed the UKI's PCR policy.
#
#  Everything it needs is already in the APEX initramfs — measured, not
#  assumed: the dracut module list contains systemd-cryptsetup and
#  systemd-pcrphase, and the image carries /usr/bin/cryptsetup,
#  /usr/lib/systemd/systemd-cryptsetup, libcryptsetup and the libtss2 stack.
#  Nothing is copied in.
#
#  POSIX sh, because that is what dracut's hook interpreter is.
# ─────────────────────────────────────────────────────────────────────────────

# /dev/console AND <0>/dev/kmsg. dracut-pre-mount.service is
# StandardOutput=syslog, so stdout goes to the journal of a machine that is
# about to power off; and an unprefixed kmsg write is KERN_WARNING, which
# Fedora's CONFIG_CONSOLE_LOGLEVEL_DEFAULT=4 filters off the console.
say() {
    printf 'APEX-BOOTLAB: %s\n' "$*" > /dev/console 2>/dev/null || true
    printf '<0>APEX-BOOTLAB: %s\n' "$*" > /dev/kmsg 2>/dev/null || true
}

MODE=unknown
for w in $(cat /proc/cmdline); do
    case "$w" in apex.bootlab.luks=*) MODE="${w#apex.bootlab.luks=}" ;; esac
done

say "apex-initramfs-reached"
say "luks-mode=$MODE"

# ── what the stub handed us ────────────────────────────────────────────────
# sd-stub extracts the UKI's .pcrsig and .pcrpkey into /.extra/, and systemd's
# own tmpfiles then copies them to /run/systemd/. Both are checked, and which
# one was found is reported: "the signature is missing" and "the signature did
# not satisfy the policy" are different failures and must not be confused.
SIG=""
for c in /run/systemd/tpm2-pcr-signature.json /.extra/tpm2-pcr-signature.json; do
    [ -r "$c" ] && { SIG="$c"; break; }
done
if [ -n "$SIG" ]; then
    say "pcr-signature=$SIG"
else
    say "pcr-signature=<absent>"
fi
for c in /run/systemd/tpm2-pcr-public-key.pem /.extra/tpm2-pcr-public-key.pem; do
    [ -r "$c" ] && { say "pcr-pubkey=$c"; break; }
done

# ── the measured state ─────────────────────────────────────────────────────
# PCR 11 straight out of sysfs, no tpm2-tools needed. This is the value the
# signed policy is about: sd-stub measures the UKI's sections into it and
# systemd-pcrphase extends it again for each boot phase.
if [ -r /sys/class/tpm/tpm0/pcr-sha256/11 ]; then
    say "pcr11=$(cat /sys/class/tpm/tpm0/pcr-sha256/11)"
else
    say "pcr11=<unreadable>"
fi
[ -c /dev/tpmrm0 ] && say "tpm-device=present" || say "tpm-device=absent"

# ── the unlock attempt ─────────────────────────────────────────────────────
attach() {  # attach LABEL KEYFILE OPTIONS
    /usr/lib/systemd/systemd-cryptsetup attach "$1" /dev/vdb "$2" "$3" 2>&1 \
        | while read -r line; do say "  cryptsetup: $line"; done
    # The pipeline's exit status is the `while`'s, so the mapper node is what
    # decides success. That is also the stronger test: LUKS2 verifies the
    # unsealed key against the keyslot digest, so a mapper device existing
    # means the TPM released the CORRECT secret, not merely some secret.
    [ -e "/dev/mapper/$1" ]
}

TPM_OPTS="tpm2-device=auto,headless=1"
[ -n "$SIG" ] && TPM_OPTS="$TPM_OPTS,tpm2-signature=$SIG"

if attach apexlab - "$TPM_OPTS"; then
    say "tpm-unlock=SUCCESS"
    # Prove the plaintext is real, and that it is the SAME volume across boots.
    # A marker written on the first successful unlock and read back after the
    # "kernel update" boot is what distinguishes "a device appeared" from "this
    # disk decrypted".
    # ── the plaintext marker, with no dd ──
    #
    # The APEX initramfs has `cat` and `tr` but NOT `dd` — measured the hard
    # way: three attempts at this used dd, and `2>/dev/null` reported the
    # resulting "command not found" as a successful write. So the read and the
    # write are done with shell redirection only.
    #
    # A dm-crypt mapper is a block device, so a write must be a whole multiple
    # of the sector size. `printf '%-511s\n'` pads the marker with spaces to
    # 511 characters and adds a newline: exactly 512 bytes, emitted by one
    # printf, and the trailing newline is what lets `read` stop after one
    # sector instead of scanning 64 MB for a line terminator.
    MARKER="APEX-BOOTLAB-PLAINTEXT-MARKER"
    EXISTING=""
    read -r EXISTING < /dev/mapper/apexlab 2>/dev/null || true
    case "$EXISTING" in
        "$MARKER"*)
            say "plaintext-marker=found" ;;
        *)
            # `sync` afterwards because the guest ends with `poweroff -f`,
            # which does not flush the page cache.
            if printf '%-511s\n' "$MARKER" > /dev/mapper/apexlab 2>/dev/null; then
                sync
                say "plaintext-marker=written"
            else
                say "plaintext-marker=WRITE-FAILED"
            fi ;;
    esac
    /usr/lib/systemd/systemd-cryptsetup detach apexlab >/dev/null 2>&1 || true
    sync
else
    say "tpm-unlock=REFUSED"
    # The recovery path, exercised in the SAME boot that was refused. "It
    # refuses" and "it is recoverable" as two separate green checks that never
    # met would not be the property a user needs.
    if [ -r /apex-bootlab-recovery-key ]; then
        if attach apexlab /apex-bootlab-recovery-key headless=1; then
            say "recovery-unlock=SUCCESS"
            /usr/lib/systemd/systemd-cryptsetup detach apexlab >/dev/null 2>&1 || true
        else
            say "recovery-unlock=FAILED"
        fi
    else
        say "recovery-unlock=<no key file in the initrd>"
    fi
fi

say "clean-poweroff"
poweroff -f 2>/dev/null || reboot -f -p 2>/dev/null || echo o > /proc/sysrq-trigger
