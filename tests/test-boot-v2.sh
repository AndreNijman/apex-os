#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  test-boot-v2.sh — executable assertions for roadmap §22's boot v2.
#
#  Two modes, because the checks split cleanly by what they need:
#
#    (no argument)     Everything that needs no toolchain: the shipped units,
#                      the health gate's actual exit codes, the build-time
#                      scripts' refusals, the tripwire against a script that
#                      touches a real boot path, and the schema parity between
#                      what apex-boot-health WRITES and what apex boot status
#                      READS. Wired into pr-validation.yml's `static` job,
#                      which has no path filter.
#
#    --with-binary     Adds the cases that drive the built `apex` binary
#                      against fixture roots. Wired into the `rust` job, the
#                      only one with a toolchain. It DIES if the binary is
#                      absent rather than skipping: a skipped check counts as
#                      success, which is the bug docs/p1-progress.md already
#                      records twice.
#
#  Why the split is not "put it all in `rust`": that job fires on
#  ^(apexd/|config/sysprofiles/|tests/). A PR touching only files/system/units
#  would skip it, and a skipped job passes. The units and the tripwire are
#  exactly what such a PR changes.
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WITH_BINARY=0
[[ "${1:-}" == "--with-binary" ]] && WITH_BINARY=1

PASS=0 FAIL=0
ok()  { PASS=$((PASS + 1)); printf '  ok   %s\n' "$*"; }
bad() { FAIL=$((FAIL + 1)); printf '  FAIL %s\n' "$*"; }
sec() { printf '\n== %s ==\n' "$*"; }
eq()  { [[ "$1" == "$2" ]] && ok "$3 == $1" || bad "$3: want '$1', got '$2'"; }

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

HEALTH="$REPO/files/system/libexec/apex-boot-health"
UNIT_HEALTH="$REPO/files/system/units/apex-boot-health.service"
UNIT_NOTICE="$REPO/files/system/units/apex-boot-notice.service"
BOOTRS="$REPO/apexd/apex/src/boot.rs"
BASECF="$REPO/Containerfile.base"
LOADER_GUID=4a67b082-0a4c-41cf-b6c7-440b29bb8c4f

for f in "$HEALTH" "$UNIT_HEALTH" "$UNIT_NOTICE" "$BOOTRS" "$BASECF"; do
    [[ -f "$f" ]] || { echo "FATAL: missing $f" >&2; exit 1; }
done
# A non-executable helper makes every exit-code assertion below return 126,
# which reads as "the gate refused" unless the assertions demand rc=1. Both
# guards are kept: this one names the cause, and the rc=1 checks catch a
# regression this one would miss.
[[ -x "$HEALTH" ]] || { echo "FATAL: $HEALTH is not executable in the repo" >&2; exit 1; }

# ═════════════════════════════════════════════════════════════════════════════
sec "the opt-in is structural, not a claim in a comment"
# This is the property the whole section is most likely to lose silently. GRUB
# is the default for every published image (AGENTS.md boot-path rule 5), so
# every unit shipped here must be incapable of doing anything on a GRUB boot.
# The switch is systemd-boot's own LoaderBootCountPath EFI variable, which is
# also what systemd-bless-boot-generator conditions on.
for u in "$UNIT_HEALTH" "$UNIT_NOTICE"; do
    n="$(basename "$u")"
    if grep -qx "ConditionPathExists=/sys/firmware/efi/efivars/LoaderBootCountPath-$LOADER_GUID" "$u"; then
        ok "$n is conditioned on LoaderBootCountPath"
    else
        bad "$n has no LoaderBootCountPath condition — it would run on a GRUB machine"
    fi
done

# RequiredBy vs WantedBy is the difference between "a failed health check
# blocks the blessing" and "a failed health check is a warning".
grep -qx 'RequiredBy=boot-complete.target' "$UNIT_HEALTH" \
    && ok "apex-boot-health is RequiredBy=boot-complete.target" \
    || bad "apex-boot-health must be RequiredBy=boot-complete.target, or a failed health check blesses anyway"
grep -qx 'Before=boot-complete.target' "$UNIT_HEALTH" \
    && ok "apex-boot-health is ordered Before=boot-complete.target" \
    || bad "apex-boot-health must be Before=boot-complete.target"
grep -qx 'WantedBy=boot-complete.target' "$UNIT_NOTICE" \
    && ok "apex-boot-notice is WantedBy (reporting must not fail a healthy boot)" \
    || bad "apex-boot-notice must be WantedBy=boot-complete.target, not RequiredBy"

# Containerfile.base must enable both, and must itself assert the condition.
grep -q 'systemctl enable apex-boot-health.service' "$BASECF" \
    && ok "Containerfile.base enables apex-boot-health.service" \
    || bad "Containerfile.base does not enable apex-boot-health.service"
grep -q 'boot-complete.target.requires/apex-boot-health.service' "$BASECF" \
    && ok "Containerfile.base checks the RequiredBy symlink systemctl created" \
    || bad "Containerfile.base does not verify the enablement it performed"

# ═════════════════════════════════════════════════════════════════════════════
sec "nothing shipped into the image touches a real boot path"
# AGENTS.md boot-path rule 1. The match is on EXECUTABLE lines only: both units
# and the health helper contain comments naming the commands they refuse to
# run, and a tripwire a comment can trip is a tripwire nobody can keep green.
DANGER='(bootctl[[:space:]]+(install|update)|bootupctl|grub2-install|grub2-mkconfig|efibootmgr[[:space:]]+-[cBo])'
mapfile -t shipped < <(find "$REPO/files" -type f \
    \( -path '*/libexec/*' -o -name '*.service' -o -name '*.timer' \) | sort)
(( ${#shipped[@]} > 0 )) || bad "found no shipped units or helpers to scan — the scan is vacuous"
hits=0
for f in "${shipped[@]}"; do
    if grep -nE "^[^#]*\b$DANGER" "$f" >/dev/null 2>&1; then
        bad "$f runs a boot-path command on an executable line"
        hits=$((hits + 1))
    fi
done
(( hits == 0 )) && ok "${#shipped[@]} shipped units/helpers contain no boot-path command"

# The tripwire's own two controls. Without these the check above could be
# reporting "clean" because the pattern never matches anything at all — a
# failure mode this project has hit with a check script that did not exist and
# exited 127.
printf '#!/bin/sh\n# this comment mentions bootctl install and bootupctl on purpose\necho hi\n' \
    > "$TMP/comment-only"
if grep -nE "^[^#]*\b$DANGER" "$TMP/comment-only" >/dev/null 2>&1; then
    bad "inverse control: a comment mentioning 'bootctl install' trips the tripwire (false red)"
else
    ok "inverse control: a comment mentioning 'bootctl install' does not trip the tripwire"
fi
printf '#!/bin/sh\nbootctl install --esp-path=/boot/efi\n' > "$TMP/really-does-it"
if grep -nE "^[^#]*\b$DANGER" "$TMP/really-does-it" >/dev/null 2>&1; then
    ok "forward control: a real 'bootctl install' line does trip the tripwire"
else
    bad "forward control: a real 'bootctl install' line does NOT trip the tripwire — the scan proves nothing"
fi

# ═════════════════════════════════════════════════════════════════════════════
sec "the health gate's exit codes are the rollback mechanism"
# apex-boot-health calls `systemctl` unqualified, so a fake earlier in PATH is
# enough to drive every branch. This tests the shipped script, not a copy of
# its logic.
mkfake() {  # mkfake DIR ACTIVE_UNITS...
    local dir="$1"; shift
    mkdir -p "$dir"
    {
        printf '#!/usr/bin/env bash\n'
        printf 'ACTIVE="%s"\n' "$*"
        printf 'case "$1" in\n'
        printf '  get-default) echo graphical.target;;\n'
        # `cat` decides which units EXIST. dbus-broker exists so the script
        # picks a real bus name; greetd exists so the display-manager branch is
        # actually exercised rather than skipped.
        printf '  cat) case "$2" in dbus-broker.service|greetd.service|apexd.service|systemd-logind.service) echo "[Unit]";; *) exit 1;; esac;;\n'
        printf '  is-active) for u in $ACTIVE; do [[ "$u" == "$2" ]] && { echo active; exit 0; }; done; echo inactive; exit 3;;\n'
        printf '  *) exit 1;;\n'
        printf 'esac\n'
    } > "$dir/systemctl"
    chmod +x "$dir/systemctl"
}

ALL_GOOD="graphical.target apexd.service systemd-logind.service dbus-broker.service greetd.service"

# (a) No LoaderBootCountPath: a GRUB machine. Must succeed and do nothing.
mkdir -p "$TMP/efivars-grub" "$TMP/state-grub"
mkfake "$TMP/bin-a" "$ALL_GOOD"
rc=0
PATH="$TMP/bin-a:$PATH" APEX_BOOT_EFIVARS="$TMP/efivars-grub" APEX_BOOT_STATE="$TMP/state-grub" \
    "$HEALTH" check >"$TMP/out-a" 2>&1 || rc=$?
eq 0 "$rc" "check on a GRUB machine exits"
grep -q 'boot counting is not in effect' "$TMP/out-a" \
    && ok "and says why" || bad "did not explain why it did nothing"
[[ ! -e "$TMP/state-grub/last-health.json" ]] \
    && ok "and wrote no verdict (a GRUB boot has nothing to bless)" \
    || bad "wrote a verdict on a machine with no boot counter"

# (b) Counting in effect, everything healthy.
mkdir -p "$TMP/efivars-sd" "$TMP/state-good"
printf '\x07\x00\x00\x00' > "$TMP/efivars-sd/LoaderBootCountPath-$LOADER_GUID"
printf '\x07\x00\x00\x00a\0p\0e\0x\0-\0n\0e\0w\0.\0e\0f\0i\0' \
    > "$TMP/efivars-sd/LoaderEntrySelected-$LOADER_GUID"
mkfake "$TMP/bin-b" "$ALL_GOOD"
rc=0
PATH="$TMP/bin-b:$PATH" APEX_BOOT_EFIVARS="$TMP/efivars-sd" APEX_BOOT_STATE="$TMP/state-good" \
    "$HEALTH" check >"$TMP/out-b" 2>&1 || rc=$?
eq 0 "$rc" "check with a healthy system exits"
eq good "$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["verdict"])' \
           "$TMP/state-good/last-health.json" 2>/dev/null || echo MISSING)" \
   "the recorded verdict"
eq 'apex-new.efi' "$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["entry"])' \
                     "$TMP/state-good/last-health.json" 2>/dev/null || echo MISSING)" \
   "the recorded entry, decoded from the UTF-16 EFI variable"

# (c) Each critical unit, dropped one at a time. A gate that only notices when
# everything is down is not a gate. `-ge`-style leniency here would mean a
# machine with no desktop and no apexd getting blessed.
for down in graphical.target apexd.service systemd-logind.service dbus-broker.service greetd.service; do
    active="${ALL_GOOD/$down/}"
    mkfake "$TMP/bin-c" "$active"
    mkdir -p "$TMP/state-c"
    rc=0
    PATH="$TMP/bin-c:$PATH" APEX_BOOT_EFIVARS="$TMP/efivars-sd" APEX_BOOT_STATE="$TMP/state-c" \
        "$HEALTH" check >"$TMP/out-c" 2>&1 || rc=$?
    # Exactly 1, not merely non-zero. An early version of this suite passed
    # here with rc=126 — "cannot execute", because the script was not
    # executable in the repo — and reported the gate as working.
    if (( rc == 1 )); then
        ok "check fails with rc=1 when $down is inactive"
    else
        bad "check returned $rc with $down inactive (want 1; 126 means it never ran)"
    fi
    grep -q "UNHEALTHY: $down is not active" "$TMP/out-c" \
        && ok "  and names $down" \
        || bad "  but did not name $down: $(cat "$TMP/out-c")"
    eq bad "$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["verdict"])' \
              "$TMP/state-c/last-health.json" 2>/dev/null || echo MISSING)" \
       "  the recorded verdict with $down down"
done

# ═════════════════════════════════════════════════════════════════════════════
sec "the rollback notice, and the missing-key trap it would otherwise hit"
# bootctl OMITS triesLeft for an entry with no counter, which is what a blessed
# deployment looks like. Code that reads it as `get("triesLeft", 0) == 0` marks
# every healthy deployment as failed and fires the notice on a machine that
# never rolled back. This fixture is the real shape, taken from
# `bootctl list --json` run against an ESP a VM had actually booted.
cat > "$TMP/entries-rolledback.json" <<'JSON'
[
  {"type":"type2","id":"apex-good.efi","path":"/boot/EFI/Linux/apex-good.efi",
   "title":"APEX-OS","isDefault":true},
  {"type":"type2","id":"apex-new.efi","path":"/boot/EFI/Linux/apex-new+0-3.efi",
   "title":"APEX-OS","triesLeft":0,"triesDone":3,"isDefault":false}
]
JSON
cat > "$TMP/entries-allgood.json" <<'JSON'
[
  {"type":"type2","id":"apex-good.efi","path":"/boot/EFI/Linux/apex-good.efi",
   "title":"APEX-OS","isDefault":true},
  {"type":"type2","id":"apex-new.efi","path":"/boot/EFI/Linux/apex-new.efi",
   "title":"APEX-OS","isDefault":false}
]
JSON
cat > "$TMP/entries-ontrial.json" <<'JSON'
[
  {"type":"type2","id":"apex-good.efi","path":"/boot/EFI/Linux/apex-good.efi",
   "title":"APEX-OS","isDefault":true},
  {"type":"type2","id":"apex-new.efi","path":"/boot/EFI/Linux/apex-new+2-1.efi",
   "title":"APEX-OS","triesLeft":2,"triesDone":1,"isDefault":false}
]
JSON

mkdir -p "$TMP/efivars-good" "$TMP/state-n"
printf '\x07\x00\x00\x00' > "$TMP/efivars-good/LoaderBootCountPath-$LOADER_GUID"
printf '\x07\x00\x00\x00a\0p\0e\0x\0-\0g\0o\0o\0d\0.\0e\0f\0i\0' \
    > "$TMP/efivars-good/LoaderEntrySelected-$LOADER_GUID"

run_notice() {
    APEX_BOOT_EFIVARS="$TMP/efivars-good" APEX_BOOT_STATE="$TMP/state-n" \
    APEX_BOOT_BOOTCTL_JSON="$1" "$HEALTH" notice >"$TMP/out-n" 2>&1
}

rc=0; run_notice "$TMP/entries-rolledback.json" || rc=$?
eq 0 "$rc" "notice with an exhausted entry exits"
if [[ -f "$TMP/state-n/rollback-notice.json" ]]; then
    ok "wrote a rollback notice"
    eq 'apex-new.efi' \
       "$(python3 -c 'import json,sys;d=json.load(open(sys.argv[1]));print(d["failedEntries"][0]["id"])' \
          "$TMP/state-n/rollback-notice.json")" "the failed entry it names"
    eq 1 "$(python3 -c 'import json,sys;print(len(json.load(open(sys.argv[1]))["failedEntries"]))' \
            "$TMP/state-n/rollback-notice.json")" "the exact number of failed entries"
    eq 'apex-good.efi' \
       "$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["runningEntry"])' \
          "$TMP/state-n/rollback-notice.json")" "the entry it says is running"
else
    bad "no rollback notice written for an exhausted entry"
fi

# THE TRAP. Two blessed entries, neither carrying triesLeft at all.
rc=0; run_notice "$TMP/entries-allgood.json" || rc=$?
eq 0 "$rc" "notice with only blessed entries exits"
[[ ! -e "$TMP/state-n/rollback-notice.json" ]] \
    && ok "a missing triesLeft key is NOT read as 0 — no notice, and the stale one was cleared" \
    || bad "an entry with no triesLeft key was treated as exhausted: $(cat "$TMP/state-n/rollback-notice.json")"

# An entry mid-trial is not a failure either.
rc=0; run_notice "$TMP/entries-ontrial.json" || rc=$?
eq 0 "$rc" "notice with an entry still on trial exits"
[[ ! -e "$TMP/state-n/rollback-notice.json" ]] \
    && ok "triesLeft=2 is not reported as a rollback" \
    || bad "an entry with tries remaining was reported as a rollback"

# Unreadable entries must fail closed. An empty list would be
# indistinguishable from "nothing failed", which is the answer that hides a
# rollback from the user.
rc=0
APEX_BOOT_EFIVARS="$TMP/efivars-good" APEX_BOOT_STATE="$TMP/state-n" \
APEX_BOOT_BOOTCTL_JSON="$TMP/does-not-exist.json" "$HEALTH" notice >"$TMP/out-n2" 2>&1 || rc=$?
if (( rc == 1 )); then
    ok "notice fails closed with rc=1 when the boot entries cannot be read"
else
    bad "notice returned $rc for unreadable boot entries (want 1; 126 means it never ran)"
fi

# ═════════════════════════════════════════════════════════════════════════════
sec "the state files have one schema, written in one place and read in another"
# apex-boot-health WRITES these files and apexd/apex/src/boot.rs READS them.
# They are in different languages in different directories, so nothing but a
# parity check couples them; a renamed key would leave `apex boot status`
# silently reporting "no verdict recorded" on a machine that had one.
for key in verdict entry target checkedAt failures; do
    grep -q "\"$key\"" "$HEALTH" \
        && grep -q "\"$key\"" "$BOOTRS" \
        && ok "last-health.json key '$key' is written and read" \
        || bad "last-health.json key '$key' is not in both apex-boot-health and boot.rs"
done
for key in rolledBack runningEntry failedEntries noticedAt; do
    grep -q "\"$key\"" "$HEALTH" \
        && grep -q "\"$key\"" "$BOOTRS" \
        && ok "rollback-notice.json key '$key' is written and read" \
        || bad "rollback-notice.json key '$key' is not in both apex-boot-health and boot.rs"
done
# triesDone is read out of the notice by boot.rs and copied into it by the
# health script from bootctl's own field name.
grep -q 'triesDone' "$HEALTH" && grep -q 'triesDone' "$BOOTRS" \
    && ok "bootctl's triesDone field name is used consistently" \
    || bad "triesDone is not consistent between the writer and the reader"

# ═════════════════════════════════════════════════════════════════════════════
sec "the build-time scripts refuse to write a real boot path"
# AGENTS.md boot-path rules 1 and 2: a guest ESP is always an image file, never
# the host's. These refusals run before anything is created, so the assertion
# is safe on the machine executing it — which is the whole point.
B="$REPO/files/scripts/boot-v2"
for target in /boot/apex-test.img /boot/efi/apex-test.img /efi/apex-test.img /usr/apex-test.img; do
    rc=0
    "$B/apex-mkesp" --disk "$target" --uki "x:0:$B/lib.sh" >"$TMP/out-esp" 2>&1 || rc=$?
    if (( rc == 0 )); then
        bad "apex-mkesp accepted --disk $target"
    else
        grep -q 'refusing to author an ESP' "$TMP/out-esp" \
            && ok "apex-mkesp refuses --disk $target" \
            || bad "apex-mkesp failed for --disk $target but not because it refused: $(cat "$TMP/out-esp")"
    fi
    [[ -e "$target" ]] && bad "apex-mkesp created $target" || true
done
rc=0
"$B/apex-sb-keys" /boot/keys >"$TMP/out-keys" 2>&1 || rc=$?
(( rc != 0 )) && grep -q 'refusing to write key material' "$TMP/out-keys" \
    && ok "apex-sb-keys refuses an output directory under /boot" \
    || bad "apex-sb-keys did not refuse /boot: $(cat "$TMP/out-keys")"

# ═════════════════════════════════════════════════════════════════════════════
sec "apex-mkuki fails closed on the inputs a UKI cannot be guessed from"
# A synthetic bzImage: the x86 boot protocol's HdrS magic at 0x202 and a
# kernel_version pointer at 0x20e. Enough for apex-mkuki's version read, and
# hermetic — a runner has no APEX kernel and pulling apex-os-core to get one
# would be a multi-gigabyte download for a refusal test.
python3 - "$TMP/fake-vmlinuz" <<'PY'
import sys
buf = bytearray(0x2000)
buf[0x202:0x206] = b'HdrS'
ver = b'6.99.0-apex-test (fake) #1 SMP\x00'
off = 0x1000
buf[off:off + len(ver)] = ver
buf[0x20e:0x210] = (off - 0x200).to_bytes(2, 'little')
open(sys.argv[1], 'wb').write(bytes(buf))
PY
printf 'not-a-real-initramfs' | gzip > "$TMP/fake-initrd.img"

mkuki_fails() {  # mkuki_fails DESCRIPTION EXPECTED_SUBSTRING ARGS...
    local what="$1" expect="$2"; shift 2
    local rc=0
    "$B/apex-mkuki" "$@" >"$TMP/out-uki" 2>&1 || rc=$?
    if (( rc == 0 )); then
        bad "apex-mkuki accepted $what"
    elif grep -qF -- "$expect" "$TMP/out-uki"; then
        ok "apex-mkuki refuses $what"
    else
        bad "apex-mkuki failed on $what for the wrong reason: $(tail -3 "$TMP/out-uki")"
    fi
}

# No microcode and no --allow-no-ucode. §22 puts microcode inside the UKI, and
# a UKI silently built without it would look identical to one that has it.
mkuki_fails "an initramfs with no microcode and no --allow-no-ucode" \
    "no microcode" \
    --output "$TMP/x.efi" --kernel "$TMP/fake-vmlinuz" --initrd "$TMP/fake-initrd.img" \
    --cmdline "root=/dev/nowhere"

# No --cmdline. A UKI's command line is signed into the image and cannot be
# edited at boot, so defaulting it would ship a guess.
mkuki_fails "a build with no --cmdline" \
    "--cmdline is required" \
    --output "$TMP/x.efi" --kernel "$TMP/fake-vmlinuz" --initrd "$TMP/fake-initrd.img" \
    --allow-no-ucode

# Two kernels under --from-root. Picking the newest silently is how a UKI gets
# paired with the wrong out-of-tree modules.
mkdir -p "$TMP/root2/usr/lib/modules/1.0-a" "$TMP/root2/usr/lib/modules/2.0-b"
cp "$TMP/fake-vmlinuz" "$TMP/root2/usr/lib/modules/1.0-a/vmlinuz"
cp "$TMP/fake-vmlinuz" "$TMP/root2/usr/lib/modules/2.0-b/vmlinuz"
mkuki_fails "a root containing two kernels" \
    "expected exactly 1 kernel" \
    --output "$TMP/x.efi" --from-root "$TMP/root2" --cmdline "root=/dev/nowhere" --allow-no-ucode

# A directory name that disagrees with the kernel's own version string.
mkdir -p "$TMP/root-mismatch/usr/lib/modules/9.9.9-wrong"
cp "$TMP/fake-vmlinuz" "$TMP/root-mismatch/usr/lib/modules/9.9.9-wrong/vmlinuz"
cp "$TMP/fake-initrd.img" "$TMP/root-mismatch/usr/lib/modules/9.9.9-wrong/initramfs.img"
cp /usr/lib/os-release "$TMP/root-mismatch/usr/lib/os-release" 2>/dev/null \
    || printf 'ID=test\nNAME=Test\nVERSION_ID=1\n' > "$TMP/root-mismatch/usr/lib/os-release"
mkuki_fails "a modules directory whose name disagrees with the kernel" \
    "kernel version mismatch" \
    --output "$TMP/x.efi" --from-root "$TMP/root-mismatch" \
    --cmdline "root=/dev/nowhere" --allow-no-ucode

# ═════════════════════════════════════════════════════════════════════════════
if (( WITH_BINARY )); then
sec "apex boot status reports the state, and does not invent the parts it cannot see"
APEX_BIN="${APEX_BIN:-$REPO/apexd/target/debug/apex}"
# Dies rather than skipping. A skipped check counts as success.
[[ -x "$APEX_BIN" ]] || { echo "FATAL: no apex binary at $APEX_BIN (build it first)" >&2; exit 1; }

# (a) A GRUB machine — the state every published APEX image is in.
G="$TMP/fx-grub"
mkdir -p "$G/proc" "$G/sys/firmware/efi/efivars" "$G/var/lib/apex/boot"
printf 'BOOT_IMAGE=/ostree/default-abc/vmlinuz ostree=/ostree/boot.1/default/abc/0 root=UUID=x\n' \
    > "$G/proc/cmdline"
printf '\x06\x00\x00\x00\x01' > "$G/sys/firmware/efi/efivars/SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c"
out="$(APEX_BOOT_ROOT="$G" "$APEX_BIN" boot status --json)"
# A dotted path into the report. Entry ids contain a literal '.' (they are
# filenames, "apex-good.efi"), so the separator is '/' and never '.' — an
# earlier version split on '.' and every entry-level assertion silently read
# `null`, which the exact-value checks caught and a truthiness check would not
# have.
j() { python3 -c 'import json,sys;d=json.load(sys.stdin)
for k in sys.argv[1].split("/"):
    d = d.get(k) if isinstance(d, dict) else None
print(json.dumps(d))' "$1" <<<"$out"; }
eq '"grub"'  "$(j bootloader)"             "GRUB fixture: bootloader"
eq 'false'   "$(j bootCounting/inEffect)"  "GRUB fixture: boot counting"
eq 'false'   "$(j bootedFromUki)"          "GRUB fixture: booted from a UKI"
eq 'true'    "$(j secureBoot/enabled)"     "GRUB fixture: Secure Boot"
eq 'null'    "$(j health)"                 "GRUB fixture: health verdict (never ran)"
eq 'null'    "$(j bootCounting/entries)"   "GRUB fixture: entries"
# Unavailable must come with a reason. A null with no reason is the shape that
# reads as "nothing failed".
[[ "$(j bootCounting/entriesUnavailable)" != null ]] \
    && ok "GRUB fixture: the entries are unavailable WITH a reason" \
    || bad "GRUB fixture: entries are null and no reason is given"

# (b) A systemd-boot machine mid-rollback. The bootctl document is the one
# `bootctl list --json` produced from an ESP a VM had actually booted four
# times, so the field shapes are real rather than invented.
S="$TMP/fx-sdboot"
mkdir -p "$S/proc" "$S/sys/firmware/efi/efivars" "$S/sys/class/tpm/tpm0" \
         "$S/run/systemd" "$S/var/lib/apex/boot"
E="$S/sys/firmware/efi/efivars"
printf 'root=UUID=x rw\n' > "$S/proc/cmdline"
printf '\x07\x00\x00\x00s\0y\0s\0t\0e\0m\0d\0-\0b\0o\0o\0t\0 \x002\x005\x008\0' \
    > "$E/LoaderInfo-$LOADER_GUID"
printf '\x07\x00\x00\x00s\0y\0s\0t\0e\0m\0d\0-\0s\0t\0u\0b\0' > "$E/StubInfo-$LOADER_GUID"
printf '\x07\x00\x00\x00\\\0E\0F\0I\0' > "$E/LoaderBootCountPath-$LOADER_GUID"
printf '\x07\x00\x00\x00a\0p\0e\0x\0-\0g\0o\0o\0d\0.\0e\0f\0i\0' \
    > "$E/LoaderEntrySelected-$LOADER_GUID"
printf '\x06\x00\x00\x00\x01' > "$E/SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c"
: > "$S/run/systemd/tpm2-pcr-signature.json"
cp "$TMP/entries-rolledback.json" "$S/bootctl-list.json"
cp "$TMP/state-good/last-health.json" "$S/var/lib/apex/boot/last-health.json"
APEX_BOOT_EFIVARS="$E" APEX_BOOT_STATE="$S/var/lib/apex/boot" \
    APEX_BOOT_BOOTCTL_JSON="$TMP/entries-rolledback.json" "$HEALTH" notice >/dev/null 2>&1

out="$(APEX_BOOT_ROOT="$S" "$APEX_BIN" boot status --json)"
eq '"systemd-boot"' "$(j bootloader)"            "sd-boot fixture: bootloader"
eq 'true'           "$(j bootCounting/inEffect)" "sd-boot fixture: boot counting"
eq 'true'           "$(j bootedFromUki)"         "sd-boot fixture: booted from a UKI"
eq 'true'           "$(j measuredBoot/tpmPresent)"   "sd-boot fixture: TPM"
eq 'true'           "$(j measuredBoot/pcrSignature)" "sd-boot fixture: signed PCR policy"
eq '"apex-good.efi"' "$(j bootCounting/selectedEntry)" "sd-boot fixture: selected entry"
eq 'null' "$(j bootCounting/entriesUnavailable)" "sd-boot fixture: no unavailability reason"
eq 2 "$(python3 -c 'import json,sys;print(len(json.load(sys.stdin)["bootCounting"]["entries"]))' <<<"$out")" \
   "sd-boot fixture: the exact number of entries"
eq 'true'  "$(j bootCounting/entries/apex-good.efi/blessed)" "sd-boot fixture: apex-good is blessed"
eq 'false' "$(j bootCounting/entries/apex-good.efi/exhausted)" "sd-boot fixture: apex-good is not exhausted"
eq 'true'  "$(j bootCounting/entries/apex-new.efi/exhausted)" "sd-boot fixture: apex-new is exhausted"
eq 'false' "$(j bootCounting/entries/apex-new.efi/blessed)"   "sd-boot fixture: apex-new is not blessed"
eq '"good"' "$(j health/verdict)" "sd-boot fixture: the health verdict is read back"
eq 'true'  "$(j rollbackNotice/rolledBack)" "sd-boot fixture: the rollback notice is surfaced"

# And the human report must actually say the words a user needs. A JSON-only
# assertion would pass with an empty text report.
text="$(APEX_BOOT_ROOT="$S" "$APEX_BIN" boot status)"
grep -q 'rolled back automatically' <<<"$text" \
    && ok "the human report announces the rollback" \
    || bad "the human report does not mention the rollback: $text"
grep -q 'OUT OF TRIES' <<<"$text" \
    && ok "the human report marks the exhausted entry" \
    || bad "the human report does not mark the exhausted entry"
textg="$(APEX_BOOT_ROOT="$G" "$APEX_BIN" boot status)"
grep -q 'GRUB is the default for every published APEX' <<<"$textg" \
    && ok "on a GRUB machine the report says that is the expected state" \
    || bad "the GRUB report reads like a fault: $textg"

# Read-only means read-only. Nothing under the fixture root may change.
before="$(find "$G" -type f -printf '%p %s\n' | sort | sha256sum)"
APEX_BOOT_ROOT="$G" "$APEX_BIN" boot status --json >/dev/null
APEX_BOOT_ROOT="$G" "$APEX_BIN" boot status >/dev/null
after="$(find "$G" -type f -printf '%p %s\n' | sort | sha256sum)"
eq "$before" "$after" "apex boot status wrote nothing"
fi

printf '\n== test-boot-v2: %d passed, %d failed ==\n' "$PASS" "$FAIL"
(( FAIL == 0 ))
