#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  test-apex-vm.sh — assertions against the SHIPPED virtualization engine,
#  files/system/libexec/apex-vm. Nothing here re-implements it: every case runs
#  the script as a process, or sources it and calls the function the image runs.
#
#  ── Why this file exists ────────────────────────────────────────────────────
#  `apex vm create` defines a libvirt domain and `apex vm rm` deletes disk
#  images. Neither can run for real in CI — GitHub's runners mostly have no
#  /dev/kvm — and neither should run for real on the developer's laptop during
#  a test: this repository has already had a suite switch the live CPU
#  scheduler and another raise a keyring dialog, both because a test reached
#  the host. A suite that defined a domain on somebody's machine would be the
#  same mistake with a virbr0 attached.
#
#  So libvirt is FAKED. $PATH is reduced to a directory holding recording stubs
#  for `virsh`, `qemu-img`, `mkfs.vfat`, `mcopy` and `lsusb`, and the suite
#  REFUSES TO RUN if `command -v virsh` does not resolve to the stub.
#
#  ── The thing this suite is really about ────────────────────────────────────
#  The domain XML. Six of P2-008's flows are elements in that XML, and three of
#  them — SMM, shared memory backing, an xHCI controller — are elements that
#  look optional and are not: dropping any one produces a domain that DEFINES
#  successfully and then refuses to START, which is the worst failure shape
#  available because the user gets a success and then an error unrelated to
#  what they typed.
#
#  The XML generator is a pure function for exactly that reason, so each
#  element can be asserted without a hypervisor.
#
#  ── What is asserted ────────────────────────────────────────────────────────
#    * VM names, which become libvirt domain names AND path components under a
#      directory this engine removes recursively — so a traversal is refused,
#      not concatenated
#    * the six flows, element by element, including the three above
#    * the DEFAULTS: an unqualified `create` produces an enforcing, TPM-carrying
#      domain, and no code path emits <graphics>
#    * --network bridge is REFUSED with a reason, not silently downgraded
#    * `apex vm run`'s egress: the mcopy argv names each nomination, the loop
#      runs over nominations rather than over the volume, and --egress-to
#      absent means nothing leaves
#    * the removal fences, including that a name pointing outside the root is
#      refused rather than removed
#
#  ── What it deliberately does NOT do ────────────────────────────────────────
#  No domain is defined, no disk is created, no guest is booted, nothing is
#  written outside a temp directory. The live half — booting real guests
#  through this engine against a real virtqemud — is tests/vmlab/run-vmlab,
#  which reports could-not-run with a reason when the machine has no KVM.
#
#  PASS = every case prints exactly what it should, with a non-zero exit where
#         one is expected.
#
#  Run from anywhere: ./tests/test-apex-vm.sh
# ─────────────────────────────────────────────────────────────────────────────
# `set +e`, as in every suite here: this one COUNTS failures instead of
# aborting, and many assertions run commands that exit non-zero on purpose.
set -uo pipefail
set +e
cd "$(dirname "$0")" || exit 2

ENGINE=../files/system/libexec/apex-vm
[ -f "$ENGINE" ] || { echo "cannot find $ENGINE"; exit 2; }
ENGINE=$(cd "$(dirname "$ENGINE")" && pwd)/$(basename "$ENGINE")

WORK=$(mktemp -d /tmp/apex-vm-test.XXXXXX)
trap 'rm -rf "$WORK"' EXIT

pass=0; fail=0

ok()  { printf 'PASS  %-58s\n' "$1"; pass=$((pass+1)); }
bad() { printf 'FAIL  %-58s %s\n' "$1" "$2"; fail=$((fail+1)); }

is() {
    local name=$1 want=$2 got=$3
    if [ "$got" = "$want" ]; then ok "$name"
    else bad "$name" "expected $(printf '%q' "$want"), got $(printf '%q' "$got")"; fi
}
has() {
    local name=$1 want=$2 hay=$3
    if grep -qF -- "$want" <<<"$hay"; then ok "$name"
    else bad "$name" "expected $(printf '%q' "$want") in: $(head -5 <<<"$hay" | tr '\n' ' ')"; fi
}
hasnt() {
    local name=$1 unwanted=$2 hay=$3
    if grep -qF -- "$unwanted" <<<"$hay"; then
        bad "$name" "found $(printf '%q' "$unwanted") and should not have"
    else ok "$name"; fi
}

# ── the sandbox the whole suite runs in ─────────────────────────────────────
BIN="$WORK/bin"
FAKEHOME="$WORK/home"
VMHOME="$WORK/vms"
CALLS="$WORK/calls"
FW="$WORK/firmware"
mkdir -p "$BIN" "$FAKEHOME" "$VMHOME" "$FW"

# `virsh`, recording. It answers `version` so the engine's session-daemon probe
# succeeds, `domstate` so the wait loop in `run` terminates, and `dominfo` with
# a failure so `create` does not think a domain already exists. Everything else
# is recorded and exits 0.
cat > "$BIN/virsh" <<EOF
#!/usr/bin/env bash
{ printf 'virsh'; printf ' <%s>' "\$@"; printf '\n'; } >> "$CALLS"
# The XML libvirt was ACTUALLY handed, kept aside. \`apex vm run\` deletes its
# own scratch directory on teardown — that is the property under test — so the
# domain it defined cannot be read back from disk afterwards. Copying it here
# is also the stronger assertion: it is what the hypervisor received, not what
# the engine left lying around.
if [ "\$3" = define ] && [ -f "\$4" ]; then cp -- "\$4" "$WORK/defined.xml"; fi
for a in "\$@"; do
    case "\$a" in
        version)  echo "Using library: libvirt 11.6.0"; exit 0 ;;
        domstate) echo "\${FAKE_DOMSTATE:-shut off}"; exit 0 ;;
        dominfo)  exit \${FAKE_DOMINFO_RC:-1} ;;
        dumpxml)  cat "\${FAKE_DUMPXML:-/dev/null}"; exit 0 ;;
    esac
done
exit \${FAKE_VIRSH_RC:-0}
EOF

# `qemu-img`, recording. `create` and `convert` must leave a file behind, or
# the engine's own postconditions would be asserting against nothing.
cat > "$BIN/qemu-img" <<EOF
#!/usr/bin/env bash
{ printf 'qemu-img'; printf ' <%s>' "\$@"; printf '\n'; } >> "$CALLS"
out=""
prev=""
for a in "\$@"; do
    case "\$prev" in -f|-O) ;; esac
    out="\$a"; prev="\$a"
done
case "\$1" in
    create)  : > "\${@: -2:1}" ;;
    convert) : > "\$out" ;;
esac
exit \${FAKE_QEMUIMG_RC:-0}
EOF

for t in mkfs.vfat mcopy lsusb swtpm; do
    cat > "$BIN/$t" <<EOF
#!/usr/bin/env bash
{ printf '$t'; printf ' <%s>' "\$@"; printf '\n'; } >> "$CALLS"
exit \${FAKE_${t//./_}_RC:-0}
EOF
done

chmod 0755 "$BIN"/*

# virtiofsd is a FILE the engine tests for, not a command it runs, so the
# fixture is an executable file rather than a stub on $PATH. `truncate` is
# deliberately NOT stubbed: `apex vm run` builds real (sparse) volume images
# and the assertions about what was written to them need the files to exist.
VIRTIOFSD="$WORK/virtiofsd"
printf '#!/bin/sh\nexit 0\n' > "$VIRTIOFSD"
chmod 0755 "$VIRTIOFSD"

# A firmware descriptor carrying both features, so the engine's availability
# probe has something true to find. It is a FIXTURE rather than the host's
# /usr/share/qemu/firmware: a developer machine with no OVMF installed must get
# the same result as one with it.
cat > "$FW/30-fixture-sb-enrolled.json" <<'EOF'
{ "description": "fixture", "interface-types": ["uefi"],
  "features": ["enrolled-keys", "requires-smm", "secure-boot"] }
EOF

export PATH="$BIN:/usr/bin:/bin"
export HOME="$FAKEHOME"
export APEX_VM_HOME="$VMHOME"
export APEX_VM_FIRMWARE_DIRS="$FW"
export APEX_VM_VIRTIOFSD="$VIRTIOFSD"

# The refusal that makes every other assertion meaningful. A suite that fell
# back to the real virsh would define domains on the developer's machine.
if [ "$(command -v virsh)" != "$BIN/virsh" ]; then
    echo "FATAL: virsh does not resolve to the stub — refusing to run"; exit 2
fi

calls() { cat "$CALLS" 2>/dev/null; }
reset_calls() { : > "$CALLS"; }
run_engine() { reset_calls; bash "$ENGINE" "$@" 2>&1; }

echo "── names, which become domain names and paths this engine removes ──"

for n in "../escape" "/abs" "UPPER" "has space" "" "a.b" "x;rm -rf /"; do
    out=$(run_engine create "$n" 2>&1); rc=$?
    if [ "$rc" -ne 0 ]; then ok "create refuses the name $(printf '%q' "$n")"
    else bad "create refuses the name $(printf '%q' "$n")" "it was accepted"; fi
done
out=$(run_engine create "ok-name-1" --disk 1G)
is "a plain name is accepted" 0 $?

echo
echo "── the six flows, element by element ──"

# The domain the engine actually wrote, read from disk rather than regenerated:
# asserting a string this suite also produced would prove nothing.
XML="$VMHOME/ok-name-1/domain.xml"
xml=$(cat "$XML" 2>/dev/null)

has "create: the disk is the VM's own qcow2" "<source file='$VMHOME/ok-name-1/disk.qcow2'/>" "$xml"
has "create: q35, which is the only machine type with SMM" "machine='q35'" "$xml"
has "create: KVM, not TCG" "<domain type='kvm'>" "$xml"

# Secure Boot. Three separate facts, because any one of them alone is a domain
# that reports enforcement it does not have.
has "secure boot: the loader is marked secure" "<loader secure='yes'/>" "$xml"
has "secure boot: the secure-boot feature is declared" "name='secure-boot'" "$xml"
has "secure boot: ENROLLED KEYS are declared, not just secure-boot" \
    "enabled='yes' name='enrolled-keys'" "$xml"
# The element libvirt refuses <loader secure='yes'> without. Dropping it does
# not produce an insecure VM, it produces a define that fails — correct, and
# opaque. This is the assertion that keeps the reason findable.
has "secure boot: SMM is on" "<smm state='on'/>" "$xml"
# And no path is written into the firmware element at all: libvirt resolves it
# from the descriptors. A hardcoded path here was the defect that produced a
# setup-mode varstore.
hasnt "secure boot: no firmware path is hardcoded" "OVMF_" "$xml"

# TPM
has "tpm: an emulated backend" "<backend type='emulator' version='2.0'/>" "$xml"
has "tpm: CRB, the interface a modern UEFI guest looks for" "model='tpm-crb'" "$xml"

# nvram format, which is what makes snapshots possible at all
has "snapshot: the variable store is qcow2, so it can hold one" \
    "<nvram format='qcow2'>" "$xml"

# USB
has "usb: an xHCI controller with ports exists before any device needs one" \
    "<controller type='usb' model='qemu-xhci' ports='15'/>" "$xml"

# headless
hasnt "headless: no graphics element" "<graphics" "$xml"
hasnt "headless: no spice" "spice" "$xml"
has "headless: a serial console on a file" \
    "<source path='$VMHOME/ok-name-1/console.log'/>" "$xml"

echo
echo "── defaults: an unqualified create must not be the weak one ──"

record=$(cat "$VMHOME/ok-name-1/record.json" 2>/dev/null)
has "default: secure boot is recorded as on" '"secure_boot": true' "$record"
has "default: a TPM is recorded as present" '"tpm": true' "$record"
has "default: the session URI is recorded" '"uri": "qemu:///session"' "$record"

out=$(run_engine create weak --disk 1G --no-secure-boot --no-tpm)
wxml=$(cat "$VMHOME/weak/domain.xml" 2>/dev/null)
has "--no-secure-boot: the loader is not secure" "<loader secure='no'/>" "$wxml"
has "--no-secure-boot: the features are declared disabled, not dropped" \
    "enabled='no' name='secure-boot'" "$wxml"
hasnt "--no-secure-boot: SMM is not emitted" "<smm" "$wxml"
hasnt "--no-tpm: no tpm element" "<tpm " "$wxml"

echo
echo "── the network refusal, which is why session mode was chosen ──"

out=$(run_engine create bridged --network bridge); rc=$?
is "--network bridge exits 2 (a refusal, not an error)" 2 "$rc"
has "--network bridge says a bridge outlives the VM" "outlive this VM" "$out"
has "--network bridge names the alternative" "qemu:///system" "$out"
[ -e "$VMHOME/bridged" ] \
    && bad "--network bridge creates nothing" "$VMHOME/bridged exists" \
    || ok "--network bridge creates nothing"

out=$(run_engine create netless --disk 1G --network none)
nxml=$(cat "$VMHOME/netless/domain.xml" 2>/dev/null)
hasnt "--network none emits no interface at all" "<interface" "$nxml"
has "--network user emits a user-mode interface" "<interface type='user'>" "$xml"

echo
echo "── share: virtiofs, and the memory element it cannot work without ──"

mkdir -p "$WORK/src" "$WORK/ref"
out=$(run_engine create shared --disk 1G --share "$WORK/src:src" --share-ro "$WORK/ref:ref")
sxml=$(cat "$VMHOME/shared/domain.xml" 2>/dev/null)
has "share: a virtiofs driver" "<driver type='virtiofs' queue='1024'/>" "$sxml"
has "share: the source directory" "<source dir='$WORK/src'/>" "$sxml"
has "share: the tag the guest mounts" "<target dir='src'/>" "$sxml"
has "share: virtiofsd is named explicitly" "<binary path=" "$sxml"
has "share-ro: the read-only share is marked read-only" "<readonly/>" "$sxml"
# THE element. Without it qemu exits with a message about guest RAM, and the
# user reads a memory problem where there is a missing access mode.
has "share: memfd-backed memory" "<source type='memfd'/>" "$sxml"
has "share: memory access is shared" "<access mode='shared'/>" "$sxml"
# And it is NOT emitted on a domain with no share, because shared memory
# backing is a real behaviour change for a VM that does not need it.
hasnt "no share: no shared memory backing" "<memoryBacking>" "$xml"

out=$(run_engine create badshare --disk 1G --share "/nonexistent/x:tag"); rc=$?
is "share: a source that is not a directory is refused" 1 "$rc"
out=$(run_engine create badtag --disk 1G --share "$WORK/src:../evil"); rc=$?
is "share: a tag that is not a plain name is refused" 1 "$rc"

echo
echo "── usb ──"

out=$(run_engine create usbvm --disk 1G --usb 046d:c52b)
uxml=$(cat "$VMHOME/usbvm/domain.xml" 2>/dev/null)
has "usb: the vendor id, hex-prefixed" "<vendor id='0x046d'/>" "$uxml"
has "usb: the product id" "<product id='0xc52b'/>" "$uxml"
# managed='yes' is what makes libvirt take the device from the host driver and
# give it back. Without it the guest gets an error and the host keeps the
# device, which reads as passthrough not working.
has "usb: managed, so the host gives the device up and gets it back" \
    "managed='yes'" "$uxml"
out=$(run_engine create usbbad --disk 1G --usb "046d"); rc=$?
is "usb: a malformed id is refused" 1 "$rc"
out=$(run_engine create usbcase --disk 1G --usb "046D:C52B")
has "usb: an uppercase id is normalised, not rejected" "id='0x046d'" \
    "$(cat "$VMHOME/usbcase/domain.xml" 2>/dev/null)"

echo
echo "── snapshot: the argv, including --atomic ──"

out=$(run_engine snapshot create ok-name-1 before)
has "snapshot create names the domain and the snapshot" \
    "<snapshot-create-as> <--domain> <ok-name-1> <--name> <before>" "$(calls)"
has "snapshot create is atomic" "<--atomic>" "$(calls)"
out=$(run_engine snapshot revert ok-name-1 before)
has "snapshot revert names both" \
    "<snapshot-revert> <--domain> <ok-name-1> <--snapshotname> <before>" "$(calls)"
out=$(run_engine snapshot create ok-name-1 "../evil"); rc=$?
is "snapshot: a traversal in the snapshot name is refused" 1 "$rc"

echo
echo "── rm: the fences on a recursive removal ──"

run_engine create doomed --disk 1G >/dev/null
[ -d "$VMHOME/doomed" ] || bad "rm fixture exists" "create did not make it"
out=$(run_engine rm doomed)
[ -e "$VMHOME/doomed" ] \
    && bad "rm removes the VM's directory" "it is still there" \
    || ok "rm removes the VM's directory"
has "rm undefines with --nvram, so keys are not inherited" "<--nvram>" "$(calls)"

# The fence that matters: a directory that is a SYMLINK must hard-exit rather
# than have its target removed.
mkdir -p "$WORK/elsewhere"
ln -s "$WORK/elsewhere" "$VMHOME/linked"
out=$(run_engine rm linked); rc=$?
is "rm refuses a VM directory that is a symlink" 2 "$rc"
[ -d "$WORK/elsewhere" ] \
    && ok "rm did not follow the symlink" \
    || bad "rm did not follow the symlink" "the target was removed"
rm -f "$VMHOME/linked"

out=$(run_engine rm "../../etc"); rc=$?
[ "$rc" -ne 0 ] && ok "rm refuses a traversal" || bad "rm refuses a traversal" "rc=$rc"

echo
echo "── apex vm run: P2-009's egress boundary ──"

GUEST="$WORK/guest.qcow2"; : > "$GUEST"
EGRESS="$WORK/out"

out=$(run_engine run --image "$GUEST" --egress-to "$EGRESS" -- true); rc=$?
is "run: --egress-to with nothing nominated is refused" 1 "$rc"
has "run: and it says nothing would leave" "nothing would leave" "$out"

out=$(run_engine run --image "$GUEST" --network user -- true); rc=$?
is "run: --network is refused outright (exit 2)" 2 "$rc"
has "run: and it says why" "exfiltration path" "$out"

for n in "../etc/passwd" "sub/dir.txt" "*" "a?b" ".hidden"; do
    out=$(run_engine run --image "$GUEST" --egress "$n" --egress-to "$EGRESS" -- true); rc=$?
    if [ "$rc" -ne 0 ]; then ok "run: --egress refuses $(printf '%q' "$n")"
    else bad "run: --egress refuses $(printf '%q' "$n")" "accepted"; fi
done

out=$(run_engine run --image "$GUEST" --name run-aaa --copy-in "$WORK/src" \
        --egress report.json --egress log.txt --egress-to "$EGRESS" -- make -j4)
c=$(calls)
has "run: the copy-in volume is labelled APEXIN" "<-n> <APEXIN>" "$c"
has "run: the egress volume is labelled APEXOUT" "<-n> <APEXOUT>" "$c"
has "run: the task script is staged into the copy-in volume" "<::/task.sh>" "$c"
has "run: --copy-in lands in the copy-in volume" "<::/src>" "$c"
# THE assertion. One mcopy per NOMINATION, naming that file — never a wildcard,
# never a listing of the volume. Iterating the image and copying what matched
# would let the guest choose what leaves by choosing filenames.
has "run: egress reads report.json by name" "<::/report.json>" "$c"
has "run: egress reads log.txt by name" "<::/log.txt>" "$c"
hasnt "run: egress never globs the volume" "::/*" "$c"
hasnt "run: egress never lists the volume" "mdir" "$c"
is "run: exactly one mcopy per nominated file, plus the two stage-in copies" \
    4 "$(grep -c '^mcopy' <<<"$c")"

# Teardown, which is the property that makes it disposable. The directory is
# gone, so the domain XML cannot be read back from it — which is why the
# no-network half is asserted on a separate run below, against the file while
# it still exists.
if [ -e "$VMHOME/run-aaa" ]; then
    bad "run: teardown removed the VM's directory" "$VMHOME/run-aaa survived"
else
    ok "run: teardown removed the VM's directory"
fi
has "run: the domain was defined and started" "<define>" "$c"
has "run: and undefined again on teardown" "<undefine>" "$c"

# The domain a disposable run actually handed libvirt, from the stub's copy.
# It cannot be read from the engine's directory, because teardown deleted that
# — which is the property asserted immediately above.
dxml=$(cat "$WORK/defined.xml" 2>/dev/null)
hasnt "run: the disposable domain has no network interface at all" "<interface" "$dxml"
hasnt "run: and no shared memory backing to reach the host through" "<memoryBacking>" "$dxml"
hasnt "run: and no filesystem share" "<filesystem" "$dxml"
has "run: the copy-in volume is attached read-only" \
    "<source file='$VMHOME/run-aaa/in.img'/><target dev='vdb' bus='virtio'/><readonly/>" "$dxml"
has "run: the egress volume is attached writable" \
    "<source file='$VMHOME/run-aaa/out.img'/><target dev='vdc' bus='virtio'/>" "$dxml"

# Without --egress-to, nothing leaves whatever --egress says.
reset_calls
out=$(bash "$ENGINE" run --image "$GUEST" --egress report.json -- true 2>&1)
c=$(calls)
has "run: without --egress-to it says nothing left" "nothing left the VM" "$out"
hasnt "run: without --egress-to no file is read out of the volume" "<::/report.json>" "$c"

echo
echo "── doctor, and the refusal every verb shares ──"

# With the stubs on PATH the stack looks present except for the pieces that are
# files, so doctor must report a mixture rather than all-ok.
out=$(APEX_VM_VIRTIOFSD="$WORK/nope" bash "$ENGINE" doctor 2>&1); rc=$?
has "doctor: names the missing piece" "virtiofs daemon" "$out"
has "doctor: prints the one install command" "sudo apex install" "$out"
[ "$rc" -ne 0 ] && ok "doctor exits non-zero when something is missing" \
                || bad "doctor exits non-zero when something is missing" "rc=0"

out=$(PATH="/usr/bin:/bin" APEX_VM_VIRSH="$WORK/nope-virsh" bash "$ENGINE" create x 2>&1); rc=$?
is "create refuses before creating anything when libvirt is absent" 1 "$rc"
has "create names the package set" "apex install qemu-kvm" "$out"

echo
echo "── nothing escaped the sandbox ──"

stray=$(find "$FAKEHOME" -mindepth 1 2>/dev/null | head -5)
if [ -z "$stray" ]; then ok "the fake HOME is still empty"
else bad "the fake HOME is still empty" "$stray"; fi

echo
printf 'apex-vm: %d passed, %d failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ]
