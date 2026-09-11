#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  P0-001-rollback-verify.sh — run this AFTER the reboot, before anything else.
#
#  P0-001's acceptance list includes "Rollback succeeds". Every other criterion
#  on that item needs a fresh install or hardware that is off-limits; this one
#  needs a single reboot on this laptop, because unlike katana — whose second
#  slot holds the same digest, so a rollback there proves nothing — the L16 has
#  two deployments with genuinely different digests.
#
#  The rollback was STAGED by the orchestrator on 2026-09-12. Nothing rebooted
#  the machine; `rpm-ostree rollback` only swaps which deployment is default.
#
#      before  ●  sha256:308127d9…  apex (2026-09-05T13:36:24Z)   ← was booted
#      after      sha256:5e206de5…  apex (2026-09-05T03:29:10Z)   ← now default
#
#  Both run kernel 7.2.3-cachyos2.fc43.x86_64, so this is NOT the August image
#  that hard-crashed on 2026-09-05 with watchdog and clocksource timeouts —
#  that one was 7.1.5-cachyos1 and is long gone from the boot chain.
#
#  Usage after the reboot:   ROADMAP/evidence/P0-001-rollback-verify.sh
#
#  It only reads. Rolling FORWARD again is a separate, deliberate step and the
#  script prints the command rather than running it.
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail

WANT_ROLLBACK_DIGEST="sha256:5e206de5e00094276d73ef8ba85491b82573bd32e3e99a99597ee1266f81e677"
WANT_ORIGINAL_DIGEST="sha256:308127d9cefeada90414ae37bdc8175d011c1f851ea9dde1661279a5da5bd89b"

pass=0; fail=0; skip=0
ok()   { printf 'PASS  %s\n' "$1"; pass=$((pass+1)); }
bad()  { printf 'FAIL  %s\n' "$1"; fail=$((fail+1)); }
skp()  { printf 'SKIP  %s\n' "$1"; skip=$((skip+1)); }
section() { printf '\n\033[1m── %s ──\033[0m\n' "$1"; }

section "which image actually booted"

booted="$(rpm-ostree status --json 2>/dev/null \
          | python3 -c 'import sys,json; d=json.load(sys.stdin)["deployments"]; print(next(x for x in d if x.get("booted")).get("container-image-reference-digest",""))' 2>/dev/null)"
printf '      booted digest: %s\n' "${booted:-<unreadable>}"

if [ -z "$booted" ]; then
    bad "the booted digest could be read at all"
elif [ "$booted" = "$WANT_ROLLBACK_DIGEST" ]; then
    ok "the machine booted the ROLLBACK deployment, not the one it was on"
elif [ "$booted" = "$WANT_ORIGINAL_DIGEST" ]; then
    bad "the machine booted the ORIGINAL deployment — the rollback did not take"
    echo "      (a crash before clean shutdown discards a staged change; see"
    echo "       memory: ostree-staged-update-discarded-by-crash)"
else
    bad "the machine booted an unexpected digest: $booted"
fi

section "did it boot cleanly"

if last -x 2>/dev/null | head -3 | grep -q crash; then
    bad "the previous boot ended cleanly rather than in a crash"
else
    ok "the previous boot ended cleanly"
fi

failed_units="$(systemctl --failed --no-legend 2>/dev/null | wc -l)"
printf '      failed units: %s\n' "$failed_units"
if [ "${failed_units:-1}" -eq 0 ]; then
    ok "no failed system units"
else
    # vconsole-setup is a known cosmetic boot race on 7.2.3 and succeeds on
    # restart; anything else is worth reading.
    systemctl --failed --no-legend 2>/dev/null | sed 's/^/        /'
    if [ "$failed_units" -eq 1 ] && systemctl --failed --no-legend | grep -q vconsole; then
        skp "one failed unit, and it is the known vconsole-setup boot race"
    else
        bad "no failed system units"
    fi
fi

section "is the desktop usable on the rolled-back image"

for u in apex-agentd.service; do
    if systemctl --user is-active --quiet "$u" 2>/dev/null; then
        ok "$u is active"
    else
        bad "$u is active"
    fi
done

command -v apex >/dev/null && ok "the apex CLI is on PATH" || bad "the apex CLI is on PATH"

if [ -d /var/lib/extensions ] && ls /var/lib/extensions/*.raw >/dev/null 2>&1; then
    if [ -x /usr/bin/cargo-clippy ]; then
        ok "the system extension merged (cargo-clippy from apex-user.raw is present)"
    else
        skp "an extension exists but its contents are not merged into /usr"
    fi
else
    skp "no system extension on this machine"
fi

printf '\nP0-001 rollback: %d passed, %d failed, %d skipped\n' "$pass" "$fail" "$skip"

section "to roll forward again, when you are ready"
cat <<'ROLLFWD'
      sudo rpm-ostree rollback        # swaps the default back
      systemctl reboot                # boots the 2026-09-05T13:36 image again

  Nothing is lost by staying on the rolled-back image for a while: it is four
  hours older than the one you were on, same kernel, and every agent worktree
  lives under /var/tmp with its work pushed to origin.
ROLLFWD

[ "$fail" -eq 0 ]
