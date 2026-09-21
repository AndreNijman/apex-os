#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  check-initramfs-budget.sh — the initramfs gates, as a predicate you can run
#  without a 40-minute image build.
#
#  WHY THIS FILE EXISTS
#
#  These gates used to live inline in Containerfile.apex, which makes them the
#  one kind of test that cannot be run by running the test suite: they only
#  exist inside a build. That is the defect family this repo has already paid
#  five days of image builds for, and it bit again here — the stanza shipped
#
#      grep -qE 'kernel/drivers/net/ethernet/'  -> FATAL
#
#  as "the initramfs has no network", and the slim initramfs the branch
#  produces CONTAINS eleven files under kernel/drivers/net. It would have
#  FATALed every build, on the very artifact it was written to pass, and
#  nobody would have known until the build ran.
#
#  Measured on 2026-09-21 against the real artifacts, those eleven arrive by
#  THREE unrelated routes and none of them is the network stack:
#
#      cnic      <- scsi/bnx2fc        FCoE offload
#      qed       <- scsi/qedf          FCoE offload
#      cxgb4     <- crypto/chelsio/chcr   a CRYPTO accelerator, not storage
#      libertas*, mt76*, mt7921*       SDIO wireless, dragged along the
#                                      MMC/SDIO block path
#
#  They are dependencies of modules that are in the initramfs for storage and
#  crypto reasons, so no omit_dracutmodules setting removes them. "No file path
#  contains the word ethernet" was never the property we meant.
#
#  WHAT WE ACTUALLY MEAN, and what this file asserts instead: the network
#  dracut MODULES are absent, NetworkManager's binary is absent, and the
#  kernel-network-modules tree — all of =drivers/net, 322 files and 21.9 MiB
#  measured on the baseline — has not come back. Eleven files and 1.5 MiB of
#  dependency residue passes; the 21.9 MiB tree does not. Both arms are real:
#  the fixture suite runs this file red on a fat initramfs and green on a slim
#  one, which is the thing an inline Containerfile assertion can never show.
#
#  TWO MODES, and the second is the point
#
#    --initrd IMG          runs `lsinitrd` itself. What Containerfile.apex uses.
#    --list FILE           reads a SAVED `lsinitrd` listing. What tests/ uses,
#                          because a GitHub runner has no lsinitrd, no initramfs
#                          and no kernel to make one from.
#
#  A single listing carries both halves — `lsinitrd` prints the dracut module
#  list between `dracut modules:` and the next rule, then the files — so one
#  decompression answers every gate. The old stanza called `lsinitrd` twice and
#  then greped the pipe; greping a FILE also means `grep -q` cannot take
#  SIGPIPE and return 141, turning a match into a silent pass.
#
#  Every gate is reported, pass or fail, and the run does NOT stop at the first
#  failure: a build log that names one problem per 40-minute build is how this
#  branch lost two rounds.
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail

INITRD=""
LIST=""
KCONF=""
VMLINUZ=""
INITRD_BYTES=""
VMLINUZ_BYTES=""
BUDGET_MIB=130

# The kernel-network-modules tree is 322 files / 21.9 MiB when it is present.
# The dependency residue is 11 files / 1.5 MiB. These ceilings sit between the
# two with room either side, so neither a new SDIO wireless dependency nor a
# rounding difference flips the gate, and the tree coming back cannot hide.
NET_MAX_FILES=40
NET_MAX_BYTES=$(( 6 * 1048576 ))

usage() {
    cat <<'EOF'
usage:
  check-initramfs-budget.sh --initrd IMG [--vmlinuz PATH] [--kconfig PATH]
  check-initramfs-budget.sh --list FILE --initrd-bytes N [--vmlinuz-bytes N]
                            [--kconfig FILE]
  [--budget-mib N]   per-deployment ceiling in MiB (default 130)

--initrd runs lsinitrd; --list reads a saved lsinitrd listing so the same
predicate runs on a machine with neither an initramfs nor lsinitrd.
EOF
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --initrd)        INITRD="$2"; shift 2 ;;
        --list)          LIST="$2"; shift 2 ;;
        --kconfig)       KCONF="$2"; shift 2 ;;
        --vmlinuz)       VMLINUZ="$2"; shift 2 ;;
        --initrd-bytes)  INITRD_BYTES="$2"; shift 2 ;;
        --vmlinuz-bytes) VMLINUZ_BYTES="$2"; shift 2 ;;
        --budget-mib)    BUDGET_MIB="$2"; shift 2 ;;
        -h|--help)       usage; exit 0 ;;
        *) echo "FATAL: unknown argument $1" >&2; usage >&2; exit 2 ;;
    esac
done

fail_count=0
pass_count=0

pass() { pass_count=$(( pass_count + 1 )); echo "  ok    $1"; }
fail() {
    fail_count=$(( fail_count + 1 ))
    echo "  FAIL  $1"
    shift
    for l in "$@"; do echo "          $l"; done
}

mib() { awk -v b="$1" 'BEGIN { printf "%.1f", b / 1048576 }'; }

# ── resolve the listing ──────────────────────────────────────────────────────
WORK="$(mktemp -d)"
cleanup() { [ -n "$WORK" ] && rm -rf "$WORK"; }
trap cleanup EXIT

if [ -n "$INITRD" ]; then
    [ -s "$INITRD" ] || { echo "FATAL: no initramfs at $INITRD"; exit 1; }
    command -v lsinitrd >/dev/null \
        || { echo "FATAL: lsinitrd is not installed; use --list with a saved listing"; exit 1; }
    LIST="$WORK/initrd.list"
    lsinitrd "$INITRD" > "$LIST" \
        || { echo "FATAL: lsinitrd failed on $INITRD"; exit 1; }
    [ -z "$INITRD_BYTES" ] && INITRD_BYTES="$(stat -c %s "$INITRD")"
elif [ -n "$LIST" ]; then
    [ -s "$LIST" ] || { echo "FATAL: no listing at $LIST"; exit 1; }
else
    echo "FATAL: one of --initrd or --list is required" >&2; usage >&2; exit 2
fi

if [ -n "$VMLINUZ" ] && [ -z "$VMLINUZ_BYTES" ]; then
    [ -s "$VMLINUZ" ] || { echo "FATAL: no kernel at $VMLINUZ"; exit 1; }
    VMLINUZ_BYTES="$(stat -c %s "$VMLINUZ")"
fi

# Split the listing once, INTO FILES ON DISK. Two reasons, and the second one
# cost this script a full debugging round on 2026-09-21:
#
# 1. The module names sit between `dracut modules:` and the rule that follows
#    it; the file entries are the `ls -l` lines. Splitting matters because the
#    word `network` appears in the module section AND in file paths, so a grep
#    over the whole listing cannot tell "the network module is installed" from
#    "a file has network in its name".
#
# 2. THE PREDICATES MUST GREP A FILE, NEVER A PIPE. The first draft held both
#    halves in shell variables and asked `printf '%s\n' "$FILES" | grep -qE …`.
#    `grep -q` exits the instant it matches, closing the pipe under `printf`,
#    which takes SIGPIPE and returns 141 — and `set -o pipefail` makes 141 the
#    status of the whole pipeline. So A MATCH READ AS A MISS. It is
#    position-dependent, which is what makes it vicious: measured here, a match
#    at listing line 280 returned 141 while a match near the end returned 0, so
#    seven present files were reported absent and two equally present ones
#    passed. A grep on a real file cannot take SIGPIPE and cannot do this.
MODF="$WORK/mods"
FILF="$WORK/files"
sed -n '/^dracut modules:$/,/^=\{10,\}$/p' "$LIST" \
    | sed '1d;$d' | tr -d ' \t' | grep -v '^$' > "$MODF"
grep -E '^[-dlbcps][rwxSsTt-]{9}' "$LIST" > "$FILF"

[ -s "$MODF" ] || { echo "FATAL: no 'dracut modules:' section in $LIST — not an lsinitrd listing?"; exit 1; }
[ -s "$FILF" ] || { echo "FATAL: no file entries in $LIST — not an lsinitrd listing?"; exit 1; }

has_mod()  { grep -qx "$1" "$MODF"; }
has_path() { grep -qE "$1" "$FILF"; }
show_path() { grep -E "$1" "$FILF" | awk '{print $NF}' | head -"${2:-6}"; }

echo "initramfs gates: $(wc -l < "$MODF") dracut modules, $(wc -l < "$FILF") entries"

# ══ 1. THE ESP BUDGET ════════════════════════════════════════════════════════
# systemd-boot puts the kernel and initramfs ON the ESP and bootc implements no
# XBOOTLDR Type #1 entries, so there is no second partition to move them to.
# apex-boot-migrate needs 3 × (vmlinuz + initramfs) + 48 MiB — booted, rollback,
# and the one an update stages alongside them.
#
# The figure is printed whether it passes or fails, because migrate-preconditions
# consumes it and a slow drift should be visible in every build log before it is
# ever fatal.
if [ -n "$INITRD_BYTES" ]; then
    PER=$(( INITRD_BYTES + ${VMLINUZ_BYTES:-0} ))
    NEED=$(( PER * 3 / 1048576 + 48 ))
    echo "initramfs-budget: vmlinuz $(mib "${VMLINUZ_BYTES:-0}") MiB + initramfs $(mib "$INITRD_BYTES") MiB = $(mib "$PER") MiB per deployment (ceiling ${BUDGET_MIB} MiB)"
    echo "initramfs-budget: apex-boot-migrate needs 3 x $(mib "$PER") + 48 = ${NEED} MiB of ESP"
    if [ "$PER" -gt $(( BUDGET_MIB * 1048576 )) ]; then
        fail "esp-budget: $(mib "$PER") MiB per deployment exceeds the ${BUDGET_MIB} MiB ceiling" \
             "apex-boot-migrate would refuse a 512 MiB ESP with esp-too-small." \
             "The usual cause is a driver re-entering the initramfs and dragging" \
             "its firmware with it. Biggest entries:"
        sort -k5 -n -r "$FILF" | head -15 | awk '{printf "          %10d  %s\n", $5, $NF}'
    else
        pass "esp-budget: $(mib "$PER") MiB per deployment, needs ${NEED} MiB of ESP"
    fi
else
    echo "  ----  esp-budget: not checked (no --initrd-bytes)"
fi

# ══ 2. NO GPU FIRMWARE ═══════════════════════════════════════════════════════
# 203 MiB of the 358 MiB baseline was /usr/lib/firmware/nvidia alone. dracut
# installs the firmware of every driver it installs, so this fails the moment a
# KMS driver is re-added — before the size gate would.
GPUFW='usr/lib/firmware/(nvidia|amdgpu|i915|xe|radeon)/'
if has_path "$GPUFW"; then
    fail "no-gpu-firmware: GPU firmware is in the initramfs" \
         "$(show_path "$GPUFW" 5 | tr '\n' ' ')" \
         "Check add_drivers in 99-nvidia-dracut.conf."
else
    pass "no-gpu-firmware"
fi

# ══ 3. NO KMS DRIVER ═════════════════════════════════════════════════════════
# Worse than the size: a KMS driver calls
# drm_aperture_remove_conflicting_pci_framebuffers() early in probe, which
# evicts simpledrm BEFORE it loads its firmware. A driver here without its
# firmware is a BLACK SCREEN at the LUKS passphrase prompt, not a plain one.
KMS='/(nouveau|amdgpu|i915|xe|radeon|nvidia|nvidia_modeset|nvidia_drm|nvidia_uvm)\.ko'
if has_path "$KMS"; then
    fail "no-kms-driver: a KMS driver is in the initramfs" \
         "$(show_path "$KMS" 8 | tr '\n' ' ')" \
         "It evicts simpledrm during probe and has no firmware to replace it with."
else
    pass "no-kms-driver"
fi

# ══ 4. NO NETWORK ════════════════════════════════════════════════════════════
# Three assertions, because the property has three halves and the old one-line
# path grep was none of them. APEX roots are local block devices; nothing in
# this repo sets rd.neednet, netroot, nfsroot or iscsi.
net_bad=()
for m in network network-manager kernel-network-modules nfs nvmf; do
    has_mod "$m" && net_bad+=("$m")
done
if [ "${#net_bad[@]}" -gt 0 ]; then
    fail "no-network-modules: dracut installed ${net_bad[*]}" \
         "omit_dracutmodules in 99-nvidia-dracut.conf has regressed."
else
    pass "no-network-modules"
fi

if has_path 'usr/s?bin/NetworkManager$'; then
    fail "no-networkmanager: the NetworkManager binary is in the initramfs"
else
    pass "no-networkmanager"
fi

# The bounded one. kernel-network-modules installs ALL of =drivers/net: 322
# files / 21.9 MiB measured on the baseline. What survives omitting it is 11
# files / 1.5 MiB of dependency residue (FCoE offload, a Chelsio crypto
# accelerator, SDIO wireless) which no dracut setting can remove. So the gate
# is a CEILING, not an absence — the absence version is what FATALed on the
# artifact it was written to pass.
net_files=$(awk '$0 ~ /kernel\/drivers\/net\// && $1 ~ /^-/' "$FILF" | wc -l)
net_bytes=$(awk '$0 ~ /kernel\/drivers\/net\// && $1 ~ /^-/ {s+=$5} END {print s+0}' "$FILF")
if [ "$net_files" -gt "$NET_MAX_FILES" ] || [ "$net_bytes" -gt "$NET_MAX_BYTES" ]; then
    fail "net-driver-tree: ${net_files} files / $(mib "$net_bytes") MiB under drivers/net" \
         "ceiling is ${NET_MAX_FILES} files / $(mib "$NET_MAX_BYTES") MiB; the baseline tree was 322 / 21.9 MiB." \
         "kernel-network-modules is back, or something now depends on it."
else
    pass "net-driver-tree: ${net_files} files / $(mib "$net_bytes") MiB (ceiling ${NET_MAX_FILES} / $(mib "$NET_MAX_BYTES") MiB)"
fi

# ══ 5. THE POSITIVE HALF — what draws the passphrase prompt ══════════════════
# Removing every KMS driver is only safe because the firmware framebuffer
# console is built INTO the kernel. Containerfile.kernel asserts this against
# the config it built; this asserts it against the config that SHIPPED.
if [ -n "$KCONF" ]; then
    if [ ! -s "$KCONF" ]; then
        fail "simpledrm-built-in: no kernel config at $KCONF"
    else
        missing=()
        for opt in CONFIG_DRM CONFIG_DRM_SIMPLEDRM CONFIG_SYSFB_SIMPLEFB \
                   CONFIG_DRM_FBDEV_EMULATION CONFIG_FRAMEBUFFER_CONSOLE; do
            grep -q "^${opt}=y" "$KCONF" || missing+=("$opt")
        done
        if [ "${#missing[@]}" -gt 0 ]; then
            fail "simpledrm-built-in: ${missing[*]} not =y in the shipped kernel" \
                 "Nothing would draw the LUKS prompt: this initramfs carries no KMS" \
                 "driver and relies on simpledrm binding the framebuffer UEFI set up."
        else
            pass "simpledrm-built-in: all five =y"
        fi
    fi
else
    echo "  ----  simpledrm-built-in: not checked (no --kconfig)"
fi

# ══ 6. PLYMOUTH SURVIVED THE OMISSIONS ═══════════════════════════════════════
# Its dracut module depends on `drm`, which is still installed — only the
# DRIVERS were omitted — and a silently-dropped plymouth looks identical to a
# working build until someone boots one.
has_path 'usr/lib(64)?/plymouth' \
    && pass "plymouth-engine" \
    || fail "plymouth-engine: the plymouth engine is not in the initramfs"
has_path 'plymouth/themes/apex-os-chartreuse' \
    && pass "plymouth-theme" \
    || fail "plymouth-theme: the APEX plymouth theme is not in the initramfs"

# ══ 7. THE DISK-UNLOCK CHAIN ═════════════════════════════════════════════════
# Nothing here is about size; these are the things whose ABSENCE this branch
# could plausibly cause, because omitting dracut modules is how you lose them.
# The keymap check names a NON-us map on purpose: `us` is the kernel built-in
# and would be there even if the whole keymap tree were missing.
check_path() {
    has_path "$2" && pass "$1" || fail "$1: $3"
}
check_path unlock-cryptsetup 'usr/bin/systemd-cryptsetup' \
    "an encrypted root could never be opened"
check_path unlock-tpm2-token 'libcryptsetup-token-systemd-tpm2\.so' \
    "a TPM keyslot would be unusable"
check_path unlock-keymaps 'kbd/keymaps/.*/de\.map' \
    "every unlock prompt would be a US one"
check_path unlock-hint 'usr/bin/apex-unlock-hint' \
    "the prompt could not say which keyboard it is using"
check_path unlock-hint-wants 'initrd\.target\.wants/apex-unlock-hint\.service' \
    "the hint is present but nothing pulls it in — it would never run"
check_path unlock-vconsole 'usr/bin/apex-vconsole-credential' \
    "a UKI machine could not be told its keyboard layout"
check_path unlock-vconsole-wants 'sysinit\.target\.wants/apex-vconsole-credential\.service' \
    "it would run after the keymap was already loaded, or never"

has_mod crypt \
    && pass "unlock-crypt-module" \
    || fail "unlock-crypt-module: dracut's crypt module is absent — rd.luks.* would be ignored"

# ── verdict ──────────────────────────────────────────────────────────────────
echo "initramfs gates: ${pass_count} passed, ${fail_count} failed"
if [ "$fail_count" -gt 0 ]; then
    echo "FATAL: ${fail_count} initramfs gate(s) failed"
    exit 1
fi
exit 0
