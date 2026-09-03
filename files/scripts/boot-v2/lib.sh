#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  boot-v2 shared helpers — firmware discovery, ESP authoring, guest launch.
#
#  Sourced by the scripts beside it. Everything here operates on files inside a
#  work directory the caller names; nothing in this directory may read or write
#  the host's ESP, `/boot`, `/boot/efi`, `/efi` or EFI variables. See
#  AGENTS.md "Touching a machine's boot path" — the katana is a real APEX
#  machine and the build box at the same time, so a guest ESP is always an
#  image file and `--esp-path`-style arguments are always explicit.
#
#  Two things in here look like over-engineering and are not:
#
#  * Firmware paths are DISCOVERED and the discovery hard-fails. Fedora has
#    moved OVMF between /usr/share/edk2/ovmf and /usr/share/edk2/x64 and
#    between 2 MB `.fd` and 4 MB `.qcow2` layouts across releases;
#    signing/spike-d/boot-sb-vm.sh still hardcodes
#    /usr/share/edk2/x64/OVMF_CODE.secure.4m.fd, which does not exist in the
#    boot lab at all. A wrong path must say so, not produce a confusing qemu
#    error twenty lines later.
#
#  * A guest run is only "successful" when qemu exited 0 under -no-reboot AND
#    the expected marker is in the serial log. Timeout-kill plus a marker means
#    the guest printed and then hung, which §22 explicitly refuses to count:
#    "Do not mark an update successful merely because the kernel started."
# ─────────────────────────────────────────────────────────────────────────────

# Callers set -euo pipefail themselves; this file must be safe to source under it.

BOOTV2_LIB_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export BOOTV2_LIB_DIR

log()  { printf '\n>>> %s\n' "$*" >&2; }
info() { printf '    %s\n' "$*" >&2; }
die()  { printf '!!! %s\n' "$*" >&2; exit 1; }

# ── firmware ────────────────────────────────────────────────────────────────
# Two OVMF variants matter and they are not interchangeable:
#   secboot  — built with SECURE_BOOT_ENABLE and SMM_REQUIRE. Verifies every
#              LoadImage against db. This is the only one that can prove a
#              signing chain, and the only one worth using here.
#   plain    — no verification. Kept out of these scripts deliberately: a run
#              that silently fell back to it would "pass" the signed-UKI
#              scenario while proving nothing.
# virt-fw-vars edits raw `.fd` varstores, so the raw pair is what we need;
# Fedora 43 ships only qcow2 for the 4 MB layout.
ovmf_code_secboot() {
    local c
    for c in /usr/share/edk2/ovmf/OVMF_CODE.secboot.fd \
             /usr/share/edk2/x64/OVMF_CODE.secboot.4m.fd \
             /usr/share/edk2/x64/OVMF_CODE.secure.4m.fd; do
        [[ -f "$c" ]] && { printf '%s\n' "$c"; return 0; }
    done
    die "no Secure Boot OVMF firmware found (looked in /usr/share/edk2/{ovmf,x64})"
}

# The PRISTINE template — the one with no keys in it at all.
#
# This is not interchangeable with OVMF_VARS.secboot.fd, and the difference
# cost a debugging round: Fedora's `secboot` variable store ships with Red Hat
# and Microsoft certificates already enrolled as PK/KEK/db. virt-fw-vars'
# --add-db APPENDS, and --no-microsoft only means "do not add more", so
# building on that template leaves a firmware that trusts Microsoft's UEFI CA.
# Every "only APEX-signed images load" assertion would then be false while
# still passing, because a Fedora-signed shim would satisfy db too.
ovmf_vars_template() {
    local v
    for v in /usr/share/edk2/ovmf/OVMF_VARS.fd \
             /usr/share/edk2/x64/OVMF_VARS.4m.fd; do
        [[ -f "$v" ]] || continue
        # Assert pristine. If Fedora ever pre-enrolls keys into this file too,
        # that must be a hard failure here rather than a silently weakened test.
        if virt-fw-vars --input "$v" --print 2>/dev/null | grep -qE '^name=(PK|db)\b'; then
            continue
        fi
        printf '%s\n' "$v"; return 0
    done
    die "no PRISTINE (key-free) OVMF variable-store template found in /usr/share/edk2/{ovmf,x64}"
}

sd_boot_efi() {
    local p=/usr/lib/systemd/boot/efi/systemd-bootx64.efi
    [[ -f "$p" ]] || die "systemd-bootx64.efi not found at $p (systemd-boot-unsigned missing)"
    printf '%s\n' "$p"
}

sd_stub_efi() {
    local p=/usr/lib/systemd/boot/efi/linuxx64.efi.stub
    [[ -f "$p" ]] || die "linuxx64.efi.stub not found at $p (systemd-boot-unsigned missing)"
    printf '%s\n' "$p"
}

# ── ESP image authoring (no loop devices, no privileges) ────────────────────
#
# The ESP is a real GPT partition inside a disk image rather than a whole-disk
# FAT "superfloppy", and that is load-bearing rather than tidiness:
# systemd-bless-boot resolves the ESP through find_esp_and_warn(), which
# insists on the EFI System Partition GPT type GUID. On a superfloppy it fails
# with "Failed to find ESP" and the whole boot-counting deliverable becomes
# untestable. `parted` writes the GPT into a plain file and the FAT filesystem
# is built separately and dd'd into place, so nothing here needs losetup or
# root inside the container.
ESP_OFFSET_BYTES=$((1024 * 1024))   # parted's default 1 MiB first-partition start

esp_disk_create() {
    local disk="$1" esp_mib="${2:-256}"
    local total_mib=$(( esp_mib + 2 ))
    rm -f "$disk" "$disk.esp"
    truncate -s "${total_mib}M" "$disk"
    parted -s "$disk" mklabel gpt \
        mkpart APEXESP fat32 1MiB "$(( esp_mib + 1 ))MiB" \
        set 1 esp on
    truncate -s "${esp_mib}M" "$disk.esp"
    mkfs.vfat -F 32 -n APEXESP "$disk.esp" >/dev/null
    # Verify the partition really is an ESP: `set 1 esp on` silently doing
    # nothing would produce a disk that boots but on which bless-boot cannot
    # find the ESP, which is exactly the failure this layout exists to avoid.
    parted -s "$disk" print 2>/dev/null | grep -q 'esp' \
        || die "GPT partition 1 in $disk is not flagged esp"
}

# Flush the staged FAT image into the partition. Idempotent, and every caller
# that mutates $disk.esp must call this before booting.
esp_disk_flush() {
    local disk="$1"
    [[ -f "$disk.esp" ]] || die "no staged ESP image at $disk.esp"
    dd if="$disk.esp" of="$disk" bs=1M seek=1 conv=notrunc status=none
}

# Pull the partition back out after a boot, so the bootloader's own writes
# (the boot-counting rename) are what we inspect — not our pre-boot staging
# copy. Reading $disk.esp after a run would report the tally we wrote, which is
# the single most likely way this whole deliverable goes green while proving
# nothing.
esp_disk_readback() {
    local disk="$1" out="$2"
    local esp_mib
    esp_mib=$(( ($(stat -c %s "$disk") / 1048576) - 2 ))
    dd if="$disk" of="$out" bs=1M skip=1 count="$esp_mib" conv=notrunc status=none
}

esp_mkdir_p() {
    local esp="$1" path="$2" acc="" part
    # mmd fails on an existing directory, so create each level tolerantly and
    # then assert the leaf exists. This is the narrow exception AGENTS.md
    # allows: a tolerated "already exists" followed by a hard postcondition.
    IFS=/ read -r -a parts <<<"${path#/}"
    for part in "${parts[@]}"; do
        acc="$acc/$part"
        mmd -i "$esp" "::$acc" >/dev/null 2>&1 || true
    done
    mdir -i "$esp" "::$path" >/dev/null 2>&1 \
        || die "could not create $path in $esp"
}

# ── guest launch ────────────────────────────────────────────────────────────
#
# Returns 0 only on a clean guest-initiated poweroff. Everything else — timeout
# kill, qemu error, firmware refusing to boot anything — is a non-zero return
# and the caller decides whether that was the expected outcome. The serial log
# is the single source of truth for what the guest said; assertions must read
# it and nothing else, because a marker found anywhere in the work directory
# could have come from the host that wrote the artifacts.
#
# `swtpm` is started per run with fresh state. A software TPM is what makes
# measured boot and TPM-bound LUKS2 testable at all; an untested TPM policy is
# worse than none, because it fails on the user's machine at the moment they
# cannot get a shell.
vm_boot() {
    local disk="" vars="" name="run" timeout=120 mem=2048 smp=2
    local tpm=0 serial="" extra_disk="" accel="kvm:tcg"
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --disk)       disk="$2"; shift 2;;
            --vars)       vars="$2"; shift 2;;
            --name)       name="$2"; shift 2;;
            --timeout)    timeout="$2"; shift 2;;
            --mem)        mem="$2"; shift 2;;
            --smp)        smp="$2"; shift 2;;
            --tpm)        tpm=1; shift;;
            --serial)     serial="$2"; shift 2;;
            --extra-disk) extra_disk="$2"; shift 2;;
            --accel)      accel="$2"; shift 2;;
            *) die "vm_boot: unknown argument $1";;
        esac
    done
    [[ -f "$disk" ]] || die "vm_boot: no disk image at $disk"
    [[ -f "$vars" ]] || die "vm_boot: no varstore at $vars"
    [[ -n "$serial" ]] || die "vm_boot: --serial is required"

    local code; code="$(ovmf_code_secboot)"
    local tpmdir="" tpm_args=()
    if (( tpm )); then
        tpmdir="$(dirname "$serial")/tpm-$name"
        rm -rf "$tpmdir"; mkdir -p "$tpmdir"
        swtpm socket --tpm2 --tpmstate "dir=$tpmdir" \
            --ctrl "type=unixio,path=$tpmdir/sock" \
            --log "file=$tpmdir/swtpm.log,level=1" \
            --pid "file=$tpmdir/swtpm.pid" --daemon \
            || die "swtpm failed to start (see $tpmdir/swtpm.log)"
        # Wait for the control socket rather than sleeping: a fixed sleep is
        # either flaky or slow, and on a 20-core box it is always the wrong
        # number.
        local i
        for i in $(seq 1 100); do
            [[ -S "$tpmdir/sock" ]] && break
            sleep 0.05
        done
        [[ -S "$tpmdir/sock" ]] || die "swtpm control socket never appeared"
        tpm_args=(
            -chardev "socket,id=chrtpm,path=$tpmdir/sock"
            -tpmdev emulator,id=tpm0,chardev=chrtpm
            -device tpm-tis,tpmdev=tpm0
        )
    fi

    local extra_args=()
    [[ -n "$extra_disk" ]] && extra_args=(-drive "if=virtio,format=raw,file=$extra_disk,media=disk")

    : > "$serial"
    local dbg="${serial%.log}.ovmf.log"
    : > "$dbg"

    info "booting '$name' (SB enforcing, tpm=$tpm, timeout=${timeout}s)"
    info "  firmware $code"
    info "  serial   $serial"

    local rc=0
    set +e
    timeout --foreground --signal=KILL "$timeout" \
    qemu-system-x86_64 \
        -machine "q35,smm=on,accel=$accel" \
        -cpu max -m "$mem" -smp "$smp" \
        -global driver=cfi.pflash01,property=secure,value=on \
        -global ICH9-LPC.disable_s3=1 \
        -drive "if=pflash,unit=0,format=raw,readonly=on,file=$code" \
        -drive "if=pflash,unit=1,format=raw,file=$vars" \
        -drive "if=virtio,format=raw,file=$disk,media=disk" \
        "${extra_args[@]}" \
        "${tpm_args[@]}" \
        -debugcon "file:$dbg" -global isa-debugcon.iobase=0x402 \
        -serial "file:$serial" \
        -display none -nodefaults -no-reboot \
        2>>"$dbg"
    rc=$?
    set -e

    if [[ -n "$tpmdir" && -f "$tpmdir/swtpm.pid" ]]; then
        kill "$(cat "$tpmdir/swtpm.pid")" 2>/dev/null || true
    fi
    info "qemu exited rc=$rc (0 = guest powered off; 137 = timeout kill)"
    return "$rc"
}

# ── assertions ──────────────────────────────────────────────────────────────
BOOTV2_PASS=0
BOOTV2_FAIL=0

ok()   { BOOTV2_PASS=$((BOOTV2_PASS + 1)); printf '  ok   %s\n' "$*" >&2; }
bad()  { BOOTV2_FAIL=$((BOOTV2_FAIL + 1)); printf '  FAIL %s\n' "$*" >&2; }

assert_eq() {
    local want="$1" got="$2" what="$3"
    if [[ "$want" == "$got" ]]; then ok "$what == $want"
    else bad "$what: want '$want', got '$got'"; fi
}

# Reads ONLY the named serial log. Never the work directory: the UKI, the ESP
# staging copy and the guest init script all contain the marker string, so a
# grep over the directory would pass without a guest ever running.
assert_serial_has() {
    local serial="$1" marker="$2"
    [[ -f "$serial" ]] || { bad "serial log $serial does not exist"; return; }
    if grep -qF -- "$marker" "$serial"; then ok "serial '$(basename "$serial")' contains $marker"
    else bad "serial '$(basename "$serial")' is missing $marker"; fi
}

assert_serial_lacks() {
    local serial="$1" marker="$2"
    [[ -f "$serial" ]] || { bad "serial log $serial does not exist"; return; }
    if grep -qF -- "$marker" "$serial"; then bad "serial '$(basename "$serial")' unexpectedly contains $marker"
    else ok "serial '$(basename "$serial")' does not contain $marker"; fi
}

bootv2_summary() {
    printf '\n== %s: %d passed, %d failed ==\n' "${1:-boot-v2}" "$BOOTV2_PASS" "$BOOTV2_FAIL" >&2
    (( BOOTV2_FAIL == 0 )) || return 1
}
