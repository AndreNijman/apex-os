#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  test-apex-storage.sh — executable assertions for roadmap §48's Storage
#  Manager: the read-and-report half.
#
#  ── The three failures this exists to catch ─────────────────────────────────
#
#  1. A DYING DISK REPORTED AS A DISK NOBODY COULD MEASURE. smartctl's exit
#     status is a bitmask. Bits 0-2 mean the read did not happen; bits 3-7 mean
#     it did and the disk is failing. `if status != 0 { unavailable }` turns the
#     one row this whole feature exists for from red into grey.
#
#  2. A PERMANENT FALSE ALARM ABOUT A MACHINE WITH A TERABYTE FREE. On a bootc
#     machine / is a composefs image: 37.8M, 0 available, 100% used, on every
#     APEX machine, forever. Every disk-usage widget ever written points at /.
#
#  3. AN ENCRYPTED DISK CALLED UNENCRYPTED. `blkid` unprivileged exits 0 and
#     prints nothing, so a caller that trusts it reports "not encrypted" for a
#     device it never opened — the one sentence here a user would act on by
#     putting data at risk.
#
#  Two modes, the split test-apex-schema.sh uses:
#
#    (no argument)     Structural checks with no toolchain.
#    --with-binary     Drives `apex storage` and the notifier against a whole
#                      fixture machine: sysfs, mountinfo, udev records and
#                      captured smartctl output. It DIES if the binary is
#                      absent; a skipped assertion reports as a pass.
#
#  NOTHING HERE TOUCHES A REAL DISK. Under $APEX_STORAGE_ROOT no subprocess is
#  spawned at all — smartctl's answer, the fstrim timer's state and every
#  filesystem's size come from files this script writes.
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WITH_BINARY=0
[[ "${1:-}" == "--with-binary" ]] && WITH_BINARY=1

PASS=0 FAIL=0
ok()  { PASS=$((PASS + 1)); printf '  ok   %s\n' "$*"; }
bad() { FAIL=$((FAIL + 1)); printf '  FAIL %s\n' "$*"; }
sec() { printf '\n== %s ==\n' "$*"; }
has() {
    if grep -qF -- "$1" "$2"; then ok "$3"; else
        bad "$3 — no '$1' in:"; sed 's/^/       /' "$2" >&2
    fi
}
hasnt() {
    if grep -qF -- "$1" "$2"; then
        bad "$3 — found '$1' in:"; sed 's/^/       /' "$2" >&2
    else ok "$3"; fi
}

CORE="$REPO/apexd/apexd-core/src/storage.rs"
CLI="$REPO/apexd/apex/src/storage.rs"
NOTICE="$REPO/files/system/libexec/apex-storage-notice"
TIMER="$REPO/files/system/units/apex-storage-notice.timer"
SVC="$REPO/files/system/units/apex-storage-notice.service"
for f in "$CORE" "$CLI" "$NOTICE" "$TIMER" "$SVC"; do
    [[ -f "$f" ]] || { echo "FATAL: missing $f" >&2; exit 1; }
done

sec "the smartctl bitmask is split, not compared against zero"
has 'const SMART_BITS' "$CORE" "every bit smartctl(8) defines is in one table"
has 'SMART says this disk is FAILING' "$CORE" "including bit 3"
# The mutation: somebody replacing the split with a comparison.
if grep -qE 'exit_code\s*!=\s*0|code\s*!=\s*0\s*\{' "$CORE"; then
    bad "a bare comparison against zero has appeared in the smart path"
else
    ok "no bare comparison against zero decides whether a read happened"
fi
has 'a_failing_disk_is_a_read_that_happened_and_not_an_unavailable_row' "$CORE" \
    "and a test names the case"

sec "free space is a property of a filesystem, not of a mount point"
has 'starts_with("/dev/")' "$CORE" "only block-device sources are considered"
has 'composefs' "$CORE" "the composefs root is named as the reason"
has 'the_composefs_root_of_a_bootc_machine_is_not_a_filesystem_to_warn_about' "$CORE" \
    "and a test holds it"
has 'SPACE_ATTENTION_BYTES' "$CORE" "a byte floor as well as a percentage"

sec "encryption is never inferred from a read that did not happen"
has 'Unknown(String)' "$CORE" "the unknown arm exists"
has 'run/udev/data' "$CLI" "the filesystem type comes from udev's own database"
hasnt 'Command::new("blkid")' "$CLI" "blkid is never spawned"
hasnt '"/usr/sbin/blkid"' "$CLI" "nor by absolute path"

sec "no report can print a disk serial"
# The udev record this module reads carries ID_SERIAL_SHORT, so the serial is
# right there and is never looked up. The binary half proves the output is
# clean; this is the structural half, and it looks for a LOOKUP rather than for
# the string, because the fixtures in the test module contain the string on
# purpose.
if sed -E 's://.*$::' "$CLI" | grep -qE 'get\("ID_SERIAL|ID_SERIAL[A-Z_]*"\]'; then
    bad "the CLI looks up ID_SERIAL"
else
    ok "the CLI never looks up ID_SERIAL"
fi
if sed -E 's://.*$::' "$CLI" | grep -qE 'Command::new.*nvme|"/usr/sbin/nvme"'; then
    bad "nvme list, which prints the serial to any user, is spawned"
else
    ok "nvme-cli is never spawned"
fi

sec "the notifier will not wake somebody over a read it could not perform"
has 'rc=$?' "$NOTICE" "the exit status is captured before anything else runs"
has 'grep ' "$NOTICE" "only attention rows reach the digest"
has 'rm -f "$SEEN"' "$NOTICE" "and a cleared warning forgets its digest"
has 'systemd/user' "$REPO/Containerfile.base" "the unit ships as a user unit"
hasnt '[Install]' "$SVC" "the service itself is not enabled; the timer is"
has 'Persistent=true' "$TIMER" "a machine that was off checks when it returns"

if [[ "$WITH_BINARY" -eq 0 ]]; then
    printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
    [[ "$FAIL" -eq 0 ]] || exit 1
    exit 0
fi

# ── binary-driven half ──────────────────────────────────────────────────────

APEX="${APEX:-$REPO/apexd/target/debug/apex}"
[[ -x "$APEX" ]] || { echo "FATAL: no apex binary at $APEX" >&2; exit 1; }

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
export XDG_STATE_HOME="$TMP/state"
export XDG_CONFIG_HOME="$TMP/config"
mkdir -p "$XDG_STATE_HOME" "$XDG_CONFIG_HOME"

FIX="$TMP/machine"
export APEX_STORAGE_ROOT="$FIX"

# A whole machine: one NVMe with five partitions, one loop device that must be
# skipped, a composefs root and a btrfs volume under five mount points.
build_machine() {
    rm -rf "$FIX"
    mkdir -p "$FIX/sys/block" "$FIX/sys/devices/pci0000:00/nvme/nvme0/nvme0n1" \
             "$FIX/sys/devices/virtual/block/loop0" \
             "$FIX/proc/self" "$FIX/run/udev/data" "$FIX/.fixture/smartctl" \
             "$FIX/.fixture/space"
    # /sys/block entries are symlinks into /sys/devices; the loop device lives
    # under devices/virtual/block and is skipped by where it is, not by name.
    ln -sfn ../devices/pci0000:00/nvme/nvme0/nvme0n1 "$FIX/sys/block/nvme0n1"
    ln -sfn ../devices/virtual/block/loop0 "$FIX/sys/block/loop0"

    local d="$FIX/sys/devices/pci0000:00/nvme/nvme0/nvme0n1"
    mkdir -p "$d/queue" "$d/device/hwmon4"
    printf '4000797360\n' > "$d/size"
    printf '0\n'          > "$d/queue/rotational"
    printf '512\n'        > "$d/queue/discard_granularity"
    printf '0\n'          > "$d/removable"
    printf '0\n'          > "$d/ro"
    printf 'SPCC M.2 PCIe SSD\n' > "$d/device/model"
    printf '37850\n'      > "$d/device/hwmon4/temp1_input"

    local i=1
    for spec in "p1:1228800:vfat" "p2:4194304:ext4" "p3:831107072:btrfs" \
                "p4:4194304:ext4" "p5:3160272896:btrfs"; do
        local part="${spec%%:*}" rest="${spec#*:}"
        local sectors="${rest%%:*}" fs="${rest##*:}"
        mkdir -p "$d/nvme0n1$part"
        printf '%s\n' "$sectors" > "$d/nvme0n1$part/size"
        printf '1\n'             > "$d/nvme0n1$part/partition"
        printf '259:%s\n' "$i"   > "$d/nvme0n1$part/dev"
        {
            printf 'S:disk/by-label/apex-%s\n' "$part"
            printf 'I:11132713\n'
            printf 'E:ID_SERIAL_SHORT=NF12312150500322\n'
            printf 'E:ID_MODEL=SPCC M.2 PCIe SSD\n'
            printf 'E:ID_FS_TYPE=%s\n' "$fs"
        } > "$FIX/run/udev/data/b259:$i"
        i=$((i + 1))
    done

    # Measured on the L16: / is composefs, and one btrfs volume is mounted at
    # five paths of which only /sysroot exposes the whole filesystem.
    cat > "$FIX/proc/self/mountinfo" <<'EOF'
49 1 0:39 / / ro,relatime shared:1 - overlay composefs ro,seclabel,lowerdir+=/x
46 49 0:36 /ostree/deploy/default/deploy/f3f5.0/etc /etc rw,relatime shared:2 - btrfs /dev/nvme0n1p5 rw,seclabel,ssd,discard=async,space_cache=v2
50 49 0:36 / /sysroot ro,relatime shared:3 - btrfs /dev/nvme0n1p5 rw,seclabel,ssd,discard=async,space_cache=v2
51 50 0:36 /ostree/deploy/default/var /var rw,relatime shared:4 - btrfs /dev/nvme0n1p5 rw,seclabel,ssd,discard=async,space_cache=v2
52 49 0:36 /boot /boot rw,relatime shared:5 - btrfs /dev/nvme0n1p5 rw,seclabel,ssd,discard=async,space_cache=v2
39 49 0:7 / /dev rw,nosuid shared:6 - devtmpfs devtmpfs rw,seclabel
41 39 0:27 / /dev/shm rw,nosuid,nodev shared:7 - tmpfs tmpfs rw,seclabel
53 49 0:40 / /boot/efi rw,relatime shared:8 - vfat /dev/nvme0n1p1 rw,fmask=0077
EOF
    # 1.5 TB, 27% used — the real figures from the L16.
    printf '1618000000000 1180000000000\n' > "$FIX/.fixture/space/sysroot"
    printf '629145600 500000000\n'         > "$FIX/.fixture/space/boot_efi"
    printf 'enabled\n' > "$FIX/.fixture/fstrim-enabled"
    printf 'Mon 2026-09-07 01:06:25 AWST\n' > "$FIX/.fixture/fstrim-last"
}

# The three smartctl answers this feature turns on.
smart_healthy() {
    printf '0\n' > "$FIX/.fixture/smartctl/nvme0n1.exit"
    cat > "$FIX/.fixture/smartctl/nvme0n1.json" <<'EOF'
{"smartctl": {"exit_status": 0},
 "model_name": "SPCC M.2 PCIe SSD", "firmware_version": "SN13683",
 "smart_status": {"passed": true},
 "temperature": {"current": 38},
 "nvme_smart_health_information_log": {
   "critical_warning": 0, "available_spare": 100, "available_spare_threshold": 10,
   "percentage_used": 1, "power_on_hours": 9222}}
EOF
}
smart_failing() {
    # Bit 3. The read HAPPENED and the disk is failing.
    printf '8\n' > "$FIX/.fixture/smartctl/nvme0n1.exit"
    cat > "$FIX/.fixture/smartctl/nvme0n1.json" <<'EOF'
{"smartctl": {"exit_status": 8},
 "model_name": "SPCC M.2 PCIe SSD",
 "smart_status": {"passed": false},
 "temperature": {"current": 62},
 "nvme_smart_health_information_log": {
   "critical_warning": 4, "available_spare": 3, "available_spare_threshold": 10,
   "percentage_used": 99, "power_on_hours": 51000}}
EOF
}
smart_refused() {
    # Exactly what an unprivileged run produces: exit 2, a full document, no
    # health log, and the reason inside `smartctl.messages`.
    printf '2\n' > "$FIX/.fixture/smartctl/nvme0n1.exit"
    cat > "$FIX/.fixture/smartctl/nvme0n1.json" <<'EOF'
{"json_format_version": [1, 0],
 "smartctl": {"version": [7, 5],
   "messages": [{"string": "Smartctl open device: /dev/nvme0n1 failed: Permission denied",
                 "severity": "error"}],
   "exit_status": 2},
 "local_time": {"time_t": 1788729982}}
EOF
}

build_machine
smart_healthy

sec "a healthy machine reads as a healthy machine"
"$APEX" storage status > "$TMP/out" 2>&1
has 'SPCC M.2 PCIe SSD' "$TMP/out" "the disk is named by model"
has 'solid state' "$TMP/out" "and by kind"
has '1% of the rated endurance is used' "$TMP/out" "wear comes from the SMART log"
has '9222 hours powered on' "$TMP/out" "with the hours beside it"
has '38 °C' "$TMP/out" "and the temperature"
has 'the filesystem discards as it frees blocks' "$TMP/out" \
    "btrfs's discard=async counts as trimming"
has '0 needing attention' "$TMP/out" "nothing to do"
hasnt 'loop0' "$TMP/out" "the loop device is not a disk"
hasnt 'NF12312150500322' "$TMP/out" "the disk serial, sitting in the udev record, is not printed"

sec "free space is one row per filesystem, and never the composefs root"
if grep -qE '^\[[a-z]+\] +[a-z0-9]+ on /( |$)' "$TMP/out"; then
    bad "the composefs root got a free-space row: $(grep -E ' on /( |$)' "$TMP/out")"
else
    ok "the composefs root has no row"
fi
hasnt 'composefs' "$TMP/out" "and composefs is not named as a filesystem to watch"
has 'btrfs on /sysroot' "$TMP/out" "the btrfs volume is named by the mount that IS the filesystem"
n="$(grep -c 'btrfs on ' "$TMP/out")"
[[ "$n" == "1" ]] && ok "five mount points produce one row" \
                  || bad "five mount points produced $n rows"
has 'vfat on /boot/efi' "$TMP/out" "a second filesystem still gets its own row"

sec "a disk that says it is failing is red, not grey"
smart_failing
"$APEX" storage status > "$TMP/out" 2>&1
has '[attention]' "$TMP/out" "the wear row needs attention"
has 'failing' "$TMP/out" "and says the disk is failing"
hasnt '[unavailable] /dev/nvme0n1 health' "$TMP/out" \
    "a non-zero exit did not turn the row into an unmeasured one"
has '1 needing attention' "$TMP/out" "counted once"
has '62 °C' "$TMP/out" "and the temperature still came through"

"$APEX" storage warnings > "$TMP/warn" 2>&1; rc=$?
[[ "$rc" -eq 1 ]] && ok "apex storage warnings exits 1 for a failing disk" \
                  || bad "warnings exited $rc for a failing disk"

sec "a refused read is grey, not red, and not silent"
smart_refused
"$APEX" storage status > "$TMP/out" 2>&1
has '[unavailable] /dev/nvme0n1 health' "$TMP/out" "the wear row is unavailable"
has 'Permission denied' "$TMP/out" "carrying what smartctl actually said"
has 'sudo apex storage status' "$TMP/out" "and the remedy"
hasnt '[attention] /dev/nvme0n1 health' "$TMP/out" "a refused read is not a failure"
has '38 °C' "$TMP/out" "and hwmon still gives a temperature with no privilege at all"

"$APEX" storage warnings > "$TMP/warn" 2>&1; rc=$?
[[ "$rc" -eq 0 ]] && ok "warnings exits 0 for a row nobody could measure" \
                  || bad "warnings exited $rc for an unmeasured row"
has 'unavailable' "$TMP/warn" "and still prints it"

sec "a filesystem with nothing left is an alert; a big one at the same percentage is not"
smart_healthy
printf '2000000000 100000000\n' > "$FIX/.fixture/space/boot_efi"
"$APEX" storage status > "$TMP/out" 2>&1
has '95% used' "$TMP/out" "the small filesystem is 95% used"
has '[attention] vfat on /boot/efi' "$TMP/out" "and that is an alert"
printf '4000000000000 400000000000\n' > "$FIX/.fixture/space/boot_efi"
"$APEX" storage status > "$TMP/out" 2>&1
has '90% used' "$TMP/out" "a 4 TB filesystem is 90% used"
hasnt '[attention] vfat' "$TMP/out" "and 400 GB free is not an alert"
printf '629145600 500000000\n' > "$FIX/.fixture/space/boot_efi"

sec "a partition nobody could look at is never called unencrypted"
mv "$FIX/run/udev/data/b259:3" "$TMP/udev-p3.saved"
"$APEX" storage status > "$TMP/out" 2>&1
has '[unavailable] /dev/nvme0n1p3' "$TMP/out" "the row is unavailable"
p3="$(grep '/dev/nvme0n1p3' "$TMP/out")"
case "$p3" in
    *"not encrypted"*) bad "a partition with no udev record was called unencrypted: $p3" ;;
    *) ok "and does not say it is unencrypted" ;;
esac
mv "$TMP/udev-p3.saved" "$FIX/run/udev/data/b259:3"

sec "a LUKS partition is named as encrypted"
sed -i 's/ID_FS_TYPE=btrfs/ID_FS_TYPE=crypto_LUKS/' "$FIX/run/udev/data/b259:3"
printf 'E:ID_FS_VERSION=2\n' >> "$FIX/run/udev/data/b259:3"
"$APEX" storage status > "$TMP/out" 2>&1
has 'encrypted (LUKS2)' "$TMP/out" "with its version"
sed -i 's/ID_FS_TYPE=crypto_LUKS/ID_FS_TYPE=btrfs/' "$FIX/run/udev/data/b259:3"

sec "a device the kernel forced read-only is an alert"
printf '1\n' > "$FIX/sys/devices/pci0000:00/nvme/nvme0/nvme0n1/ro"
"$APEX" storage status > "$TMP/out" 2>&1
has 'is read-only' "$TMP/out" "the row exists"
has 'nothing can be written to it' "$TMP/out" "and says what that means"
printf '0\n' > "$FIX/sys/devices/pci0000:00/nvme/nvme0/nvme0n1/ro"

sec "a device that supports discard with nothing trimming it is the only trim alert"
printf 'disabled\n' > "$FIX/.fixture/fstrim-enabled"
sed -i 's/,discard=async//g' "$FIX/proc/self/mountinfo"
"$APEX" storage status > "$TMP/out" 2>&1
has '[attention] /dev/nvme0n1 trim' "$TMP/out" "nothing is trimming a device that supports it"
printf '0\n' > "$FIX/sys/devices/pci0000:00/nvme/nvme0/nvme0n1/queue/discard_granularity"
"$APEX" storage status > "$TMP/out" 2>&1
has 'nothing to trim' "$TMP/out" "a device with no discard has nothing to trim"
hasnt '[attention] /dev/nvme0n1 trim' "$TMP/out" "and is not an alert"
build_machine; smart_healthy

sec "the notifier"
NOTICE_BIN="$REPO/files/system/libexec/apex-storage-notice"
# A notify-send that records rather than notifies. Nothing reaches a desktop.
mkdir -p "$TMP/bin"
cat > "$TMP/bin/notify-send" <<EOF
#!/usr/bin/env bash
printf '%s\n' "\$*" >> "$TMP/notified"
EOF
chmod +x "$TMP/bin/notify-send"
export PATH="$TMP/bin:$PATH"
export APEX_BIN="$APEX"
rm -f "$TMP/notified"

"$NOTICE_BIN" > "$TMP/out" 2>&1
[[ ! -f "$TMP/notified" ]] && ok "a healthy machine notifies nobody" \
                           || bad "it notified: $(cat "$TMP/notified")"

smart_refused
"$NOTICE_BIN" > "$TMP/out" 2>&1
[[ ! -f "$TMP/notified" ]] && ok "a machine it could not measure notifies nobody" \
                           || bad "it notified about an unmeasured row: $(cat "$TMP/notified")"

smart_failing
"$NOTICE_BIN" > "$TMP/out" 2>&1
if [[ -f "$TMP/notified" ]]; then
    ok "a failing disk notifies"
    has 'failing' "$TMP/notified" "and says what is wrong"
    has 'critical' "$TMP/notified" "at critical urgency"
else
    bad "a failing disk notified nobody: $(cat "$TMP/out")"
fi
before="$(wc -l < "$TMP/notified")"
"$NOTICE_BIN" > "$TMP/out" 2>&1
after="$(wc -l < "$TMP/notified")"
[[ "$before" == "$after" ]] && ok "and does not say it again tomorrow" \
                            || bad "the same warning notified twice"

smart_healthy
"$NOTICE_BIN" > "$TMP/out" 2>&1
[[ ! -f "$XDG_STATE_HOME/apex/storage-warnings.seen" ]] \
    && ok "a cleared warning forgets its digest, so its return is announced" \
    || bad "the digest survived the warning clearing"

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]] || exit 1
exit 0
