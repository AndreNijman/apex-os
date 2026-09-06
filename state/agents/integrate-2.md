# integrate-2
items: (integration only) P0-005/006/007 os + shell, P1-030
repo: both
worktree: /var/tmp/apex-work/int-os and /var/tmp/apex-work/int-shell
branch: roadmap/v2.2

## NEXT
apex-shell DONE AND PUSHED: roadmap/v2.2 = 0fd12ee. apex-os p0-005 DONE AND
PUSHED: 5ae4350. REMAINING: cherry-pick task/p1-030-shell-integrations
(6 commits, 63493c7) onto apex-os roadmap/v2.2, then ONE clippy container run.

## PLAN
1. apex-os: cherry-pick d31257a..task/p0-005-permission-modes onto roadmap/v2.2 (9a24d2b)
2. apex-shell: land task/p0-005-agent-center-modes (1 commit) on d2d1f33
3. apex-os: cherry-pick d31257a..task/p1-030-shell-integrations

## FACTS FOUND SO FAR
- both os branches based on d31257a; roadmap/v2.2 = 9a24d2b (24 commits ahead)
- p0-005 overlap with roadmap: Containerfile.base, apex-agent-core/src/protocol.rs,
  apex-agentd/src/main.rs, apex-agentd/src/session.rs, apex/src/secret.rs,
  docs/agent-runtime.md
- p1-030 is NOT purely additive as its author reported. It also touches
  .github/workflows/pr-validation.yml, files/desktop/shell/agent.sh,
  apexd/apex/src/agent.rs (which p0-005 also rewrites) and
  apexd/apex-agent-core/src/lib.rs (also touched by p0-005). Only
  Containerfile.base of the two Containerfiles named by the author exists in
  its diff.
- protocol.rs: p0-005 bumps PROTOCOL_VERSION 4->5 and adds SYSTEM_GRANT_VERSION=5.
  roadmap is ALREADY at 5 (GENERIC_CAPABILITY_VERSION=5,
  MCP_BRIDGE_VERSION=GENERIC_CAPABILITY_VERSION). Plan: SYSTEM_GRANT_VERSION
  becomes the SAME revision 5, defined as = GENERIC_CAPABILITY_VERSION, per brief.

## APEX-OS BASELINE
cargo test --locked --no-fail-fast on 9a24d2b: 1512 passed / 0 failed.
FLAKE (pre-existing, not ours): apex-secretd `tests::a_stale_socket_from_a_dead_daemon_is_replaced`
failed on the first (fail-fast) run and passed on the --no-fail-fast rerun.

## P0-005 RESOLUTIONS (2 files conflicted, 3 hunks + 3 hunks)
- apex-agent-core/src/protocol.rs (round 3/10, commit ed9030c) -- COMPOSED.
  * PROTOCOL_VERSION: both sides 4->5. Kept ONE revision 5. The "5 -" doc entry
    now names THREE changes, one revision (P1-001 generic capabilities,
    P0-003 message body, P0-005/6/7 system-access grants), rewritten as one
    paragraph rather than two "5 -" bullets.
  * SYSTEM_GRANT_VERSION = GENERIC_CAPABILITY_VERSION (not a literal 5),
    mirroring MCP_BRIDGE_VERSION, with a doc comment saying why.
  * const asserts: kept HEAD's BROKERED < GENERIC and MCP_BRIDGE == GENERIC;
    added SYSTEM_GRANT == GENERIC. DROPPED p0-005's
    `BROKERED < SYSTEM_GRANT` and `SYSTEM_GRANT <= PROTOCOL_VERSION` -- both
    implied by what is already asserted, and `<` would be a false claim that
    they are different revisions.
  * every_version_guard_names_a_revision_that_exists: all THREE names in the
    list, all THREE assert_eq!(_, PROTOCOL_VERSION) kept.
- apex/src/secret.rs (rounds 3 and 8, ed9030c and 3dd9c09) -- HEAD side, twice.
  Same-idea-two-vocabularies: roadmap's ad1316b ALREADY did p0-005's
  "constants are checked by the compiler" refactor for this file (integrate-1
  did it). p0-005's incoming edits modify then delete a test roadmap has
  already deleted, and add const asserts roadmap already has. Only difference:
  roadmap's floor is `BROKERED > 0`, p0-005's was
  `BROKERED > REQUEST_ORIGIN_VERSION`. Kept roadmap's -- p0-005's is redundant
  with protocol.rs's own `REQUEST_ORIGIN_VERSION < BROKERED_SECRET_SERVICE_VERSION`
  compile-time assert. JUDGEMENT CALL, flagged.
  Also kept HEAD's `the_version_guard_names_the_revision_the_wire_changed_in`
  (integrate-1's deliberate keep) and `an_option_must_be_written_as_a_pair`.
- Everything else auto-merged: Containerfile.base, apexd/apex-agentd/src/main.rs,
  apexd/apex-agentd/src/session.rs, docs/agent-runtime.md.

## OUT-OF-ORDER LANDING (coordinator instruction mid-task)
8110944 `fix(secret): the one path out of a use that never scrubbed anything`
from origin/task/p1-002-cloudflare landed and pushed ALONE, ahead of the three
branches. Textually clean, but it does NOT compile against 9a24d2b: its new
test `a_provider_that_puts_the_credential_in_an_error_does_not_get_to_hand_it_over`
calls `use_capability(me(), record(..))` with two arguments, and P0-003's
landing gave that method a third, `body: Vec<u8>`. Every one of the other 27
call sites on the tip already passes `Vec::new()`. Added the same `Vec::new()`
and amended the cherry-pick so the branch builds at every commit. Pushed as
bb5b355. NOT a textual conflict; a one-token mechanical adaptation.
apex-secretd bin tests 69 -> 70. Full suite 1512 passed + the known flake.

## KNOWN PRE-EXISTING FLAKES (neither caused by this landing)
- apex-secretd `tests::a_stale_socket_from_a_dead_daemon_is_replaced` — fails
  intermittently under the full parallel run, passes 6/6 in isolation, and
  ALSO failed once on the untouched 9a24d2b baseline.
- apex-agentd `pty::tests::spawning_runs_the_real_program_on_a_real_terminal`
  and `egress::tests::a_head_that_never_ends_is_abandoned_rather_than_held_open`
  HUNG for 30+ min in one full run while two other agents' cargo runs were
  live; the same two were hung for 1h and 5h in /var/tmp/apex-work/wt-p0-005's
  own runs. Run alone: 86 passed / 0 failed in 2.01s. pty.rs and egress.rs are
  not touched by any branch here. Cross-run contention, not a regression.

## APEX-OS P0-005 VERIFIED AND PUSHED (tip 5ae4350)
- cargo test --locked --no-fail-fast: 1589 passed / 0 failed (28 binaries).
  Baseline 1512 -> +1 (the scrub-fix test) +76 (p0-005) = 1589.
- check-no-conflict-markers.sh: PASS after every round and on the tip.
- test-privilege-requests.sh  38/0
- test-agent-profile.sh       48/0
- test-apex-verbs.sh          44/0
- check-doc-verbs.sh: running it with NO ARGUMENTS is a phantom pass
  ("0 valid, 0 deliberate, 0 not a command"); it takes doc paths and an
  APEX= pointing at the build. Run properly on docs/agent-runtime.md:
  43 valid / 0 not-a-command. Over docs/*.md there are 4 BADs, all in
  historical progress notes (m0-results, m4-install-runbook, p1-progress,
  p3-progress) that this landing does not touch and that the script's own
  header says the CI wrapper excludes. Pre-existing.
- Reading pass for renames: p0-005 renames PolicyError::SystemAccessUnavailable
  -> BreakGlassCannotBeConfined and deletes the old "has no grant behind it in
  this build ... apex request" sentence. grep over tests/*.sh, Containerfile.*,
  .github/workflows/, files/ and docs/: NOTHING asserts the old variant or the
  old sentence. No test-secret-migrate-class failure hiding here.
- Brief's two orderings confirmed by inspection of the LANDED privilege.rs:
  * renew_system_grant: may_be_granted() at line 441, the grant-exists lookup
    after it; revoke_system_grant: session check before grants.revoke(id).
  * decided_by line 509: (false, Some(id)) => (Decision::AllowOnce, Some(id),
    "requested-and-covered") -- allow_once carrying system_grant, NOT
    allow_for_project.

## APEX-SHELL RESULT (tip 0fd12ee, was d2d1f33)
ZERO file overlap between task/p0-005-agent-center-modes and roadmap/v2.2 since
their common base 321e3a5, so the cherry-pick of d9cdb37 was clean. The branch
touches five files, not the two the brief named: AgentService.qml,
agentpolicy.js, SessionRow.qml, tests/agent-policy-test.js and
tests/check-agent-settings.sh.
Counts (every suite run from the REPOSITORY ROOT, XDG_* at /var/tmp/apex-shell-int2-xdg):
  check-no-conflict-markers   PASS      PASS
  settings-semantics          33/0  ->  33/0
  settings-pages              15/0  ->  15/0
  check-agent-settings        65/0  ->  81/0   (self-test 14/0 -> 17/0)
  check-color-tokens          22/0  ->  21/1 -> 22/0 after my fix below
  agent-policy-test.js        pass  ->  pass
EXPECT_WHITE_FG stayed 211. The red break-glass chip uses Theme.danger, a token,
so it added no translucent-white foreground. NOT lowered, NOT raised.

ONE COMMIT OF MY OWN (0fd12ee), flagged: check-color-tokens self-test (g)
mutated the exact line `border.color: badge.toneColor` in SessionRow.qml.
P0-005 put `(row.breakGlass && row.live) ? Theme.danger : ` in front of it, so
the literal search stopped matching, the mutant did not apply, and the harness
reported 21 passed / 1 failed -- correctly, since a mutation that does not apply
proves nothing. Every real check was green throughout. Changed the search to
`badge.toneColor` alone (first occurrence in that file IS the row border) so it
survives further edits to the front of the line. 9 mutants apply again.

Degradation against an older daemon verified by driving agentpolicy.js in node
with a session object carrying none of the new fields:
  sessionNativeLabel -> ""   isBreakGlass -> false
  sessionGrant -> null       sessionSystem -> "none"
Also "" for {} and for null. The chip is omitted, nothing throws.
Andre's own quickshell (pid 2338) untouched; ~/.config/apex-shell mtime still
6 Sep 09:01; no window opened.
