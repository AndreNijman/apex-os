#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  test-installer-keymap-boot.sh — the acceptance criterion for L-002's keymap
#  half, measured as a user experiences it: A PASSPHRASE CONTAINING A CHARACTER
#  A NON-`us` LAYOUT PRODUCES MUST UNLOCK THE VOLUME AT THE BOOT PROMPT.
#
#  ═══ WHY THIS SUITE EXISTS AND WHAT IT REFUSES TO DO ═══
#
#  Everything else about the keymap can be asserted cheaply and none of it is
#  the property. `/etc/vconsole.conf` containing KEYMAP=de is not it. A karg
#  appearing in a bootloader entry is not it. `loadkeys de` succeeding is not
#  it. The owner of an encrypted machine finds out whether the work was right
#  by pressing keys at a prompt drawn before any filesystem exists, and if the
#  answer is no there is no way back into the disk.
#
#  So: a real guest, the SHIPPED initramfs out of the APEX image, a real LUKS2
#  volume, and keystrokes delivered to an emulated keyboard as key POSITIONS.
#  The passphrase is `apexzed1`. On a German layout the `z` is produced by the
#  key a US keyboard calls `y` — QWERTZ swaps exactly those two — so the same
#  eight key positions are the right passphrase on `de` and the wrong one on
#  `us`. Nothing about the test changes between the passing and failing runs
#  except the channel that tells the initramfs which layout to load.
#
#  ═══ THE FIVE BOOTS ═══
#
#   1 cmdline-de          vconsole.keymap=de on the kernel command line
#                         -> UNLOCKS. This is what apex-install writes today.
#   2 no-channel          nothing set; the initramfs's baked KEYMAP=us
#                         -> REFUSED. The mutant: same keys, no layout.
#   3 credential-alone    a vconsole.keymap systemd credential, no help
#                         -> REFUSED, and this is a MEASUREMENT, not a bug.
#                         systemd-vconsole-setup(8): "The matching options in
#                         vconsole.conf and on the kernel command line take
#                         precedence over these credentials." dracut bakes
#                         /etc/vconsole.conf into the initramfs, so a .cred on
#                         the ESP is inert on its own.
#   4 credential+shim     the same credential, plus the dracut module's
#                         apex-vconsole-credential unit
#                         -> UNLOCKS. This is the UKI-era channel.
#   5 precedence          cmdline says `us`, credential says `de`, shim present
#                         -> REFUSED, because the command line must keep
#                         winning. A shim that quietly demoted it would break
#                         every machine installed before the UKI pivot.
#
#  Runs 3 and 4 are the pair that makes run 4 mean anything: without 3, "the
#  credential works" could be the credential doing nothing while something else
#  set the keymap.
#
#  ═══ BOUNDS ═══
#
#  No real disk is touched. The volume is a 256 MB file; the guest sees it as a
#  virtio disk. No NVRAM is written: this is a direct kernel boot with no
#  firmware at all, so there is nothing that could call efibootmgr.
#  `--device /dev/kvm` is a device pass-through, not a privilege; the qemu
#  container is unprivileged. Large artefacts go under /var/lab-scratch, never
#  /tmp, which is a 15 GB tmpfs on 29 GB of RAM here.
#
#  WHAT IT NEEDS: passwordless root, podman, /dev/kvm, an APEX-OS image in ROOT
#  podman storage, cryptsetup/losetup on the host, and the `apex-bootlab`
#  container image (bootlab/Containerfile; this script builds it if absent).
#  It cannot run on a CI runner and is listed in tests/suites-not-in-ci.txt.
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail
cd "$(dirname "$0")" || exit 1
REPO=$(cd .. && pwd)

IMAGE="${APEX_KEYMAP_IMAGE:-localhost/apex-os:daily}"
LAB="${APEX_BOOTLAB_IMAGE:-localhost/apex-bootlab}"
SCRATCH="${APEX_KEYMAP_SCRATCH:-/var/lab-scratch/apex-keymap-boot}"
PASSPHRASE="${APEX_KEYMAP_PASSPHRASE:-apexzed1}"
# The KEY POSITIONS, in US-layout names, that spell the passphrase on a German
# keyboard. `y` is the odd one: QWERTZ puts `z` there.
KEYS="a,p,e,x,y,e,d,1"

pass=0; fail=0
ok()  { printf 'PASS  %-46s %s\n' "$1" "${2:-}"; pass=$((pass+1)); }
bad() { printf 'FAIL  %-46s %s\n' "$1" "${2:-}"; fail=$((fail+1)); }
die() { printf '\nFATAL: %s\n' "$*" >&2; exit 1; }

# ── preconditions, every one of them a hard stop ────────────────────────────
command -v podman    >/dev/null || die "podman is not installed."
command -v cryptsetup>/dev/null || die "cryptsetup is not installed."
command -v losetup   >/dev/null || die "losetup is not installed."
sudo -n true 2>/dev/null        || die "this suite needs passwordless sudo."
[ -c /dev/kvm ]                 || die "/dev/kvm is absent; a TCG run takes long enough to be useless here."
sudo -n podman image exists "$IMAGE" 2>/dev/null \
    || die "$IMAGE is not in ROOT podman storage. Build it, or set APEX_KEYMAP_IMAGE."

# /tmp is a tmpfs on these machines and a 400 MB initramfs copied four times
# there is 1.6 GB of RAM. Refuse rather than discover it.
mkdir -p "$SCRATCH" || die "could not create $SCRATCH"
case "$(stat -f -c %T "$SCRATCH" 2>/dev/null)" in
  tmpfs|ramfs) die "$SCRATCH is a RAM filesystem. Set APEX_KEYMAP_SCRATCH to somewhere on a real disk." ;;
esac
W="$SCRATCH/work"
sudo -n rm -rf "$W"; mkdir -p "$W" || die "could not create $W"

if ! sudo -n podman image exists "$LAB" 2>/dev/null; then
    echo "note: building the boot lab image ($LAB) — qemu and swtpm are build-time tooling and are deliberately not installed on APEX machines"
    sudo -n podman build -t "$LAB" -f "$REPO/bootlab/Containerfile" "$REPO" >"$W/bootlab-build.log" 2>&1 \
        || die "could not build $LAB (see $W/bootlab-build.log)"
fi

LOOP=""
cleanup() {
    [ -n "$LOOP" ] && sudo -n losetup -d "$LOOP" 2>/dev/null
    sudo -n cryptsetup luksClose apexkmbuild 2>/dev/null
    return 0
}
trap cleanup EXIT

# ── stage the SHIPPED kernel and initramfs out of the image ─────────────────
# Copied, never rebuilt. A locally regenerated initramfs would be a different
# artefact from the one every APEX machine boots, and the entire difficulty
# this suite exists for is that the shipped one is built before anybody has
# chosen a keyboard layout.
echo "── staging the shipped kernel and initramfs out of $IMAGE ─────────────"
sudo -n podman run --rm -v "$W":/out:z "$IMAGE" bash -c '
set -e
kver=$(ls /usr/lib/modules | head -1)
cp -L "/usr/lib/modules/$kver/vmlinuz" /out/vmlinuz
cp -L "/usr/lib/modules/$kver/initramfs.img" /out/initramfs.img
printf "%s\n" "$kver" > /out/kver
chmod 644 /out/vmlinuz /out/initramfs.img /out/kver
' >/dev/null 2>&1 || die "could not copy the kernel and initramfs out of $IMAGE"
[ -s "$W/vmlinuz" ] && [ -s "$W/initramfs.img" ] || die "the staged kernel or initramfs is empty"
KVER=$(cat "$W/kver")
ok "staged the shipped kernel and initramfs" "$KVER, $(stat -c %s "$W/initramfs.img") bytes"

# ── the volume, with a plaintext marker inside it ───────────────────────────
# The marker is what makes "unlocked" mean unlocked. A dm-crypt mapper node
# appearing proves the device was set up; reading a known string out of the
# filesystem inside it proves the key was right.
truncate -s 256M "$W/luks.img" || die "could not create the volume file"
LOOP=$(sudo -n losetup -fP --show "$W/luks.img") || die "losetup failed"
case "$LOOP" in /dev/loop[0-9]*) : ;; *) die "losetup returned '$LOOP'" ;; esac
printf '%s' "$PASSPHRASE" | sudo -n cryptsetup luksFormat --type luks2 --batch-mode \
    --pbkdf argon2id --pbkdf-memory 32768 --pbkdf-parallel 1 --iter-time 200 \
    --label apexkm "$LOOP" - >/dev/null 2>&1 || die "luksFormat failed"
printf '%s' "$PASSPHRASE" | sudo -n cryptsetup luksOpen "$LOOP" apexkmbuild - \
    || die "the volume would not open with the passphrase that was just set"
sudo -n mkfs.ext4 -q -L apexkm -F /dev/mapper/apexkmbuild || die "mkfs.ext4 failed"
MNT=$(mktemp -d "$W/mnt.XXXXXX")
sudo -n mount /dev/mapper/apexkmbuild "$MNT" || die "could not mount the new filesystem"
echo "APEX-KEYMAP-PLAINTEXT-MARKER" | sudo -n tee "$MNT/apex-keymap-marker" >/dev/null
sudo -n umount "$MNT"; rmdir "$MNT"
sudo -n cryptsetup luksClose apexkmbuild
UUID=$(sudo -n cryptsetup luksUUID "$LOOP") || die "could not read the volume UUID"
sudo -n losetup -d "$LOOP"; LOOP=""
sudo -n chmod 644 "$W/luks.img"
ok "LUKS2 volume with a plaintext marker" "$UUID"

# ── the guest probe: a lab-only reporter, in its own cpio ───────────────────
P="$W/probe"
mkdir -p "$P/usr/bin" "$P/usr/lib/systemd/system/sysinit.target.wants" \
         "$P/var/lib/dracut/hooks/pre-mount"
cat > "$P/usr/bin/apex-keymap-probe" <<'PROBE'
#!/bin/sh
say() { printf '<0>APEX-KEYMAP-PROBE: %s\n' "$*" > /dev/kmsg 2>/dev/null || true; }
say "vconsole.conf=[$(tr '\n' ' ' < /etc/vconsole.conf 2>/dev/null)]"
say "syscreds=[$(ls /run/credentials/@system 2>/dev/null | tr '\n' ' ')]"
say "READY"
PROBE
cat > "$P/usr/lib/systemd/system/apex-keymap-probe.service" <<'PROBEUNIT'
[Unit]
Description=APEX keymap probe (lab only)
DefaultDependencies=no
Wants=systemd-vconsole-setup.service
After=systemd-vconsole-setup.service
Before=sysinit.target
[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=/usr/bin/apex-keymap-probe
StandardOutput=kmsg
StandardError=kmsg
[Install]
WantedBy=sysinit.target
PROBEUNIT
ln -sf ../apex-keymap-probe.service \
    "$P/usr/lib/systemd/system/sysinit.target.wants/apex-keymap-probe.service"
cat > "$P/var/lib/dracut/hooks/pre-mount/50-apex-keymap-result.sh" <<'RESULT'
#!/bin/sh
say() { printf '<0>APEX-KEYMAP-RESULT: %s\n' "$*" > /dev/kmsg 2>/dev/null || true; }
if [ -b /dev/mapper/apexkm ]; then
    mkdir -p /apexprobe 2>/dev/null
    if mount -o ro /dev/mapper/apexkm /apexprobe 2>/dev/null; then
        say "unlocked=yes marker=[$(cat /apexprobe/apex-keymap-marker 2>/dev/null)]"
        umount /apexprobe 2>/dev/null
    else
        say "unlocked=yes marker=[mount-failed]"
    fi
else
    say "unlocked=no"
fi
say "DONE"
poweroff -f 2>/dev/null || systemctl --force --force poweroff 2>/dev/null \
    || { echo 1 > /proc/sys/kernel/sysrq; echo o > /proc/sysrq-trigger; }
RESULT
chmod 755 "$P/usr/bin/apex-keymap-probe" \
          "$P/var/lib/dracut/hooks/pre-mount/50-apex-keymap-result.sh"
( cd "$P" && find . | cpio -o -H newc --quiet | gzip -9 ) > "$W/probe.cpio.gz" \
    || die "could not build the probe cpio"

# ── the shim cpio, built from the REPO'S OWN module files ───────────────────
# The paths are the ones files/dracut/apex-unlock-hint/module-setup.sh installs,
# and that correspondence is asserted below rather than trusted: a shim that
# works here at a path dracut never writes would be a green run for a feature
# that ships broken.
MOD="$REPO/files/dracut/apex-unlock-hint"
S="$W/shim"
mkdir -p "$S/usr/bin" "$S/usr/lib/systemd/system/sysinit.target.wants"
install -m 0755 "$MOD/apex-vconsole-credential" "$S/usr/bin/apex-vconsole-credential" \
    || die "the dracut module has no apex-vconsole-credential"
install -m 0644 "$MOD/apex-vconsole-credential.service" \
    "$S/usr/lib/systemd/system/apex-vconsole-credential.service" \
    || die "the dracut module has no apex-vconsole-credential.service"
ln -sf ../apex-vconsole-credential.service \
    "$S/usr/lib/systemd/system/sysinit.target.wants/apex-vconsole-credential.service"
( cd "$S" && find . | cpio -o -H newc --quiet | gzip -9 ) > "$W/shim.cpio.gz" \
    || die "could not build the shim cpio"

echo
echo "── the shim this suite boots is the one dracut installs ───────────────"
ms="$MOD/module-setup.sh"
mspaths=0
for want in \
    '/usr/bin/apex-vconsole-credential' \
    'apex-vconsole-credential.service' \
    'sysinit.target.wants'
do
    if grep -qF -- "$want" "$ms"; then mspaths=$((mspaths+1)); fi
done
if [ "$mspaths" = 3 ]; then
    ok "module-setup.sh installs the paths this suite boots" "3/3"
else
    bad "module-setup.sh installs the paths this suite boots" "$mspaths/3 — the cpio and the module disagree"
fi

# Concatenated cpios: the kernel unpacks each archive in turn and later entries
# overwrite earlier ones, which is exactly how dracut's own microcode prepend
# and this module's files would land.
cat "$W/initramfs.img" "$W/probe.cpio.gz"                   > "$W/initrd-probe.img"
cat "$W/initramfs.img" "$W/probe.cpio.gz" "$W/shim.cpio.gz" > "$W/initrd-shim.img"

# ── the boots ───────────────────────────────────────────────────────────────
BASE="rd.luks.uuid=$UUID rd.luks.name=$UUID=apexkm rd.luks.options=$UUID=tries=1"
BASE="$BASE root=/dev/mapper/apexkm rootfstype=ext4 rd.timeout=45 rd.shell=0"
BASE="$BASE rd.emergency=poweroff plymouth.enable=0 rd.plymouth=0"
BASE="$BASE systemd.log_target=kmsg systemd.show_status=1 loglevel=7"
BASE="$BASE console=ttyS0,115200 console=tty1"
CRED='type=11,value=io.systemd.credential:vconsole.keymap=de'

install -m 0644 ./keymap-boot-drive.py "$W/keymap-boot-drive.py" \
    || die "keymap-boot-drive.py is missing"

# boot NAME INITRD APPEND SMBIOS TIMEOUT — prints the driver's report.
boot() {
    local name="$1" initrd="$2" append="$3" smbios="$4" tmo="$5"
    printf '%s' "$append" > "$W/append-$name.txt"
    local sm=()
    [ -n "$smbios" ] && sm=(--smbios "$smbios")
    sudo -n podman run --rm --device /dev/kvm -v "$W":/w:z -w /w "$LAB" -c \
      "python3 /w/keymap-boot-drive.py --work /w --name $name --kernel /w/vmlinuz \
       --initrd /w/$initrd --disk /w/luks.img --keys $KEYS \
       --append \"\$(cat /w/append-$name.txt)\" --timeout $tmo ${sm[*]+${sm[*]}}" 2>&1
}

# serial_has NAME PATTERN
# -F, and it is not cosmetic: the marker strings contain [ and ], which a basic
# regular expression reads as a character class. The first run of this suite
# reported three failures for runs that had plainly succeeded.
serial_has() { grep -aqF -- "$2" "$W/serial-$1.log" 2>/dev/null; }
# unlocked NAME — 0 if the guest read the plaintext marker
unlocked()   { serial_has "$1" "APEX-KEYMAP-RESULT: unlocked=yes marker=[APEX-KEYMAP-PLAINTEXT-MARKER]"; }
# tried NAME — 0 if the guest got as far as a real passphrase attempt. A run
# that never reached the prompt must never be read as "the passphrase was
# rejected": that is a gate inspecting nothing.
tried()      { serial_has "$1" "APEX-KEYMAP-PROBE: READY" \
                 && serial_has "$1" "Failed to activate with specified passphrase"; }

echo
echo "── 1. the kernel command line, which is what apex-install writes ──────"
boot cmdline-de initrd-probe.img "$BASE vconsole.keymap=de" "" 240 | sed 's/^/    /'
if unlocked cmdline-de; then
    ok "vconsole.keymap=de unlocks the volume" "typed key positions $KEYS"
else
    bad "vconsole.keymap=de unlocks the volume" "see $W/serial-cmdline-de.log"
fi

echo
echo "── 2. THE MUTANT: the same keystrokes with no layout at all ───────────"
boot no-channel initrd-probe.img "$BASE" "" 200 | sed 's/^/    /'
if unlocked no-channel; then
    bad "with no keymap channel the same keys are refused" "IT UNLOCKED — the test proves nothing"
elif tried no-channel; then
    ok "with no keymap channel the same keys are refused" "cryptsetup rejected the passphrase"
else
    bad "with no keymap channel the same keys are refused" "the guest never reached a passphrase attempt — see $W/serial-no-channel.log"
fi
if serial_has no-channel "KEYMAP=us"; then
    ok "the initramfs bakes KEYMAP=us into itself" "which is why run 3 below fails"
else
    bad "the initramfs bakes KEYMAP=us into itself" "the probe did not report vconsole.conf"
fi

echo
echo "── 3. a systemd credential on its own, which is documented to lose ────"
boot credential-alone initrd-probe.img "$BASE" "$CRED" 200 | sed 's/^/    /'
if serial_has credential-alone "syscreds=[vconsole.keymap"; then
    ok "the vconsole.keymap credential reaches the initrd" "sd-stub/SMBIOS delivered it"
else
    bad "the vconsole.keymap credential reaches the initrd" "the guest saw no system credential"
fi
if unlocked credential-alone; then
    bad "a credential alone does NOT beat the baked vconsole.conf" "it unlocked — then the shim is unnecessary and this suite is wrong"
elif tried credential-alone; then
    ok "a credential alone does NOT beat the baked vconsole.conf" "as systemd-vconsole-setup(8) documents"
else
    bad "a credential alone does NOT beat the baked vconsole.conf" "the guest never reached a passphrase attempt"
fi

echo
echo "── 4. the credential plus the dracut module's shim: the UKI channel ───"
boot credential-shim initrd-shim.img "$BASE" "$CRED" 240 | sed 's/^/    /'
if serial_has credential-shim "KEYMAP=de"; then
    ok "the shim applies the credential to the initramfs" "vconsole.conf became KEYMAP=de"
else
    bad "the shim applies the credential to the initramfs" "see $W/serial-credential-shim.log"
fi
if unlocked credential-shim; then
    ok "a credential + the shim unlocks with no karg" "this is the channel a signed UKI leaves open"
else
    bad "a credential + the shim unlocks with no karg" "see $W/serial-credential-shim.log"
fi

echo
echo "── 5. the command line must keep winning ──────────────────────────────"
boot precedence initrd-shim.img "$BASE vconsole.keymap=us" "$CRED" 200 | sed 's/^/    /'
if serial_has precedence "apex-vconsole-credential: kernel command line already sets"; then
    ok "the shim stands down for an explicit karg" "it said so"
else
    bad "the shim stands down for an explicit karg" "no such line — did the shim run at all?"
fi
if unlocked precedence; then
    bad "cmdline us beats credential de" "it unlocked — the shim demoted the kernel command line"
elif tried precedence; then
    ok "cmdline us beats credential de" "us was loaded, so the keys spelled apexyed1"
else
    bad "cmdline us beats credential de" "the guest never reached a passphrase attempt"
fi

echo
echo "──────────────────────────────────────────────────────────────────────"
echo "serial logs: $W/serial-*.log"
printf '%s passed, %s failed\n' "$pass" "$fail"
[ "$fail" = 0 ] || exit 1
[ "$pass" -ge 11 ] || { echo "FATAL: only $pass assertions ran — something was skipped"; exit 1; }
exit 0
