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
