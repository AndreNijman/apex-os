#!/bin/sh
# ─────────────────────────────────────────────────────────────────────────────
#  guest-luks-enroll.sh — runs INSIDE the real APEX initramfs, at dracut's
#  pre-mount hook point, and runs the REAL SHIPPED enrolment script
#  (files/system/libexec/apex-luks-enroll) against a fresh LUKS2 volume.
#
#  ── why this exists ──
#
#  Every host-side enrol-sb-on/enroll-* scenario, and the by-value control
#  volume `luks-firmware-change` builds for itself, either read Secure Boot
#  state from a FIXTURE (`--probe-root`) or bind PCR 7 to a VALUE CHOSEN BY THE
#  TEST rather than read live from a TPM. Neither proves that
#  `systemd-cryptenroll --tpm2-pcrs=7` — the bare, LIVE-READ form the shipped
#  script actually runs — succeeds when pointed at a TPM whose PCR 7 was JUST
#  extended by a REAL (v)UEFI Secure Boot chain, in the one process where that
#  measurement is still live.
#
#  IT HAS TO BE THIS PROCESS. TPM PCRs are volatile: swtpm's
#  `--flags startup-clear` resets them on every fresh `swtpm socket` start, the
#  same as real hardware resets them on every power cycle. PCR 7 is
#  REPRODUCIBLE across separate boots (same OVMF, same enrolled Secure Boot
#  keys, same signing certificate on whatever gets loaded — PCR 7 measures the
#  CERTIFICATE that verified an image, not the image itself), which is what
#  makes an EARLIER, SEPARATE enrolment still unseal on a LATER boot. But
#  enrolling in the first place needs a LIVE, CURRENT PCR 7 to read — and the
#  only place that value exists outside a real firmware boot is inside a guest
#  that has just finished one. A host-side process cannot reach in: QEMU is the
#  sole client on swtpm's data socket for the guest's whole lifetime.
#
#  So this script, the systemd-cryptenroll binary, python3 (the shipped
#  script's `tpm2_report`/`token_types`/`keyslot_count` parse
#  `cryptsetup luksDump --dump-json-metadata` with it), and `od` (used by the
#  shipped script's `efivar_byte5`) are ALL delivered via --extra-initrd,
#  because none of the four ships in the generic dracut initramfs — measured,
#  not assumed, against a root staged straight off katana's own live install
#  with `lsinitrd`. Every path added is `usr/...`: this initramfs is usrmerged
#  (`lib64 -> usr/lib64`, `bin -> usr/bin`), and a payload cpio that created a
#  REAL `lib64/` directory would shadow the symlink and take every library in
#  the initramfs down with it — the same trap `apex-image`'s hook comment
#  documents for `usr/lib/dracut/hooks`.
#
#  POSIX sh, because that is what dracut's hook interpreter is, same as
#  guest-luks-probe.sh.
# ─────────────────────────────────────────────────────────────────────────────

say() {
    printf 'APEX-BOOTLAB: %s\n' "$*" > /dev/console 2>/dev/null || true
    printf '<0>APEX-BOOTLAB: %s\n' "$*" > /dev/kmsg 2>/dev/null || true
}

say "apex-initramfs-reached"

# ── preconditions, said BEFORE the attempt, so a boot that never reaches the
#    enrolment line still says exactly what was missing ──────────────────────
if command -v systemd-cryptenroll >/dev/null 2>&1; then
    say "have-systemd-cryptenroll=yes"
else
    say "have-systemd-cryptenroll=NO"
fi
if command -v python3 >/dev/null 2>&1; then
    say "have-python3=yes ($(python3 -c 'import sys; print(sys.version.split()[0])' 2>/dev/null))"
else
    say "have-python3=NO"
fi
if [ -x /usr/libexec/apex-luks-enroll ]; then
    say "have-apex-luks-enroll=yes"
else
    say "have-apex-luks-enroll=NO"
fi

# `-x` above inspected the FILE. It says nothing about whether the kernel can
# RUN it, and the difference is not academic: execve() returns ENOENT for a
# missing SHEBANG INTERPRETER, so a perfectly present script reports
# "No such file or directory" about a file that is plainly there. That exact
# pair — `have-apex-luks-enroll=yes` next to `enroll-exit=127` — is what this
# scenario's first real boot produced on 2026-09-21: the shipped script starts
# `#!/usr/bin/env bash`, and the generic APEX initramfs carries `bash` but not
# `env`. So name the interpreter and say whether it is there. Pure shell, no
# sed/cut: a probe for missing commands must not itself need one.
read -r apex_shebang < /usr/libexec/apex-luks-enroll 2>/dev/null || apex_shebang=""
case "$apex_shebang" in
    '#!'*)
        apex_interp="${apex_shebang#\#!}"
        while [ "${apex_interp# }" != "$apex_interp" ]; do apex_interp="${apex_interp# }"; done
        apex_interp_arg="${apex_interp#* }"
        apex_interp="${apex_interp%% *}"
        [ "$apex_interp_arg" = "$apex_interp" ] && apex_interp_arg=""
        if [ -x "$apex_interp" ]; then
            say "enroll-interpreter=$apex_interp present"
        else
            say "enroll-interpreter=$apex_interp MISSING (execve fails ENOENT)"
        fi
        if [ -n "$apex_interp_arg" ]; then
            if command -v "$apex_interp_arg" >/dev/null 2>&1; then
                say "enroll-interpreter-arg=$apex_interp_arg present"
            else
                say "enroll-interpreter-arg=$apex_interp_arg MISSING"
            fi
        fi
        ;;
    *) say "enroll-interpreter=<no shebang read>" ;;
esac
[ -c /dev/tpmrm0 ] && say "tpm-device=present" || say "tpm-device=absent"

# PCR 7 BEFORE enrolling, off sysfs, no tpm2-tools needed. This is what the
# whole point of running enrolment in-guest is FOR: it must be a real
# measurement — not the all-zero value an uninitialised bank reports — or the
# enrolment below is reproducing systemd's own "PCR policy effectively
# unenforced" trap rather than testing past it.
if [ -r /sys/class/tpm/tpm0/pcr-sha256/7 ]; then
    say "pcr7-pre-enroll=$(cat /sys/class/tpm/tpm0/pcr-sha256/7)"
else
    say "pcr7-pre-enroll=<unreadable>"
fi
# Reported for the same reason guest-luks-probe.sh reports it: this UKI (no
# --pcr-key/--pcr-pubkey) still gets measured into PCR 11 by sd-stub, and a
# scenario comparing this boot's PCR 11 against a later boot's DIFFERENT UKI is
# what shows the PCR 7 binding held while PCR 11 moved — the binding really is
# to PCR 7, not to "whatever this UKI happens to measure".
if [ -r /sys/class/tpm/tpm0/pcr-sha256/11 ]; then
    say "pcr11=$(cat /sys/class/tpm/tpm0/pcr-sha256/11)"
else
    say "pcr11=<unreadable>"
fi

# Secure Boot, read the same way the shipped script reads it, so a boot where
# THIS says off and the script's own stderr disagrees is a defect worth seeing.
if [ -r /sys/firmware/efi/efivars/SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c ]; then
    sb_byte5="$(od -An -tu1 -j4 -N1 /sys/firmware/efi/efivars/SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c 2>/dev/null | tr -d ' \n')"
    say "secure-boot-byte=${sb_byte5:-<unreadable>}"
else
    say "secure-boot-byte=<no efivar>"
fi

# NEWPIN exported even though --with-pin is never passed: the shipped script's
# own harness lesson (a mistaken PIN prompt blocks on systemd's ask-password
# SOCKET protocol forever, with no terminal to answer it) applies to any code
# path that could reach --tpm2-with-pin, and an inert value costs nothing.
export NEWPIN=inert-unused-pin

# ── the enrolment itself ────────────────────────────────────────────────────
#
# No --policy: letting `policy_select`'s auto mode fall through to pcr7 IS the
# property under test. If this UKI accidentally satisfies probe_signed_pcr11
# instead (a `--pcr-key`/`--pcr-pubkey` build), the "tpm2:" line will say so —
# read the field, do not assume it.
enroll_out=/run/apex-enroll.out
enroll_err=/run/apex-enroll.err
enroll_rc=0
if command -v timeout >/dev/null 2>&1; then
    timeout 90 /usr/libexec/apex-luks-enroll \
        --device /dev/vdb \
        --unlock-key-file /apex-bootlab-passphrase \
        --recovery-out /run/apex-bootlab-recovery-key \
        --tpm2-device auto \
        >"$enroll_out" 2>"$enroll_err" </dev/null
    enroll_rc=$?
else
    say "enroll-fatal=no timeout(1) in this initramfs, refusing to run unbounded"
    enroll_rc=127
fi
say "enroll-exit=$enroll_rc"

# Relayed line by line, and the two streams kept apart: the contract with
# luks-installer is stdout carries machine lines and stderr carries prose, and
# a scenario that merged them could not tell a machine line from a decline
# explanation that happens to contain a colon.
if [ -r "$enroll_out" ]; then
    while IFS= read -r line; do say "enroll-out: $line"; done < "$enroll_out"
fi
if [ -r "$enroll_err" ]; then
    while IFS= read -r line; do say "enroll-err: $line"; done < "$enroll_err"
fi

# ── enrolling a TPM slot must never become the ONLY way in ──────────────────
#
# L-003 is "TPM auto-unlock by default, WHERE SAFE", and no reading of "safe"
# survives an enrolment that quietly cost the user their passphrase. Prove it
# in THIS boot, against the volume that was just enrolled, rather than
# inferring it from a slot count: a header can list three key slots while one
# of them no longer opens anything. `--test-passphrase` verifies a key against
# the header and needs no device-mapper node, so it runs safely here in the
# initramfs with the real root still unmounted.
if command -v cryptsetup >/dev/null 2>&1; then
    if cryptsetup open --test-passphrase --key-file /apex-bootlab-passphrase \
           /dev/vdb >/dev/null 2>&1; then
        say "passphrase-after-enrol=SUCCESS"
    else
        say "passphrase-after-enrol=FAILED"
    fi
else
    say "passphrase-after-enrol=<no cryptsetup in this initramfs>"
fi

if [ -r /run/apex-bootlab-recovery-key ]; then
    say "recovery-key=$(cat /run/apex-bootlab-recovery-key)"
else
    say "recovery-key=<not written>"
fi

say "clean-poweroff"
poweroff -f 2>/dev/null || reboot -f -p 2>/dev/null || echo o > /proc/sysrq-trigger
