#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  test-boot-migrate.sh — executable assertions for the in-place move from
#  ostree + GRUB to composefs + systemd-boot.
#
#  What it can and cannot check, said plainly. The migration's real proof is a
#  guest with its power cut: ROADMAP/evidence/sdboot-migrate-20260921-lab.md
#  has eleven boots' worth, and nothing here replaces it. What a CI runner can
#  check is the part that is most likely to rot silently:
#
#    * the ORDER of the writes — nothing that changes what the firmware boots
#      may happen before the single BootNext commit;
#    * that every refusal exists and fires, because a precheck that stopped
#      refusing would migrate a machine that must not be migrated;
#    * that the state machine cannot skip a step or re-arm a failed migration;
#    * that the unit's conditions and ordering are what the design says.
#
#  Every assertion here is written to fail if the property is removed. The
#  suite was mutation-tested: each block below was checked to go red when the
#  line it guards is changed.
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MIG="$REPO/files/system/libexec/apex-boot-migrate"
UNIT="$REPO/files/system/units/apex-boot-migrate-confirm.service"
BASECF="$REPO/Containerfile.base"
OPS="$REPO/apexd/apex/src/ops.rs"

PASS=0 FAIL=0
ok()  { PASS=$((PASS + 1)); printf '  ok   %s\n' "$*"; }
bad() { FAIL=$((FAIL + 1)); printf '  FAIL %s\n' "$*"; }
sec() { printf '\n== %s ==\n' "$*"; }

for f in "$MIG" "$UNIT" "$BASECF" "$OPS"; do
    [[ -f "$f" ]] || { echo "FATAL: missing $f" >&2; exit 1; }
done
[[ -x "$MIG" ]] || { echo "FATAL: $MIG is not executable in the repo" >&2; exit 1; }

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

# A body without its comments. Every grep for "does the code do X" runs against
# this, because this file argues with itself at length in comments and a naive
# grep would match the argument instead of the code. (The same trap the
# Containerfile tripwire hit: a comment naming a command it refuses to run.)
CODE="$TMP/code.sh"
sed 's/[[:space:]]*#.*$//' "$MIG" > "$CODE"

run_mig() {   # run the engine with a fixture state dir; never touches this machine
    APEX_MIGRATE_STATE="$TMP/state" \
    APEX_MIGRATE_ROOT="$TMP/sysroot" \
    APEX_MIGRATE_ESP="$TMP/esp" \
    APEX_MIGRATE_DRYRUN=1 \
    APEX_MIGRATE_STORE="${STORE:-ostreeContainer}" \
        bash "$MIG" "$@" 2>&1
}

# ═════════════════════════════════════════════════════════════════════════════
sec "the commit is BootNext, and nothing before it changes what boots"
# This is the property the whole design rests on. If a stage ever writes
# BootOrder or BootNext, a power cut in the middle of the ESP write leaves a
# machine pointed at a half-written loader.
if grep -n 'efibootmgr --bootnext' "$CODE" >/dev/null; then
    ok "the commit writes BootNext"
else
    bad "nothing writes BootNext — where is the commit point?"
fi
if grep -n 'create-only' "$CODE" >/dev/null; then
    ok "the boot entry is created with --create-only (not put into BootOrder)"
else
    bad "the entry is created without --create-only, so creating it reorders the boot"
fi

# The stage function must contain no efibootmgr call at all. Extracted by
# brace-free line range: from `cmd_stage() {` to the next line that is a
# function definition at column 0.
stage_body() {
    awk '/^cmd_stage\(\) \{/{inside=1} inside{print} inside && /^\}/{exit}' "$CODE"
}
if stage_body | grep -q 'efibootmgr'; then
    bad "cmd_stage calls efibootmgr — the stage must change no boot variable"
else
    ok "cmd_stage calls efibootmgr nowhere"
fi
if stage_body | grep -qE 'bootctl[[:space:]]+(install|update)|bootupctl|grub2-install'; then
    bad "cmd_stage installs a bootloader with a tool that writes a live boot path"
else
    ok "cmd_stage installs no bootloader of its own"
fi

# And BootOrder is written only by confirm.
confirm_body() {
    awk '/^cmd_confirm\(\) \{/{inside=1} inside{print} inside && /^\}/{exit}' "$CODE"
}
if confirm_body | grep -q 'efibootmgr --bootorder'; then
    ok "BootOrder is written by cmd_confirm"
else
    bad "cmd_confirm does not write BootOrder"
fi
if grep -c 'efibootmgr --bootorder' "$CODE" | grep -qx 1; then
    ok "BootOrder is written in exactly one place"
else
    bad "BootOrder is written in more than one place"
fi

# ═════════════════════════════════════════════════════════════════════════════
sec "the install cannot reach the real /boot"
# Measured: run bare, `bootc install to-existing-root` deletes /EFI/fedora,
# overwrites /EFI/BOOT/BOOTX64.EFI and wipes the root filesystem's /boot. The
# bind over /target/boot is the only thing that stops the third one.
if grep -qE '\-v "\$stagedir:/target/boot"' "$CODE"; then
    ok "the install binds a staging filesystem over /target/boot"
else
    bad "nothing binds /target/boot — bootc's wipe would hit the real one"
fi
if grep -q 'mkfs.vfat' "$CODE"; then
    ok "the staging is a real filesystem (bootc reads a UUID off it)"
else
    bad "the staging is not a filesystem; bootc fails with 'No UUID found for /boot'"
fi
# The staging must carry the ESP's own volume id, or the migrated machine is
# told to mount a filesystem that was deleted minutes earlier. Measured: the
# guest booted and dropped to emergency mode on boot.mount.
if grep -qE 'mkfs.vfat .*-i "\$volid"' "$CODE"; then
    ok "the staging filesystem is given the ESP's volume id"
else
    bad "the staging gets a fresh volume id, which leaks into boot=UUID= on the cmdline"
fi
if grep -q 'entry-names-wrong-boot' "$CODE"; then
    ok "the staged entry's boot=UUID= is checked against the real ESP"
else
    bad "nothing checks the boot=UUID= the machine will boot with"
fi

# ═════════════════════════════════════════════════════════════════════════════
sec "the old path is preserved, not destroyed"
for want in 'BOOTX64.EFI.orig' 'grub-esp-moved' 'grub-boot-wiped'; do
    if grep -q "$want" "$CODE"; then
        ok "the stage guards: $want"
    else
        bad "missing guard: $want"
    fi
done
# GRUB is demoted, never removed: nothing may delete /EFI/fedora or the ostree
# deployment.
if grep -qE 'rm -rf .*(EFI/fedora|ostree/deploy)|ostree admin undeploy' "$CODE"; then
    bad "the engine deletes part of the old boot path"
else
    ok "nothing deletes /EFI/fedora or the ostree deployment"
fi

# ═════════════════════════════════════════════════════════════════════════════
sec "every refusal exists"
# Each of these is a machine that must NOT be migrated. A precheck that stops
# refusing is how a laptop with a 600 MiB ESP gets a migration that cannot fit.
for token in not-root not-uefi already-migrated update-staged bootc-too-old \
             secure-boot-unsigned-loader no-esp esp-too-small no-dosfstools \
             no-rsync no-podman no-repo-size root-too-small; do
    if grep -q "refuse \"$token\"" "$CODE"; then
        ok "refuses: $token"
    else
        bad "no refusal for: $token"
    fi
done
# A refusal must exit 10 and say nothing was changed: `apex update` reads that
# code to mean "carry on with the normal update", and anything else would make
# a machine that cannot migrate also fail to update.
if grep -qE '^refuse\(\).*exit 10|exit 10; \}' "$CODE"; then
    ok "a refusal exits 10"
else
    bad "a refusal does not exit 10 — apex update would treat it as a failure"
fi

# ═════════════════════════════════════════════════════════════════════════════
sec "the state machine"
rm -rf "$TMP/state"; mkdir -p "$TMP/state" "$TMP/esp" "$TMP/sysroot"

out="$(run_mig commit || true)"
if grep -q 'REFUSED \[nothing-staged\]' <<<"$out"; then
    ok "commit without a stage is refused"
else
    bad "commit without a stage was not refused: $out"
fi

echo failed > "$TMP/state/phase"
out="$(run_mig auto || true)"
if grep -q 'REFUSED \[last-attempt-failed\]' <<<"$out"; then
    ok "auto refuses to re-arm a migration whose trial boot failed"
else
    bad "auto re-arms a failed migration — every update would spend a reboot on it"
fi

out="$(run_mig retry || true)"
if [[ "$(cat "$TMP/state/phase")" == staged ]]; then
    ok "retry puts a failed migration back to staged"
else
    bad "retry did not re-arm: phase is $(cat "$TMP/state/phase")"
fi

echo confirmed > "$TMP/state/phase"
out="$(run_mig retry || true)"
if grep -q 'REFUSED \[nothing-failed\]' <<<"$out"; then
    ok "retry refuses when nothing failed"
else
    bad "retry fired on a machine with nothing to retry"
fi

echo committed > "$TMP/state/phase"
out="$(run_mig abort || true)"
if grep -q 'REFUSED \[already-committed\]' <<<"$out"; then
    ok "abort refuses past the commit point"
else
    bad "abort discarded a committed migration"
fi

# ═════════════════════════════════════════════════════════════════════════════
sec "the machine's data comes with it"
# A migration that loses /var/home is a reinstall wearing a migration's name.
if grep -q 'join_state' "$CODE"; then ok "the stage joins /var and /etc"
else bad "nothing joins the machine's state"; fi
# The rename has to happen in a private mount namespace: the ostree stateroot's
# var is a mountpoint on a running machine and a plain rename is EBUSY.
if grep -q 'unshare -m --propagation private' "$CODE"; then
    ok "/var is moved inside a private mount namespace"
else
    bad "/var is moved in the host namespace, where the rename returns EBUSY"
fi
# And the direction matters: the composefs path needs a real directory, or its
# /var comes up read-only. Measured both ways in the same guest.
if grep -qE 'mv -f "\$1" "\$2"' "$CODE"; then
    ok "the machine's var is moved INTO the composefs stateroot"
else
    bad "the var join does not move the machine's var into the stateroot"
fi
if grep -q 'ln -s "\$3" "\$1"' "$CODE"; then
    ok "a symlink is left where ostree looks, so the GRUB path still has /var"
else
    bad "nothing is left behind for the GRUB path — the fallback would boot empty"
fi

# ═════════════════════════════════════════════════════════════════════════════
sec "the confirm unit"
grep -q '^ConditionPathExists=/var/lib/apex/boot-migrate/phase$' "$UNIT" \
    && ok "inert on a machine that never started a migration" \
    || bad "the unit runs on machines with no migration in flight"
grep -q '^Wants=boot-complete.target$' "$UNIT" \
    && ok "pulls in boot-complete.target where it can be reached" \
    || bad "the unit does not pull in the health target"
if grep -q '^Requires=boot-complete.target' "$UNIT"; then
    bad "Requires= on a target a migrated machine may never reach: the confirm would never run"
else
    ok "does not Require= a target the first migrated boot may not reach"
fi
grep -q '^WantedBy=multi-user.target$' "$UNIT" \
    && ok "WantedBy, so a machine that cannot run it still boots" \
    || bad "the unit is not WantedBy=multi-user.target"
grep -q 'ExecStart=/usr/libexec/apex-boot-migrate confirm' "$UNIT" \
    && ok "runs the engine's confirm verb" \
    || bad "the unit runs something other than 'apex-boot-migrate confirm'"

# ═════════════════════════════════════════════════════════════════════════════
sec "apex update runs it, and a refusal does not stop the update"
grep -q 'fn migrate_boot_path' "$OPS" \
    && ok "ops.rs has the migration step" \
    || bad "apex update does not call the migration"
grep -q 'Ok(10) =>' "$OPS" \
    && ok "ops.rs treats exit 10 (refused) as 'carry on'" \
    || bad "a refusal is not distinguished from a failure"
# Instead of, not as well as: an ostree deployment staged in the same
# invocation would give one shutdown two finalize paths.
if grep -q 'return finish_update(started, worst, &opts);' "$OPS"; then
    ok "a machine that migrated does not also stage an image update"
else
    bad "the update carries on into bootc upgrade after migrating"
fi

# ═════════════════════════════════════════════════════════════════════════════
sec "the image ships it"
grep -q 'COPY --chmod=0755 files/system/libexec/apex-boot-migrate' "$BASECF" \
    && ok "Containerfile.base ships the engine" \
    || bad "the engine is not in the image"
grep -q 'systemctl enable apex-boot-migrate-confirm.service' "$BASECF" \
    && ok "the confirm unit is enabled at build time" \
    || bad "the confirm unit is shipped but never enabled"
for dep in podman mkfs.vfat rsync efibootmgr unshare; do
    grep -q "command -v $dep" "$BASECF" \
        && ok "the image asserts $dep is present" \
        || bad "$dep is a runtime dependency of the migration and is not asserted"
done

# ═════════════════════════════════════════════════════════════════════════════
sec "the five holes the review found, each with the assertion that would catch it"

# 1. A migrated machine must not be told on every update that it stays on GRUB.
rm -rf "$TMP/state"; mkdir -p "$TMP/state"
echo confirmed > "$TMP/state/phase"
# `set -e` would kill the suite on the non-zero exit these cases are ABOUT,
# so every one of them captures the code instead of letting it propagate.
rc=0; run_mig auto >/dev/null 2>&1 || rc=$?
if [[ "$rc" == 3 ]]; then
    ok "auto on a confirmed machine exits 3 (nothing to do), not 10 (refused)"
else
    bad "auto on a confirmed machine exits $rc — apex update would print a refusal forever"
fi
grep -q 'Ok(3) => false' "$OPS" \
    && ok "apex update treats exit 3 as silence" \
    || bad "apex update does not handle the 'nothing to do' code"

# 2. Two updates in one boot must not re-run the install.
rm -rf "$TMP/state"; mkdir -p "$TMP/state"
echo committed > "$TMP/state/phase"
cat /proc/sys/kernel/random/boot_id > "$TMP/state/committed-boot"
rc=0; out="$(run_mig auto 2>&1)" || rc=$?
if [[ "$rc" == 0 ]] && grep -q 'reboot to finish' <<<"$out"; then
    ok "committed in this boot: auto says reboot, and runs no install"
else
    bad "auto re-ran on a machine committed in this same boot (rc=$rc): $out"
fi
# Specifically: the COMMIT writes it. Checking that the file is merely
# mentioned passes while nothing creates it — the fixture writes one itself,
# so that weaker assertion survived the mutation that removed the write.
commit_body() {
    awk '/^cmd_commit\(\) \{/{inside=1} inside{print} inside && /^\}/{exit}' "$CODE"
}
if commit_body | grep -q 'committed-boot'; then
    ok "the commit records which boot it happened in"
else
    bad "nothing records the committing boot, so a second update cannot tell"
fi
# committed in an EARLIER boot, still on GRUB, is the failed case
echo committed > "$TMP/state/phase"
echo "not-this-boot" > "$TMP/state/committed-boot"
out="$(run_mig auto 2>&1)" || true
if grep -q 'REFUSED \[last-attempt-failed\]' <<<"$out"; then
    ok "committed in an earlier boot and still on GRUB is recorded as failed"
else
    bad "a trial boot that never happened is retried: $out"
fi

# 3. The saved fallback must never be overwritten by a re-run.
if grep -q 'fallback-already-systemd-boot' "$CODE"; then
    ok "refuses when the fallback is already systemd-boot with nothing saved"
else
    bad "a re-run could save systemd-boot as 'the original' fallback"
fi
if grep -qE '\[ ! -f "\$STATE/BOOTX64.EFI.orig" \]' "$CODE"; then
    ok "the fallback is saved once and never over an existing copy"
else
    bad "the fallback save is unconditional — a re-run destroys the good copy"
fi

# 4. The ESP has to hold three deployments, which is what the message says.
if grep -q 'need=$(( per \* 3 / 1024' "$CODE"; then
    ok "the ESP check sizes for three deployments"
else
    bad "the ESP check sizes for fewer deployments than an update needs"
fi

# 4b. The ROOT filesystem is checked too, and separately from the ESP — a
# migration writes a second full image copy into /composefs, and the ESP
# being big enough says nothing about that. This is item 2's whole point:
# before this, only the ESP was checked.
if grep -q 'root_need=$(( repo_kib \* 2' "$CODE"; then
    ok "the root check sizes for two image copies (the temporary + the permanent one)"
else
    bad "the root check does not size for both the temporary and permanent copy"
fi
if grep -qE 'df -Pk "\$SYSROOT"' "$CODE"; then
    ok "the root check reads free space on \$SYSROOT, not the ESP"
else
    bad "the root check does not measure \$SYSROOT — it may be checking the wrong filesystem"
fi
if grep -qE 'du -sk "\$SYSROOT/ostree/repo"' "$CODE"; then
    ok "the root check estimates size from the ostree repo, offline and without copying anything"
else
    bad "the root check has no offline size estimate — it may need to copy data to know if there is room for the copy"
fi
# The root check has to run whether or not this machine even has room for it
# to mount an ESP — a machine that is refusing no-esp should still refuse
# root-too-small first if it is ALSO too small, so a fix to one refusal is not
# mistaken for a fix to both. Order it ahead of with_esp in the source.
if awk '/^cmd_precheck\(\) \{/{p=1} p && /root_need=\$\(\(/{print "root"; exit} p && /with_esp \|\| refuse "no-esp"/{print "esp"; exit}' "$CODE" | grep -qx root; then
    ok "the root-space check runs before the ESP is even mounted"
else
    bad "the root-space check runs after with_esp — reorder so it does not depend on ESP state"
fi

# 5. A LUKS root must still find its ESP.
if grep -q 'lsblk -ndo TYPE' "$CODE"; then
    ok "root_disk walks up to a real disk (LUKS roots have a partition parent)"
else
    bad "root_disk takes one PKNAME — every encrypted machine would refuse no-esp"
fi

# and the Secure Boot state is read, not inferred from mokutil being installed
if grep -q 'SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c' "$CODE"; then
    ok "Secure Boot is read out of efivarfs"
else
    bad "Secure Boot is inferred from a tool's presence — absent tool reads as 'off'"
fi
grep -q 'secure-boot-unknown' "$CODE" \
    && ok "an unreadable SecureBoot variable is a refusal, not a guess" \
    || bad "an unreadable SecureBoot variable is treated as 'off'"

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
