# followups-3 — the debt nobody owns, against the landed tip

items: (no roadmap id — these are findings carried across rounds)
repo: both
branch: task/followups-3

Every item below has been recorded for at least one round with no owner. They
are small, they are independent, and each one is a real defect or a real gap.
Work against `roadmap/v2.2` as it stands now (apex-os `4ae4cf22`+,
apex-shell `bdfb056`+). Land what you finish; do not sit on a branch.

## 1. `apex remote` is documented nowhere  (FINDINGS-round5 item 2)

`apex remote pair|devices|revoke|status|enable` appears in NO document.
`check-doc-verbs.sh` structurally cannot catch this: it validates
documented-verb -> real-command, and not the reverse. Three rounds have
recorded it and none has fixed it.

Note the reverse pass now exists and found **132 of 259 apex commands
documented nowhere** — so scope this to `apex remote` plus whatever the
reverse pass reports for it, not to all 132.

**Also unowned, same shape:** `check-doc-verbs.sh` reports seven `apex lid`
verbs that are in the binary and in no document (found by p1-061, untouched).

## 2. `tests/test-apex-task.sh` — 70 passed / 1 failed, pre-existing

Checked at `583355e`, before any of round 20's merges, and red there too. The
assertion wants `understands up to` in the refusal for a `tasks.toml` written
by a newer apex; the message was reworded and the suite was not. One line, in
either direction — decide which is right and say why.

## 3. `provenance.rs` flake — `a_git_status_that_could_not_run…`

Fails about **one run in three in parallel** and **never** with
`--test-threads=1`. Same `set_var` shape as the agentd flake that was fixed
earlier in this program — start there. It is not owned by any branch and it is
not caused by one.

## 4. `tests/lib/headless.sh` sets an output mode by calling its own stub

Found while fixing nav-geometry. The runner sets an output mode by calling the
stub it itself provides, under 13 runners, **with a comment denying that is
what it does**. Either the comment is wrong or the code is; establish which by
measurement and fix the one that is.

## 5. Two items owed from integrate-4's card

- The per-commit build check for the four `p1-020os` commits.
- Squash-merged branch cleanup by `git merge-tree --write-tree` comparison.
  The deletable list is in `dispatch.json` under *"the branches that are now
  safe to delete"*. Deletion is cheap and irreversible — verify by content
  before deleting anything, and never delete a branch an alive agent owns.

## Rules
- Mutation discipline: watch each new assertion go red, restore byte-identical
  with plain `cp` (never `mv`, never `cp -p`).
- "Permission denied is not absence." A refusal, an absence and a
  could-not-run are three different answers; `verify.rs::Verdict` is the house
  pattern.
- Never push `main`, never open a PR.
- Name your commit-message file after this unit — the session scratchpad is
  SHARED between concurrent agents and one unit's message has already landed on
  another unit's commit.

## NEXT
Round 2 (fresh agent; the first was killed by a usage limit). IN PROGRESS —
this block is rewritten at the end of the round.

Settled so far, all first-hand on this machine:
 1. **DONE, by someone else** — `f9dff3b7` wrote `docs/remote.md`. Verified:
    forward pass on that file, 8 valid / 0 not-a-command; reverse pass over the
    canonical docs, **0 undocumented and undeclared, 0 stale**, and no `remote`
    line in `tests/doc-verbs-undocumented`. Those two together are airtight:
    an undocumented `apex remote` verb would have to be in the debt file or in
    the undeclared count, and it is in neither.
 2. **DONE, by someone else** — `5a1a5390`. `tests/test-apex-task.sh` is
    **71 passed / 0 failed** at this tip. The PRODUCT was right and the shell
    assertion was three days stale; it now asserts the same four parts the Rust
    test does.
 5a. **DONE** — per-commit build check, four p1-020os commits, all rc=0.
 5b. **DONE** — seven remote refs deleted after content verification.
