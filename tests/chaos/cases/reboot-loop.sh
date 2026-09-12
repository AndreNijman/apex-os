# shellcheck shell=bash
# The CASE_* variables and the case_* functions are the contract
# tests/chaos/run-chaos reads; nothing in this file uses them itself.
# shellcheck disable=SC2034
# ─────────────────────────────────────────────────────────────────────────────
#  reboot-loop — an update that never finishes booting, across five real guest
#  boots, and the machine has to come back on the old image AND SAY SO.
#
#  P1-062 criterion 2, "boot/update/install/rollback loops run continuously in
#  VMs", and the incident the card names first:
#
#    "A staged ostree deployment is discarded by a crash before clean
#     shutdown. 'Updated but nothing changed' is that, and it is exactly
#     criterion 1's power loss during update. The machine must come back on the
#     old image *and say so*."
#
#  `tests/chaos/cases/power-loss-during-update.sh` does the userspace half of
#  that on a fixture tree in two seconds. This is the other half, and it cannot
#  be faked: the counter that decides it is decremented by the BOOTLOADER,
#  before any kernel runs, because the boots it must count are the ones that
#  never reach userspace. Nothing in userspace can count those. So this case
#  needs a bootloader, firmware and a real reboot — five of them.
#
#  ═══ STAGED, AND WHY ═══
#
#  CASE_STAGED=1: it is asked for by name and is not in a default run. It needs
#  /dev/kvm, podman, and the boot lab image built from bootlab/Containerfile.
#  On a checkout with none of those the honest answer is could-not-inject, and
#  a default run that quietly dropped it would be a green tick for zero boots —
#  the failure this repository has been bitten by repeatedly. Staged membership
#  is the same device files/scripts/boot-v2/run-scenarios uses for the same
#  reason, and run-chaos borrowed the mechanism from there.
#
#  ═══ THE FAULT ═══
#
#  The ESP is built with two entries, which is the shape of a real APEX machine
#  mid-update:
#
#    apex-new+3-0   the deployment just installed. Three boot attempts granted,
#                   none used. NOTHING blesses it, because the guest is not a
#                   systemd guest — which is exactly what a deployment that
#                   never finishes booting looks like to the bootloader.
#    apex-good      the previous deployment. A blessed entry carries no +N-M
#                   suffix at all, so it stays eligible forever.
#
#  INJECTION   a staged deployment that can never be blessed, on a real ESP,
#              booted under Secure-Boot-enforcing OVMF.
#  PROOF       the entry filenames, read back OUT OF THE DISK IMAGE with mdir.
#              The lab builds the ESP and boots it in one invocation, so the
#              pre-boot state is gone by the time the harness can look — which
#              means the proof has to be a shape only a staged-and-never-blessed
#              deployment can produce, and it is: a blessed entry with NO suffix
#              beside a counted one exhausted at +0-3. A machine where nothing
#              was staged has one entry; a machine whose staged deployment
#              booted has a blessed one with no suffix. Neither makes this pair.
#              `run-scenarios` exiting 0 is not the proof and is not read.
#  EXPOSURE    guests really booted. Five serial logs must be non-empty and
#              each must carry the bootlab marker, because a harness that
#              silently ran no guest also exits 0, and this whole unit exists
#              to refuse exactly that.
#  SURVIVAL    four things, and the last is the one the card is really asking
#              for.
#              1. The tally walks down by exactly one per boot: +2-1, +1-2,
#                 +0-3. Not "decreases" — the exact filename, because `-ge`
#                 once let 20 of 68 dropped bindings pass a test called "it
#                 generates every binding it can".
#              2. When the tally is exhausted the loader selects the OTHER
#                 entry: the machine comes back on the old image.
#              3. It STAYS there. A machine alternating between a broken
#                 deployment and a good one every other boot is worse than one
#                 that stayed broken.
#              4. The exhausted entry is STILL IN THE ESP, still carrying
#                 +0-3. That is the "and say so": the reason the machine rolled
#                 back is legible after the fact, to `apex boot status` and to
#                 a person with a torch. An entry the loader deleted on failure
#                 would leave a machine that silently went back in time.
#
#  ═══ WHAT THIS ADDS OVER run-scenarios boot-counting ═══
#
#  Stated plainly, because a case that only re-runs another harness is a case
#  that should not exist. `run-scenarios boot-counting` makes the same boots and
#  asserts (1), (2) and (3). What this adds is the three things this unit is
#  about: a could-not-inject verdict when the lab is absent, instead of a skip;
#  an EXPOSURE proof that guests actually booted rather than that a script
#  exited 0; and assertion (4), the legibility of the final state, which
#  `boot-counting` reads past on its way to the filenames.
#
#  ═══ THE BOOT PATH ═══
#
#  Nothing here touches this machine's boot path, and the containment is not a
#  promise, it is the shape of the thing: every write goes into a scratch
#  directory bind-mounted into a container, the ESP is a file, the firmware
#  varstore is a file, and the guest is an unprivileged qemu process with
#  `-display none -nodefaults -no-reboot`. /dev/kvm is a device pass-through,
#  not a privilege. tests/test-apex-chaos.sh asserts that no file under
#  tests/chaos/ runs bootctl, efibootmgr, ostree admin or rpm-ostree, and this
#  file is held to it like the rest.
# ─────────────────────────────────────────────────────────────────────────────

CASE_TITLE="an update that never boots, five real guest boots, and the fallback that must be legible"
CASE_CRITERION="2 (boot/rollback loop in a VM), 1 (power loss during update), 3 (diagnostics)"
CASE_NEEDS="kvm podman bootlab"
CASE_STAGED=1

LAB_OUT=""

# _esp_entries — the /EFI/Linux filenames, read out of the FAT by the lab's own
# mtools. This is the observation everything here rests on and it is made from
# OUTSIDE the guest, which is what makes it independent of anything the guest
# did or did not do.
#
# The ESP is carved out of the disk image here rather than read from the
# `.after` file `run-scenarios` happens to leave behind. Depending on another
# harness's intermediate file is how a case starts passing because a file
# exists rather than because a machine behaved: if boot-v2 renamed it, this
# would report "the ESP could not be read" and the reason would be nothing to
# do with APEX. The offset is the one esp_disk_readback uses — the partition
# starts at 1 MiB and runs to two short of the end.
_esp_entries() {
    podman run --rm -v "$LAB_OUT:/lab:z" "$CHAOS_BOOTLAB_IMAGE" -c '
        set -euo pipefail
        d=/lab/disk-count.img
        [ -f "$d" ] || exit 1
        mib=$(( ($(stat -c %s "$d") / 1048576) - 2 ))
        dd if="$d" of=/tmp/esp.img bs=1M skip=1 count="$mib" status=none
        mdir -i /tmp/esp.img -b ::/EFI/Linux
    ' 2>/dev/null \
        | sed 's|^::/EFI/Linux/||' | tr -d '\r' | grep -i '\.efi$' | sort | tr '\n' ' '
}

case_setup() {
    LAB_OUT="$CASE_ROOT/lab"
    mkdir -p "$LAB_OUT"
    echo "boot lab work directory $LAB_OUT, image $CHAOS_BOOTLAB_IMAGE"
}

# No baseline. The control here is INSIDE the fault: `apex-good` is the entry
# with no tally, and every assertion about the broken deployment is paired with
# one about the good one in the same ESP. A separate "healthy" run would boot a
# different disk and prove nothing about this one.
#
# Said rather than left out, because a missing case_baseline is otherwise
# indistinguishable from a forgotten one.

case_inject() {
    # The ESP with the two entries is built by the lab, and building it IS the
    # injection: a staged deployment with three attempts and no blessing.
    # run-scenarios' boot-counting scenario does this and then boots it, so the
    # injection and the subject are the same invocation — which is why the
    # proof below reads the disk image rather than this function's exit code.
    podman run --rm --device /dev/kvm \
        -v "$CHAOS_REPO:/work:z" -v "$LAB_OUT:/lab:z" \
        "$CHAOS_BOOTLAB_IMAGE" -c \
        '/work/files/scripts/boot-v2/run-scenarios --work /lab boot-counting'
}

case_prove() {
    # Independent of the injector in the way that matters: the tally in the FAT,
    # after the fact. What this can no longer prove is the state BEFORE the
    # boots — the same invocation made the ESP and consumed it — so it proves
    # the shape that only a staged-and-never-blessed deployment produces: the
    # blessed entry with no suffix, and the counted one exhausted at +0-3.
    #
    # A machine where nothing was staged has one entry. A machine whose staged
    # deployment booted has a blessed one with no suffix. Neither can produce
    # this pair, so this pair is the fault.
    local entries
    entries="$(_esp_entries)"
    echo "ESP /EFI/Linux after the run: ${entries:-<empty>}"
    if [[ -z "$entries" ]]; then
        echo "the ESP could not be read, so nothing shows a deployment was staged"
        return 1
    fi
    if [[ "$entries" != *"apex-new+"* ]]; then
        echo "no counted entry in the ESP: nothing was staged with a boot tally"
        return 1
    fi
    if [[ "$entries" != *"apex-good.efi"* ]]; then
        echo "no blessed entry in the ESP: there was nothing to fall back to, so the case tests nothing"
        return 1
    fi
    return 0
}

case_observe() {
    # The diagnostic a person reads: what each boot selected, in order, out of
    # the serial logs the guests themselves wrote.
    local log entry
    printf 'boot   selected      serial log\n'
    for log in count-0 count-1 count-2 count-fallback count-fallback2; do
        entry="$(grep -ao 'apex\.bootlab\.entry=[a-z]*' "$LAB_OUT/serial-$log.log" 2>/dev/null \
                 | head -1 | cut -d= -f2)"
        printf '%-22s %-13s %s bytes\n' "$log" "${entry:-<none>}" \
            "$(wc -c < "$LAB_OUT/serial-$log.log" 2>/dev/null || echo 0)"
    done
    printf '\nfinal ESP: %s\n' "$(_esp_entries)"
    printf '\nwhich entry the loader recorded as selected:\n'
    grep -ah 'LoaderEntrySelected=' "$LAB_OUT"/serial-count-*.log 2>/dev/null \
        | sed 's/^/  /' | sort -u
}

# ── exposure: guests really booted ──────────────────────────────────────────
#
# The single most important function in this file. `run-scenarios` exiting 0
# having booted nothing would produce a clean bill of health for a machine
# nobody looked at, and that is the defect this whole unit is built to refuse.
# Every one of the five guests has to have left a serial log with the lab's own
# marker in it — written by the guest, from inside the VM.
case_prove_exposed() {
    local log missing=0 total=0
    for log in count-0 count-1 count-2 count-fallback count-fallback2; do
        total=$((total + 1))
        if [[ ! -s "$LAB_OUT/serial-$log.log" ]]; then
            echo "serial-$log.log is missing or empty: that guest never ran"
            missing=$((missing + 1))
        elif ! grep -aq 'apex\.bootlab\.entry=' "$LAB_OUT/serial-$log.log"; then
            echo "serial-$log.log carries no bootlab marker: that guest produced no evidence"
            missing=$((missing + 1))
        fi
    done
    if (( missing )); then
        echo "$missing of $total guests left no evidence they booted"
        return 1
    fi
    echo "all $total guests booted and wrote a marker to their serial port"
    return 0
}

# entry_for LOG — which entry a boot selected, from the guest's own serial log.
_entry_for() {
    grep -ao 'apex\.bootlab\.entry=[a-z]*' "$LAB_OUT/serial-$1.log" 2>/dev/null \
        | head -1 | cut -d= -f2
}

case_judge() {
    # 1 ── the tally walked down, and the machine kept trying the new
    #      deployment while it had attempts left. Three boots, each on the
    #      staged entry.
    local i
    for i in 0 1 2; do
        local got; got="$(_entry_for "count-$i")"
        if [[ "$got" == "new" ]]; then
            _chaos_pass "boot $((i + 1)) still tried the staged deployment"
        else
            _chaos_fail "boot $((i + 1)) selected '${got:-<nothing>}', not the staged deployment"
        fi
    done

    # 2 ── the tally exhausted, the machine came back on the old image.
    local fb; fb="$(_entry_for count-fallback)"
    if [[ "$fb" == "good" ]]; then
        _chaos_pass "with the attempts used up the machine came back on the previous deployment"
    else
        _chaos_fail "the machine did not fall back: boot 4 selected '${fb:-<nothing>}'"
    fi
    # …and the loader said so in an EFI variable, which is what `apex boot
    # status` reads. A fallback the firmware performed and did not record is a
    # machine that cannot tell you why it is running what it is running.
    expect_says "…and the loader recorded which entry it chose" \
        "LoaderEntrySelected=apex-good.efi" \
        "$(cat "$LAB_OUT/serial-count-fallback.log" 2>/dev/null)"

    # 3 ── and it STAYED there. A machine that alternates is worse than one
    #      that stays broken, because the fault only shows up half the time.
    local fb2; fb2="$(_entry_for count-fallback2)"
    if [[ "$fb2" == "good" ]]; then
        _chaos_pass "the fallback is stable — the next boot did not go back to the broken one"
    else
        _chaos_fail "the machine alternated: boot 5 selected '${fb2:-<nothing>}'"
    fi

    # 4 ── AND SAY SO. The exhausted entry is still in the ESP, still carrying
    #      its +0-3, so what happened is legible after the fact. An entry the
    #      loader deleted on failure would leave a machine that silently went
    #      back in time — which is "updated but nothing changed", the incident
    #      this case is named for, with no way left to find out why.
    local entries; entries="$(_esp_entries)"
    if [[ "$entries" == "apex-good.efi apex-new+0-3.efi " ]]; then
        _chaos_pass "the exhausted deployment is still in the ESP at +0-3, so the rollback is legible"
    else
        _chaos_fail "the ESP reads '$entries', wanted 'apex-good.efi apex-new+0-3.efi '"
    fi

    # 5 ── the blessed entry never grew a tally. If it had, the machine would
    #      be three bad boots away from having nowhere to go.
    if [[ "$entries" == *"apex-good.efi"* && "$entries" != *"apex-good+"* ]]; then
        _chaos_pass "the known-good deployment is still unconditional — it can never be evicted by a counter"
    else
        _chaos_fail "the known-good deployment grew a boot counter: $entries"
    fi
}
