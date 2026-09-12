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
**Round 2 complete. Every item on this card is closed.** Round 1 was killed by
a usage limit after landing items 3 and 4; round 2 found 1 and 2 had been
closed by other agents in the meantime, and did 3's missing half and all of 5.

Landed this round: apex-os `2e9a0b4f` (one comment, no code).
Branch `task/followups-3` pushed in both repos, merged up to
apex-os `4b1e797f` / apex-shell `111ed75`.

### 1 — `apex remote` documented   DONE (by `f9dff3b7`, verified here)
Forward pass on `docs/remote.md`: **8 valid, 0 not-a-command**. Reverse pass
over the canonical docs: **166 documented, 121 declared undocumented, 0
undocumented and undeclared, 0 stale**. No `remote` line in
`tests/doc-verbs-undocumented`. The two together are airtight rather than
suggestive: an undocumented `apex remote` verb would have to appear in the
debt file or in the undeclared count, and it is in neither.

### 2 — `tests/test-apex-task.sh`   DONE (by `5a1a5390`, verified here)
**71 passed, 0 failed** at this tip. The question the card asked — which side
was wrong — was answered the right way: the PRODUCT was correct (P1-045 made
the refusal name the `bootc rollback` remedy) and the shell assertion had been
stale for three days. It now asserts the same four parts the Rust test does,
and matches `reads version [0-9]+` rather than pinning the number, which is
what made it go stale the first time.

### 3 — the provenance flake   CLOSED, and now measured
Round 1 landed the fix; nothing had shown the fix was what fixed it. The guard
(`wait_until_executable`) and the reporting fix landed in the same commit
(`9ef8a555`), so "the panic now names ETXTBSY" had never been separated from
"the retry stops it happening".

Measured by deleting the call and running both builds through one harness:

      unguarded   40 failures / 1000 runs   (4.0%)
      guarded      0 failures / 1200 runs   (200 + 1000)

Every failure was `provenance::tests::a_git_status_that_could_not_run_is_not_a
_clean_working_tree` and no other test, each panicking `git could not be run:
Text file busy (os error 26)`. At 4% a clean 1000 has probability 1.6e-18.

Two things worth keeping: the guarded arm ran alongside four concurrent cargo
workspace builds and the unguarded arm did not, so the comparison is biased
AGAINST the guard and still came back clean. And the rate tracks machine load
— 1/100 idle, 4/100 loaded — which is why the original 2-in-60 reading
understates it. Source restored byte-identical with plain `cp` (sha256 checked,
`git status` clean). `2e9a0b4f` writes the measurement into the helper's doc
comment so nobody simplifies a 200-iteration retry loop away.

### 5a — per-commit build, the four p1-020os commits   DONE
Re-run first-hand in a fresh `CARGO_TARGET_DIR`, each commit in its own
detached worktree, `cargo build --locked --workspace --all-targets`:

      bf9a5fb rc=0    e77695a rc=0    5685418 rc=0    4ab5f75 rc=0

`roadmap/v2.2` is bisectable across that landing. (An earlier run at 18:44
today got the same four; this is an independent confirmation.)

### 5b — squash-merged branch cleanup   DONE, 7 remote refs deleted
**The method integrate-4 specified cannot work, and that is a finding.**
`git merge-tree --write-tree` never equals the tip's tree for any of these:
all but one exit 1 with a CONFLICT in `.github/workflows/ci.yml`, a file every
branch appends to, and the "extra content" in the resulting tree is conflict
markers. Diffing against that tree reads like real stranded work and is not.

Used the exact content test `ROADMAP/state/unlanded.py::squashed_at` uses
instead — find a commit on the integration branch where every file the branch
touched is byte-identical to the branch tip. Deleted (pre-deletion sha kept, so
any of these is recoverable by sha from a local clone):

      apex-os     fix/pkg-multilib         a887c5fd   whole at bf2c1236
                  fix/pkg-multilib-2       aa7bf627   whole at 5b51ebc8
                  task/p1-018-mcp-auth     16fc8aeb   see below
      apex-shell  fix/labwc-desktop-parity            4ba1d51  whole at f609a6c
                  fix/popup-first-open-and-media-keys 9af43d1  whole at b8a63a8
                  fix/screenshot-off-hyprland         f3a28a6  whole at 4e267eb
                  task/p0-016-agent-settings          2685d81  whole at 4335e42

`task/p1-050-remote-protocol` had no remote ref — already gone, nothing to do.

`task/p1-018-mcp-auth` was the one that failed the exact test, and it took the
most work to settle. All ten of its commits are on the tip by subject, each as
a different sha (a cherry-pick landing). The one commit `git cherry` calls
pending, `34016f8b`, landed as `05e2db2c`; the three files it created whole
(`confine.rs`, `sidecar.rs`, `mcp_sidecar_live.rs`) are byte-identical on the
tip. 168 of its added lines are genuinely absent — and they are absent because
the tip SUPERSEDED the design: `service.rs:1445` says in so many words that
"P1-018 gated `--everywhere` on `OperationSpec::names_nothing`", which later
work found necessary but not sufficient and replaced with `same_everywhere`.
The missing lines are that older wording, the test renamed to
`the_everywhere_gate_reads_the_operations_own_declaration`, and one private
helper (`fn collect`) refactored away. Nothing is stranded.

Local branches and worktrees for all seven still exist on this machine; only
the remote refs were deleted. `CLAUDE.md`'s 2026-09-06 note still says
`fix/pkg-multilib` and `fix/pkg-multilib-2` "are safe to delete" — they are
now deleted, and that line is stale but harmless.

### Not verified
- None of this was run on GitHub CI. The flake numbers are this ThinkPad's
  (16 cores); a CI runner forks differently and the rate there is unknown.
- The seven deleted branches were verified by CONTENT against the integration
  tip. Nobody rebuilt or re-tested them.
- `apex remote`'s documented verbs were checked for PARSING, not for whether
  what `docs/remote.md` says about them is true.
