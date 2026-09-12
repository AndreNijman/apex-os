# `ROADMAP/state/` — the resume system

## The problem this exists for

On 2026-09-06 a session usage limit killed four agents at 19:53. Nothing was
lost from disk — the snapshot timer had every worktree — but bringing the agents
back cost about **10% of a usage window**, because the only way to resume an
agent was `SendMessage` to its task-id, and that replays the agent's entire
transcript through the model. Four transcripts, 2.1–2.4 MB each.

That is the wrong shape. Recovering from an interruption should cost roughly
nothing, and it should not care what kind of interruption it was — a usage
limit, a killed terminal, or the laptop losing power mid-build.

## The shape that replaces it

**State lives on disk in files a human or a script can read, not in transcripts
a model has to replay.** Three files carry everything:

| file | written by | answers |
|---|---|---|
| `dispatch.json` | the orchestrator, at every dispatch and every landing | which agents exist, what they own, where their work is |
| `agents/<slug>.md` | each agent, continuously | what it has finished, what it is doing right now, the exact next action |
| `queue.json` | the orchestrator | what to dispatch next, in what order, and what each unit conflicts with |

`../resume.sh` reads all three plus git and prints one compact report. **That
report is the resume.** A resuming orchestrator runs one command instead of
fifteen git sweeps, and dispatches a *fresh* agent carrying the dead one's card
instead of replaying its transcript. A card is about 2 KB. A transcript is
about 2.4 MB. That is the whole saving.

## The contract every agent is held to

Write your card **as you go, never at the end** — the whole point is that it is
already correct when you are killed without warning. In practice: update it
after each commit, and whenever the next action changes.

```markdown
# <slug>
items: P1-045, P1-046, P1-047
repo: apex-os
worktree: /var/tmp/apex-work/wt-p1-045
branch: task/p1-045-updates-trust

## NEXT
<one line: the exact next action, specific enough that a stranger could do it>

## DONE
- <what is finished and pushed, one line each>

## IN PROGRESS
- <what is half-written, and where — file and function>

## FOUND
- <defects and surprises the roadmap did not know about>

## BLOCKED ON
- <nothing, or the exact thing>
```

`NEXT` is the load-bearing field. Everything else can be re-derived from git;
the next action cannot.

## Durability

- `apex-wip-snapshot.timer` (every 3 min, `Persistent=true`, user lingering)
  force-pushes every dirty worktree to `refs/wip/<worktree>` on origin, **and**
  this whole `ROADMAP/` directory to `refs/wip/roadmap-state` on apex-os. So the
  cards and the queue survive losing the disk, not just losing the session.
- Recover the state directory with:
  `git -C apex-os fetch origin refs/wip/roadmap-state && git -C apex-os archive FETCH_HEAD | tar -x -C <dest>`
- `Persistent=true` means a timer missed while the machine was off fires once on
  the next boot, so a laptop that shuts down mid-work snapshots as soon as it
  comes back up.

## What a resume costs now

One `resume.sh` run and one dispatch per dead agent. No transcript replay.

## `settled.json` — so the same false positive is not chased twice

`unlanded.py`'s line-presence heuristic cannot tell *"landed in August, then
four lines rewritten in September for a better reason"* from orphaned work, and
its own docstring says so. The consequence is that a commit somebody has already
settled by hand gets re-flagged every time the report regenerates — **every five
hours, forever** — and the next orchestrator pays the same `git diff` to reach
the same answer. On 2026-09-13 two such commits were sitting in the report; one
of them had been settled in a previous round and written down nowhere a script
could read.

So a settled commit goes in `settled.json`, keyed by abbreviated sha, and the
report prints it in its own section **with its reason** instead of in the
unlanded list:

```json
{ "c8239748": "SETTLED 2026-09-13 (round 23). The prescribed diff across the one file it touches is EMPTY. Content-landed; nothing to do." }
```

Three properties it was built to have, each verified in both directions:

- **The reason is mandatory in practice.** An entry with no reason is
  indistinguishable from a commit somebody wanted to stop seeing, which is the
  failure mode this whole directory exists to prevent.
- **A malformed file refuses rather than degrading.** A status page that quietly
  drops its suppression list is how real unlanded work goes unreported —
  the same reasoning that makes `unlanded.py` refuse a missing repo argument.
- **Stale entries are reported as safe to delete.** When the heuristic stops
  flagging a commit — it landed, it was dropped, the branch is gone — the report
  says so instead of letting the file accumulate suppressions nobody can audit.

Add an entry only after actually running the command the report prescribes:
`git diff <integration> <branch> -- $(git show --name-only --format= <commit>)`.
