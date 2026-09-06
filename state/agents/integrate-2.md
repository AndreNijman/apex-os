# integrate-2
items: (integration only) P0-005/006/007 os + shell, P1-030
repo: both
worktree: /var/tmp/apex-work/int-os and /var/tmp/apex-work/int-shell
branch: roadmap/v2.2

## NEXT
apex-os p0-005: ALL 10 COMMITS CHERRY-PICKED (tip a37dc71). Cherry-pick finished,
no rebase in progress. Running verification (cargo test / shell suites).
Recovery tags: int-os-prelanding-2 (9a24d2b), int-shell-prelanding-2 (d2d1f33).

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
