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
# Two opt-in behaviours, each off unless the UKI's SIGNED command line asks for
# it. The command line is inside the signed image, so a scenario cannot turn
# these on for a guest it did not build — and the default path stays exactly
# what the three original LUKS scenarios asserted.
CONTROL=0     # also probe a second, by-value-bound volume on /dev/vdc
DO_S3=0       # suspend to RAM with the volume open, and report what came back
for w in $(cat /proc/cmdline); do
    case "$w" in
        apex.bootlab.luks=*)    MODE="${w#apex.bootlab.luks=}" ;;
        apex.bootlab.control=1) CONTROL=1 ;;
        apex.bootlab.s3=1)      DO_S3=1 ;;
    esac
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
# PCR 7 is the Secure Boot policy register: the firmware extends it with PK,
# KEK, db, dbx and the SecureBoot variable itself. It is reported because the
# firmware-change scenario binds a CONTROL volume to this value, and a control
# whose value nobody printed cannot be enrolled against the right number. It is
# also the register that makes the honest limitation legible — PCR 11 is the
# one APEX's policy binds, and these two moving independently is the property
# under test.
if [ -r /sys/class/tpm/tpm0/pcr-sha256/7 ]; then
    say "pcr7=$(cat /sys/class/tpm/tpm0/pcr-sha256/7)"
else
    say "pcr7=<unreadable>"
fi
[ -c /dev/tpmrm0 ] && say "tpm-device=present" || say "tpm-device=absent"

# ── the unlock attempt ─────────────────────────────────────────────────────
attach() {  # attach LABEL DEVICE KEYFILE OPTIONS
    /usr/lib/systemd/systemd-cryptsetup attach "$1" "$2" "$3" "$4" 2>&1 \
        | while read -r line; do say "  cryptsetup: $line"; done
    # The pipeline's exit status is the `while`'s, so the mapper node is what
    # decides success. That is also the stronger test: LUKS2 verifies the
    # unsealed key against the keyslot digest, so a mapper device existing
    # means the TPM released the CORRECT secret, not merely some secret.
    [ -e "/dev/mapper/$1" ]
}

TPM_OPTS="tpm2-device=auto,headless=1"
[ -n "$SIG" ] && TPM_OPTS="$TPM_OPTS,tpm2-signature=$SIG"

# ── the by-value control volume, when one was attached ─────────────────────
#
# The firmware-change scenario's whole argument rests on this. "A firmware
# change did not break the PCR 11 policy" is worth nothing unless the same boot
# also shows that SOMETHING bound to the firmware state DID break — otherwise a
# guest that never noticed the firmware change at all reports the same green.
#
# So /dev/vdc carries a second LUKS2 volume bound by VALUE to PCR 7. Before the
# change both unlock; after it, this one must refuse and /dev/vdb must not. It
# is attached FIRST, so its result is on the console even if the main volume's
# handling later powers the guest off early.
#
# It carries no recovery key on purpose: it is an instrument, not a volume
# anyone is meant to recover, and giving it a fallback would hide the refusal.
if [ "$CONTROL" = 1 ]; then
    if [ -b /dev/vdc ]; then
        if attach apexctl /dev/vdc - "$TPM_OPTS"; then
            say "control-unlock=SUCCESS"
            /usr/lib/systemd/systemd-cryptsetup detach apexctl >/dev/null 2>&1 || true
        else
            say "control-unlock=REFUSED"
        fi
    else
        # Never silent. A control volume that was requested and is not there
        # must not read the same as one that refused.
        say "control-unlock=<no /dev/vdc attached>"
    fi
fi

if attach apexlab /dev/vdb - "$TPM_OPTS"; then
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

    # ── suspend to RAM, with the volume still open ─────────────────────────
    #
    # L-001's suspend/resume criterion, asked as the question a user cares
    # about: after the machine comes back, is the encrypted volume still
    # usable, and does the TPM still answer?
    #
    # The mapper is deliberately NOT detached first. S3 is the case where the
    # volume stays open and the master key stays in kernel memory across the
    # sleep — that is the property, and closing it first would be testing a
    # reboot with extra steps.
    #
    # Two claims are then separated, because they can fail independently:
    #   post-resume-marker  the still-open mapper still decrypts. The key
    #                       survived the sleep in kernel memory.
    #   post-resume-tpm     a FRESH unseal works after resume. That is the TPM
    #                       itself answering again, which on real hardware is
    #                       the part vendors get wrong (Shutdown/Startup(STATE)
    #                       across S3), and it is a different question from
    #                       whether dm-crypt kept its key.
    if [ "$DO_S3" = 1 ]; then
        if [ ! -w /sys/power/state ]; then
            say "s3=<no /sys/power/state>"
        elif ! grep -q mem /sys/power/state 2>/dev/null; then
            say "s3=<kernel offers no mem state: $(cat /sys/power/state 2>/dev/null)>"
        else
            # The counter is read before and after and BOTH are reported. A
            # single "after" value proves nothing: the kernel may have
            # suspended successfully at some earlier point, or the file may not
            # exist and be read as empty. The delta is the observation.
            S3_BEFORE=$(cat /sys/power/suspend_stats/success 2>/dev/null || echo "?")
            say "s3-attempt before=$S3_BEFORE"
            sync
            # This blocks until something resumes the machine. Under the lab
            # that is files/scripts/boot-v2/qmp-wake.py issuing system_wakeup;
            # on real hardware it is the power button. If nothing wakes it, the
            # guest never returns and vm_boot's timeout kills it — which is why
            # the waker is started before qemu, not after.
            S3_ERR=$( { echo mem > /sys/power/state ; } 2>&1 )
            S3_RC=$?
            S3_AFTER=$(cat /sys/power/suspend_stats/success 2>/dev/null || echo "?")
            if [ "$S3_RC" != 0 ]; then
                # COULD-NOT-RUN, with the reason, and NEVER as a failure: a
                # kernel that refuses S3 has not disproved anything about LUKS.
                say "s3=REFUSED rc=$S3_RC err=$S3_ERR"
            elif [ "$S3_BEFORE" = "$S3_AFTER" ]; then
                # The write returned 0 and the counter did not move. That is
                # the case where the kernel accepted the request and did not
                # actually suspend, and it must not read as a resume.
                say "s3=NO-OP before=$S3_BEFORE after=$S3_AFTER"
            else
                say "s3=resumed before=$S3_BEFORE after=$S3_AFTER"
                # The same read that proved the volume across a REBOOT, now
                # across a SLEEP. Same string, same meaning, different event.
                POST=""
                read -r POST < /dev/mapper/apexlab 2>/dev/null || true
                case "$POST" in
                    "$MARKER"*) say "post-resume-marker=found" ;;
                    *)          say "post-resume-marker=LOST" ;;
                esac
                # And now the TPM, from scratch: detach and unseal again.
                /usr/lib/systemd/systemd-cryptsetup detach apexlab >/dev/null 2>&1 || true
                if attach apexlab /dev/vdb - "$TPM_OPTS"; then
                    say "post-resume-tpm-unlock=SUCCESS"
                else
                    say "post-resume-tpm-unlock=REFUSED"
                fi
            fi
        fi
    fi

    /usr/lib/systemd/systemd-cryptsetup detach apexlab >/dev/null 2>&1 || true
    sync
else
    say "tpm-unlock=REFUSED"
    # The recovery path, exercised in the SAME boot that was refused. "It
    # refuses" and "it is recoverable" as two separate green checks that never
    # met would not be the property a user needs.
    if [ -r /apex-bootlab-recovery-key ]; then
        if attach apexlab /dev/vdb /apex-bootlab-recovery-key headless=1; then
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
