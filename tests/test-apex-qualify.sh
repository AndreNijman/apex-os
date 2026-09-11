#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  test-apex-qualify.sh — executable assertions for roadmap §33's hardware
#  qualification database.
#
#  ── What this is guarding ───────────────────────────────────────────────────
#  Two properties, and neither of them is "the command runs".
#
#  1. NOTHING IS RECORDED WITHOUT CONSENT. Not a file, not a directory, not an
#     empty skeleton. §33 says "with explicit user consent", and a database
#     that creates itself before the question is asked has already answered it.
#
#  2. A CHECK NOBODY HAS TRIED IS NOT A CHECK THAT FAILED. P0-001 recorded
#     suspend as PARTIAL because one machine had resumed forty times and the
#     other had never suspended at all. A two-valued readout turns the second
#     into a fault report about hardware nobody has exercised.
#
#  Two modes, the split test-apex-schema.sh uses:
#
#    (no argument)     Structural checks with no toolchain: that the store is
#                      registered in the migration framework, that the verdict
#                      type still has three arms, and that every check carries
#                      a sentence saying who can settle it.
#
#    --with-binary     Drives `apex qualify` against a fixture machine and a
#                      temp state tree. It DIES if the binary is absent; a
#                      skipped assertion reports as a pass.
#
#  Every run writes into its own mktemp tree and exports XDG_STATE_HOME and
#  XDG_CONFIG_HOME at it. This suite drives code that WRITES persistent state,
#  and a run that inherited the real variables would write into the developer's
#  own database.
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

CORE="$REPO/apexd/apexd-core/src/qualify.rs"
CLI="$REPO/apexd/apex/src/qualify.rs"
MIGRATE="$REPO/apexd/apexd-core/src/migrate.rs"
SCHEMARS="$REPO/apexd/apex/src/schema.rs"
for f in "$CORE" "$CLI" "$MIGRATE" "$SCHEMARS"; do
    [[ -f "$f" ]] || { echo "FATAL: missing $f" >&2; exit 1; }
done

sec "the store is inside the migration framework, not beside it"
has 'id: "qualification"' "$MIGRATE" "the store is declared in migrate::STORES"
has '"qualification"' "$SCHEMARS" "apex schema status knows where to look for it"
# The framework's own suite asserts that every declared store has a resolver.
# This is the other direction: a store declared and then quietly dropped from
# the path table would still pass that one if somebody added it to the
# exemption list, so the id is asserted here by name.
if grep -q 'crate::qualify::db_path()' "$SCHEMARS"; then
    ok "and resolves it through the same helper the reader uses"
else
    bad "schema.rs resolves the qualification path some other way"
fi

sec "consent is a decision with three answers"
for arm in Unset Granted Declined; do
    has "    $arm," "$CORE" "Consent::$arm exists"
done
# The whole point. A boolean would make "never asked" and "said no" the same
# state, and a settings page then either nags somebody who declined or never
# asks anybody.
has 'consent != Consent::Granted' "$CORE" "recording is gated on Granted specifically, not on 'not declined'"

sec "a verdict has three arms and two of them carry a sentence"
has 'pub enum Verdict' "$CORE" "the verdict type exists"
has '    Pass,' "$CORE" "Pass"
has '    Fail {' "$CORE" "Fail"
has '    NotChecked {' "$CORE" "NotChecked"
has 'reason: String' "$CORE" "and not-checked says why not"
# The mutation this is written against: somebody collapsing the readout to a
# bool because "unknown is basically false".
if grep -qE 'pub (verdict|passed|result): bool' "$CORE"; then
    bad "a boolean verdict has appeared; not-checked and failed are now the same row"
else
    ok "no boolean verdict anywhere in the store"
fi

sec "nothing about one machine reaches the record"
has 'pub struct Machine' "$CORE" "the identity type exists"
# Comments are stripped first, and deliberately: both files name these fields
# in prose to say why they are excluded, and a grep that counted those would
# fail for the documentation rather than for the code. What must not exist is
# a READ of one.
strip_comments() { sed -E 's://.*$::' "$1"; }
for f in "$CORE" "$CLI"; do
    strip_comments "$f" > "$f.stripped.$$"
    for forbidden in product_serial board_serial product_uuid; do
        if grep -qF -- "$forbidden" "$f.stripped.$$"; then
            bad "$(basename "$f") reads $forbidden outside a comment"
        else
            ok "$(basename "$f") never reads $forbidden"
        fi
    done
    rm -f "$f.stripped.$$"
done
has 'no_serial_number_can_reach_the_record' "$CORE" "and a test asserts it on a serialised document"

sec "the probe refuses to guess"
# Two rows are file reads. Everything else has to be somebody using the
# machine, and the readout says who.
has 'fn why_not_probed' "$CLI" "every unprobed row carries an instruction"
has 'probe_secure_boot' "$CLI" "Secure Boot is read from the EFI variable"
has 'bytes.get(4)' "$CLI" "and from the fifth byte, past the four attribute bytes"
has 'NotChecked' "$CLI" "a refused read is not-checked"
# The defect class this repository has swept for fourteen times. The behaviour
# is proved by running the probe against a sealed file, in the binary half
# below and in the Rust test named here; this assertion exists so that deleting
# either one is visible in a diff rather than silent.
has 'a_refused_secureboot_read_is_not_checked_rather_than_failed' "$CLI" \
    "a named test covers a refused read"
has 'an_unreadable_database_is_an_error_rather_than_an_empty_one' "$CLI" \
    "and another covers a database that could not be read"

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
DB="$XDG_STATE_HOME/apex/qualification.json"

# A whole fixture machine, so nothing below reads the runner's hardware.
FIX="$TMP/machine"
mkdir -p "$FIX/proc" "$FIX/sys/class/dmi/id" \
         "$FIX/sys/bus/pci/devices/0000:64:00.0" \
         "$FIX/sys/firmware/efi/efivars"
printf 'LENOVO\n'             > "$FIX/sys/class/dmi/id/sys_vendor"
printf 'ThinkPad L16 Gen 2\n' > "$FIX/sys/class/dmi/id/product_family"
printf '21SCCTO1WW\n'         > "$FIX/sys/class/dmi/id/product_name"
printf 'ThinkPad L16 Gen 2\n' > "$FIX/sys/class/dmi/id/product_version"
printf '10\n'                 > "$FIX/sys/class/dmi/id/chassis_type"
printf 'R2UET31W (1.31 )\n'   > "$FIX/sys/class/dmi/id/bios_version"
# A serial the probe must never carry into the record, in the place a
# root-run probe would find it.
printf 'PF0ABCDE\n'           > "$FIX/sys/class/dmi/id/product_serial"
mkdir -p "$FIX/proc/sys/kernel"
printf '7.2.3-cachyos2.fc43.x86_64\n' > "$FIX/proc/sys/kernel/osrelease"
printf 'BOOT_IMAGE=(hd0,gpt5)/boot/vmlinuz ostree=/ostree/boot.0/default/1d98b51dd76621b656c50e4f22dc7e5eade9b0f869443a3efa90eee08eb9373e/0\n' \
    > "$FIX/proc/cmdline"
cat > "$FIX/proc/cpuinfo" <<'EOF'
processor	: 0
vendor_id	: AuthenticAMD
model name	: AMD Ryzen 7 PRO 250 w/ Radeon 780M Graphics
physical id	: 0
core id		: 0
EOF
printf '0x030000\n' > "$FIX/sys/bus/pci/devices/0000:64:00.0/class"
printf '0x1002\n'   > "$FIX/sys/bus/pci/devices/0000:64:00.0/vendor"
printf '0x1900\n'   > "$FIX/sys/bus/pci/devices/0000:64:00.0/device"
mkdir -p "$FIX/sys/bus/pci/drivers/amdgpu"
ln -sfn ../../drivers/amdgpu "$FIX/sys/bus/pci/devices/0000:64:00.0/driver"
SBVAR="$FIX/sys/firmware/efi/efivars/SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c"
printf '\x06\x00\x00\x00\x01' > "$SBVAR"
export APEX_QUALIFY_ROOT="$FIX"

sec "before consent, there is no database"
"$APEX" qualify status > "$TMP/out" 2>&1
has 'consent: unset' "$TMP/out" "status says nobody has been asked"
has 'ThinkPad L16 Gen 2' "$TMP/out" "and still names the machine, which needs no consent"
has '0 passed, 0 failed, 12 not known' "$TMP/out" "with every row unknown"
if [[ -e "$DB" ]]; then
    bad "reading the status created $DB"
else
    ok "reading the status wrote nothing"
fi

"$APEX" qualify probe > "$TMP/out" 2>&1; rc=$?
[[ "$rc" -ne 0 ]] && ok "probe refuses without consent (exit $rc)" \
                 || bad "probe ran without consent"
has 'consent is unset' "$TMP/out" "and says which state it is in"
"$APEX" qualify record sleep --pass > "$TMP/out" 2>&1; rc=$?
[[ "$rc" -ne 0 ]] && ok "record refuses without consent (exit $rc)" \
                 || bad "record ran without consent"
has 'not recording sleep' "$TMP/out" "and names the check it did not store"
if [[ -e "$DB" ]]; then
    bad "a refused record created $DB"
else
    ok "two refusals later, still no file"
fi

sec "granting consent, and the probe that follows"
"$APEX" qualify consent grant > "$TMP/out" 2>&1
has 'sent nowhere' "$TMP/out" "granting says what it does and does not do"
[[ -f "$DB" ]] && ok "the decision is persisted" || bad "no database after granting"
mode="$(stat -c '%a' "$DB")"
[[ "$mode" == "600" ]] && ok "and is readable only by its owner" \
                       || bad "the database is mode $mode"

"$APEX" qualify probe > "$TMP/out" 2>&1
has 'gpu-driver         pass' "$TMP/out" "the bound amdgpu driver is a pass"
has 'secure-boot        pass' "$TMP/out" "the SecureBoot variable's fifth byte is read as 1"
has 'sleep              not checked' "$TMP/out" "sleep is not checked rather than failed"
has 'suspend the machine and wake it' "$TMP/out" "and says who can settle it"
hasnt 'PF0ABCDE' "$DB" "the serial sitting next to the model in DMI is not in the database"

sec "a probe never overwrites what a person recorded"
"$APEX" qualify record sleep --pass > "$TMP/out" 2>&1
has 'recorded sleep' "$TMP/out" "a person records a pass"
"$APEX" qualify probe > "$TMP/out" 2>&1
"$APEX" qualify status > "$TMP/out" 2>&1
has '[y] sleep' "$TMP/out" "and a later probe leaves it alone"

sec "an unreadable EFI variable is not a firmware that fails"
chmod 0000 "$SBVAR"
if [[ -r "$SBVAR" ]]; then
    printf '  skip  this user reads a 0000 file (root or CAP_DAC_OVERRIDE)\n'
else
    "$APEX" qualify probe --json > "$TMP/out" 2>&1
    has '"verdict": "not checked"' "$TMP/out" "a refused read is not checked"
    has 'Permission denied' "$TMP/out" "and carries the reason"
    hasnt 'switched off' "$TMP/out" "and never says the firmware has it switched off"
fi
chmod 0644 "$SBVAR"

sec "the machine that is missing a driver is the one that fails"
rm -f "$FIX/sys/bus/pci/devices/0000:64:00.0/driver"
"$APEX" qualify probe > "$TMP/out" 2>&1
has 'gpu-driver         fail' "$TMP/out" "an unbound display device is a real failure"
has '0000:64:00.0' "$TMP/out" "named by slot"
ln -sfn ../../drivers/amdgpu "$FIX/sys/bus/pci/devices/0000:64:00.0/driver"

sec "export, import, and the consent that governs both"
"$APEX" qualify record audio --pass > /dev/null 2>&1
"$APEX" qualify export "$TMP/export.json" > "$TMP/out" 2>&1
has 'wrote' "$TMP/out" "export writes where it was told"
grep -q '"consent"' "$TMP/export.json" && ok "the export is a database" \
                                       || bad "the export has no consent field"

# A second machine's state tree, with no consent, is offered the export.
export XDG_STATE_HOME="$TMP/state2"
mkdir -p "$XDG_STATE_HOME"
DB2="$XDG_STATE_HOME/apex/qualification.json"
"$APEX" qualify import "$TMP/export.json" > "$TMP/out" 2>&1; rc=$?
[[ "$rc" -ne 0 ]] && ok "an import into a machine without consent is refused" \
                 || bad "an import bypassed consent"
if [[ -e "$DB2" ]]; then bad "the refused import created $DB2"; else
    ok "and wrote nothing"; fi
"$APEX" qualify consent grant > /dev/null 2>&1
"$APEX" qualify import "$TMP/export.json" > "$TMP/out" 2>&1
has 'took' "$TMP/out" "with consent, the import takes the results"
"$APEX" qualify status > "$TMP/out" 2>&1
has '[y] audio' "$TMP/out" "and they are queryable on the new machine"

sec "withdrawing consent deletes what was kept"
"$APEX" qualify consent decline > "$TMP/out" 2>&1
has 'record(s) deleted' "$TMP/out" "declining says how much it removed"
"$APEX" qualify status > "$TMP/out" 2>&1
has 'consent: declined' "$TMP/out" "the decision is remembered"
hasnt '[y] audio' "$TMP/out" "and the results are gone"
grep -q '"machines": {}' "$DB2" && ok "the file holds an empty machine list" \
                                || bad "machines survived the withdrawal: $(cat "$DB2")"

sec "a database from a newer build is refused, not overwritten"
printf '{"schema": 99, "consent": "granted", "machines": {"x": {"machine": {}, "results": {}}}}\n' > "$DB2"
before="$(cat "$DB2")"
"$APEX" qualify status > "$TMP/out" 2>&1; rc=$?
[[ "$rc" -ne 0 ]] && ok "status refuses a schema from the future (exit $rc)" \
                 || bad "status read a schema 99 file"
has 'newer' "$TMP/out" "and says so"
"$APEX" qualify consent grant > "$TMP/out" 2>&1
[[ "$(cat "$DB2")" == "$before" ]] && ok "and nothing rewrote it" \
                                   || bad "the newer file was overwritten"

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]] || exit 1
exit 0
