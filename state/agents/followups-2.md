# followups-2 — a failing test on the tip, and the documentation debt behind it

Repo: apex-os. Branch `task/followups-r19`, from `origin/roadmap/v2.2`.
Worktree `/var/tmp/apex-work/wt-followups2`. No roadmap ids: record what you
close on this card, not with `set-status.py`.

## 1. `tests/test-apex-secret-broker.sh` fails on the integration tip

Measured by the p1-014 agent, not inferred: **64 passed / 1 failed** on the
current tip versus 63/2 on its base. The surviving failure asserts that
`'cloudflare.dns.delete' is not an operation` — which **P1-008 made false** when
it implemented all 32 of section 13.2's names. The test is stale, not the
product; confirm that before changing anything, then make the assertion say what
it is actually for. A failing test carried on the integration tip is worse than
an unfinished feature: it teaches everyone to read a red suite as normal.

## 2. 132 of 259 commands are documented nowhere

`tests/check-doc-verbs.sh` now walks binary → docs as well as docs → binary, and
`tests/doc-verbs-undocumented` records the debt as a **ratchet**: an entry that
has since been documented fails as *stale*, so the file cannot rot quietly.
Worst clusters, from that measurement:

    apex shell   14 of 16 undocumented
    apex env     11 of 13
    apex host     8 of 8
    apex ai       7 of 7

Take them cluster by cluster, writing what the command actually does — read the
source, run `--help`, do not paraphrase the verb name back at the reader — and
remove each one from the ratchet as you go. Finishing two clusters properly is
worth more than touching all four.

## 3. `apexd-core/src/host.rs:185` refuses a future `hosts.toml` with no remedy

It still says "understands up to" and stops there. Section 25 says a version
refusal must name the remedy, because a file from a newer APEX is exactly what
`bootc rollback` leaves behind — `tasks.toml` got that treatment in P1-045.
`tests/test-apex-host.sh:487` is green against the old wording, so **both move
together or not at all**.

## NEXT

Nothing done yet. Item 1 first: it is the only one that is currently red.
