#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  resume.sh — the whole program's state, in one page, for no Claude usage.
#
#  ── Why this exists ────────────────────────────────────────────────────────
#  On 2026-09-06 a usage limit killed four agents. No work was lost — the
#  snapshot timer had every worktree — but bringing them back cost about 10% of
#  a usage window, because the only resume path replayed four transcripts of
#  2.1-2.4 MB each through the model. Before that it took fifteen git commands
#  and a lot of reading just to work out what the state WAS.
#
#  Both halves of that are shell work, not model work. This is the shell work.
#  Run it, read the page, dispatch. That is the resume.
#
#  ── What it will not do ────────────────────────────────────────────────────
#  It never writes, never pushes, never fetches by default (-f to fetch), and
#  never touches a worktree. It is safe to run at any time, including while six
#  agents are mid-commit.
#
#  Usage: ROADMAP/resume.sh [-f]      -f = fetch both remotes first (slower)
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
STATE="$HERE/state"
OS=/var/home/andre/Projects/apex/apex-os
SHELLR=/var/home/andre/Projects/apex/apex-shell
INT_OS=/var/tmp/apex-work/int-os
INT_SHELL=/var/tmp/apex-work/int-shell
TASKS_GLOB='/tmp/claude-1000/-var-home-andre-Projects-apex/*/tasks'

FETCH=0
[ "${1:-}" = "-f" ] && FETCH=1

hr() { printf '─%.0s' $(seq 1 76); echo; }
sec() { echo; hr; printf '  %s\n' "$1"; hr; }

echo "APEX ROADMAP — resume report — $(date '+%Y-%m-%d %H:%M %Z')"

if [ "$FETCH" = 1 ]; then
    git -C "$INT_OS"    fetch -q origin 2>/dev/null
    git -C "$INT_SHELL" fetch -q origin 2>/dev/null
fi

# ── 1. Where the roadmap stands ─────────────────────────────────────────────
sec "roadmap"
python3 - "$HERE/roadmap.yaml" <<'PY'
import sys, yaml, collections
d = yaml.safe_load(open(sys.argv[1]))
def walk(o):
    if isinstance(o, dict):
        if 'id' in o and 'status' in o: yield o
        for v in o.values(): yield from walk(v)
    elif isinstance(o, list):
        for v in o: yield from walk(v)
items = list(walk(d))
per, tot = collections.Counter(), collections.Counter()
for i in items:
    ph = str(i['id']).split('-')[0]
    per[(ph, i.get('status', 'todo'))] += 1
    tot[i.get('status', 'todo')] += 1
order = ['done', 'partial', 'todo', 'blocked']
print(f"  {'':6}  " + "  ".join(f"{s:>8}" for s in order))
for ph in sorted({k[0] for k in per}):
    print(f"  {ph:6}  " + "  ".join(f"{per[(ph,s)] or '-':>8}" for s in order))
print(f"  {'ALL':6}  " + "  ".join(f"{tot[s] or '-':>8}" for s in order) + f"   of {len(items)}")
print()
nd = [i for i in items if str(i['id']).startswith('P0') and i.get('status') != 'done']
print("  P0 not done: " + (", ".join(f"{i['id']}({i['status'][:4]})" for i in nd) if nd else "none — P0 is complete"))
PY

# ── 2. The agents, and whether they are alive ───────────────────────────────
sec "agents"
python3 - "$STATE/dispatch.json" "$STATE/agents" "$TASKS_GLOB" <<'PY'
import sys, json, os, glob, subprocess, time
disp, carddir, tglob = sys.argv[1], sys.argv[2], sys.argv[3]
try:
    agents = json.load(open(disp))['agents']
except Exception as e:
    print(f"  dispatch.json unreadable: {e}"); raise SystemExit
now = time.time()
def sh(*a):
    try: return subprocess.run(a, capture_output=True, text=True, timeout=20).stdout.strip()
    except Exception: return ""
for a in agents:
    wt, slug = a['worktree'], a['slug']
    exists = os.path.isdir(wt)
    head  = sh('git','-C',wt,'log','--oneline','-1') if exists else ''
    dirty = len([l for l in sh('git','-C',wt,'status','--porcelain').splitlines() if l]) if exists else 0
    ahead = sh('git','-C',wt,'rev-list','--count',f"origin/{a['branch']}..HEAD") if exists else ''
    # alive: any tasks/<agent_id>.output written in the last 10 minutes
    alive, age = 'no worktree' if not exists else 'unknown', None
    for d in glob.glob(a.get('output_glob', tglob), recursive=True):
        p = os.path.join(d, a.get('agent_id','') + '.output')
        if os.path.exists(p):
            age = now - os.stat(p).st_mtime
            alive = 'ALIVE' if age < 600 else f'dead {int(age/60)}m'
            break
    card = os.path.join(carddir, slug + '.md')
    nxt, cage = '(no card)', ''
    if os.path.exists(card):
        cage = f"{int((now-os.stat(card).st_mtime)/60)}m"
        lines = open(card).read().splitlines()
        for i, l in enumerate(lines):
            if l.strip().upper().startswith('## NEXT'):
                for j in lines[i+1:]:
                    if j.strip(): nxt = j.strip(); break
                break
    print(f"  {slug:10} {alive:10} {','.join(a['items']):34} {a['branch']}")
    print(f"  {'':10} head {head[:62]}")
    print(f"  {'':10} dirty {dirty} file(s), {ahead or '?'} unpushed, card {cage or '-'}")
    print(f"  {'':10} NEXT {nxt[:66]}")
    print()
PY

# ── 3. Task branches carrying work the integration branch does not have ─────
# `merge-base --is-ancestor` is useless here: the flow rebases task branches
# onto roadmap/v2.2, so every landed branch reads as unmerged. `git cherry`
# compares patch-ids and is closer, but a commit whose conflicts were resolved
# during the rebase also reads as unlanded — that fired twice on 2026-09-06.
# The check that settles it is a two-dot diff scoped to the branch's own files.
sec "unlanded work (content-checked against roadmap/v2.2)"
if ! python3 "$STATE/unlanded.py" origin/roadmap/v2.2 "$INT_OS" "$INT_SHELL"; then
    echo
    echo "  *** THIS SECTION FAILED. Do not read its absence as 'nothing unlanded'."
    echo "  *** Fix it before dispatching anything, or you will land work twice."
fi

# ── 4. Whether the net is actually up ───────────────────────────────────────
sec "durability"
t=$(systemctl --user is-active apex-wip-snapshot.timer 2>&1)
printf '  snapshot timer: %s   ' "$t"
systemctl --user list-timers apex-wip-snapshot.timer --no-pager 2>/dev/null | sed -n 2p | awk '{print "last", $5, $6, $7}'
echo "  wip refs:"
git -C "$INT_OS"    ls-remote origin 'refs/wip/*' 2>/dev/null | sed 's|^\(.......\).*refs/wip/|    apex-os    \1  |' | head -25
git -C "$INT_SHELL" ls-remote origin 'refs/wip/*' 2>/dev/null | sed 's|^\(.......\).*refs/wip/|    apex-shell \1  |' | head -25

# ── 5. What to dispatch next ────────────────────────────────────────────────
sec "next"
python3 - "$STATE/queue.json" "$STATE/dispatch.json" "$HERE/roadmap.yaml" <<'PY'
import sys, json
q = json.load(open(sys.argv[1]))
try: agents = json.load(open(sys.argv[2]))['agents']
except Exception: agents = []
live = {a['slug'] for a in agents}
# A unit whose work is already in someone's hands must not be offered again.
# The agent slug and the queue id are allowed to differ (followups-integrate-3
# was dispatched as followups-int3), so an agent may name its unit explicitly.
taken = set(live) | {a['queue_id'] for a in agents if a.get('queue_id')}
print(f"  {len(live)} agent(s) dispatched, ceiling {q['concurrency_ceiling']}")
print()
# A unit every one of whose roadmap items is `done` has nothing left to
# dispatch. Derived from roadmap.yaml rather than maintained by hand, because a
# queue that still offers finished work is how the same task gets done twice.
status = {}
try:
    import yaml
    status = {t['id']: t.get('status') for t in yaml.safe_load(open(sys.argv[3]))['tasks']}
except Exception:
    pass

def finished(u):
    # `closed` is the hand-set escape hatch, and it exists because deriving
    # this from roadmap.yaml alone is not enough. An item stays `partial` when
    # its remaining criterion belongs to a DIFFERENT item (P1-005's per-task
    # credential is P1-011's work) or needs hardware nobody here has
    # (BASE-010 wants non-zero spendable VRAM on an APU). The unit is finished;
    # the item honestly is not. Without this, `all(status == 'done')` re-offers
    # such a unit for ever -- on 2026-09-12 that cost two dispatched agents in
    # one round, both of which opened a worktree on work already landed.
    # Set it with a reason; it is printed, not silent.
    if u.get('closed'):
        return True
    ids = [i for i in u['items'] if i in status]
    return bool(ids) and len(ids) == len(u['items']) and all(status[i] == 'done' for i in ids)

ready, held, running, done_units = [], [], [], []
for u in q['units']:
    if u['id'] in taken:
        running.append(u); continue
    if finished(u):
        done_units.append(u); continue
    blockers = [a for a in u['after'] if a in live or a == '*']
    (held if blockers else ready).append((u, blockers))
if running:
    print("  IN HAND (a dispatched agent owns this unit — do NOT dispatch it again):")
    for u in running:
        print(f"    {u['id']:14} {','.join(u['items'])[:30]:32} {u['title'][:60]}")
    print()
print("  READY (dependencies believed satisfied — confirm against the agent table above):")
for u, _ in ready[:8]:
    print(f"    {u['id']:14} {','.join(u['items'])[:30]:32} {u['title'][:60]}")
    if u.get('conflicts'): print(f"    {'':14} conflicts with {', '.join(u['conflicts'])}")
print()
print("  HELD:")
for u, b in held[:10]:
    print(f"    {u['id']:14} after {', '.join(b)[:44]}")
if done_units:
    print()
    print("  COMPLETE (nothing left to dispatch — not offered again).")
    print("  This says the WORK is done, not that it is landed — check the")
    print("  unlanded section above before assuming roadmap/v2.2 carries it:")
    auto = [u for u in done_units if not u.get('closed')]
    shut = [u for u in done_units if u.get('closed')]
    if auto:
        print("   ", ", ".join(u['id'] for u in auto))
    for u in shut:
        print(f"    {u['id']} — CLOSED BY HAND: {u['closed']}")
PY

sec "how to resume"
cat <<'EOF'
  1. This page IS the resume. Do not re-derive it with git commands.
  2. A dead agent is NOT resumed with SendMessage — that replays its whole
     transcript through the model and cost 10% of a usage window on 2026-09-06.
     Dispatch a FRESH agent and paste it ROADMAP/state/agents/<slug>.md.
     Its worktree is intact; its branch is pushed; its card says what is next.
  3. Land finished branches onto roadmap/v2.2, record them with set-status.py,
     then dispatch from READY up to the ceiling.
  4. Full context, if you need more than this page: ROADMAP/PROGRESS.md.
EOF
