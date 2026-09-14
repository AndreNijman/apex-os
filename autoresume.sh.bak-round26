#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  autoresume.sh — keep the resume report fresh, and continue the run.
#
#  The in-session cron that used to do this died with the Claude session, which
#  meant a laptop shutdown ended the program until Andre noticed. A systemd user
#  timer with Persistent=true does not: a firing missed while the machine was
#  off happens once on the next boot.
#
#  It always does the free half — regenerate the report so whoever looks next
#  needs no Claude usage to see where things stand. It only starts a Claude
#  session if the run is armed, and arming is a file:
#
#      touch  ROADMAP/state/AUTORESUME     # continue unattended
#      rm     ROADMAP/state/AUTORESUME     # stop; the report keeps updating
#
#  It disarms itself when the roadmap has nothing left to do, so it cannot run
#  forever, and it refuses to start a second session while one is still going.
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail

ROOT=/var/home/andre/Projects/apex
RM="$ROOT/ROADMAP"
STATE="$RM/state"
LOG="$STATE/autoresume.log"
LOCK="$STATE/autoresume.lock"
ARMED="$STATE/AUTORESUME"

say() { printf '%s %s\n' "$(date '+%Y-%m-%d %H:%M:%S')" "$*" >> "$LOG"; }

# The free half, always.
"$RM/resume.sh" -f > "$STATE/report.txt" 2>&1
say "report refreshed ($(wc -l < "$STATE/report.txt") lines)"

[ -e "$ARMED" ] || { say "not armed; nothing started"; exit 0; }

# Stop when there is nothing left. A timer that keeps spending after the work is
# finished is worse than one that never ran.
remaining=$(python3 - "$RM/roadmap.yaml" <<'PY'
import sys, yaml
d = yaml.safe_load(open(sys.argv[1]))
def walk(o):
    if isinstance(o, dict):
        if 'id' in o and 'status' in o: yield o
        for v in o.values(): yield from walk(v)
    elif isinstance(o, list):
        for v in o: yield from walk(v)
print(sum(1 for i in walk(d) if i.get('status') in ('todo', 'partial', 'blocked')))
PY
)
if [ "${remaining:-1}" = 0 ]; then
    rm -f "$ARMED"
    say "roadmap has nothing outstanding; disarmed"
    exit 0
fi

# ── Is someone already working? ─────────────────────────────────────────────
# The first time this timer fired it launched a second orchestrator alongside a
# live one, and two orchestrators dispatching from the same queue would fight
# over the same branches. The signal that settles it is the same one resume.sh
# uses to judge an agent alive: when did anything last get written.
#
# A subagent output file touched in the last 20 minutes means work is in flight.
# A heartbeat file touched in the last 45 minutes means an orchestrator is alive
# even with no agents running. Either one means stay out of the way.
busy=""
if [ -n "$(find /tmp/claude-*/-var-home-andre-Projects-apex/*/tasks -name '*.output' -mmin -20 2>/dev/null | head -1)" ]; then
    busy="agents are still writing output"
elif [ -r "$STATE/orchestrator.pid" ] \
     && kill -0 "$(cat "$STATE/orchestrator.pid")" 2>/dev/null \
     && tr '\0' ' ' < "/proc/$(cat "$STATE/orchestrator.pid")/cmdline" 2>/dev/null | grep -q claude; then
    busy="the orchestrator process $(cat "$STATE/orchestrator.pid") is still alive"
elif [ -n "$(find "$STATE" -maxdepth 1 -name orchestrator.heartbeat -mmin -45 2>/dev/null)" ]; then
    busy="an orchestrator heartbeat is less than 45 minutes old"
fi
if [ -n "$busy" ]; then
    say "skipped: $busy"
    exit 0
fi

# One at a time. flock rather than a pid file: a killed session releases it.
exec 9>"$LOCK"
if ! flock -n 9; then
    say "a resume session is still running; skipped"
    exit 0
fi

say "starting a resume session ($remaining items outstanding)"
cd "$ROOT" || exit 1
timeout 4h claude -p --permission-mode bypassPermissions "$(cat <<'PROMPT'
AUTONOMOUS RESUME — APEX ROADMAP v2.2. You are continuing an autonomous run Andre asked for; ask him nothing and do not wait for approval.

Read ROADMAP/state/report.txt first. It was regenerated seconds ago and holds the roadmap counts, every dispatched agent with whether it is alive and what its next action was, which branches carry work the integration branch does not have, and the ready dispatch queue. Do not re-derive any of it with git commands.

Then:
1. Land finished task branches onto roadmap/v2.2 in both repos (rebase --onto, run tests/check-no-conflict-markers.sh, push --force-with-lease).
2. Record what landed with ROADMAP/set-status.py <id> <status> --evidence "...".
3. Update ROADMAP/state/dispatch.json.
4. Dispatch from the queue's READY list, up to six concurrent agents. Give every agent the card requirement from ROADMAP/state/README.md.

Never revive a dead agent by messaging it — that replays its whole transcript and cost 10% of a usage window on 2026-09-06. Dispatch a fresh agent and hand it ROADMAP/state/agents/<slug>.md.

Constraints: never push main or open a PR until the final integration; headless only, never a window on Andre's desktop; never run `qs -p`; no polkit or keyring prompts; never pkill apex-agentd; do not interrupt gaming on katana. Longer context if you need it: ROADMAP/PROGRESS.md.
PROMPT
)" >> "$LOG" 2>&1
say "resume session ended (exit $?)"
