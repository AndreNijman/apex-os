# integrate-2
items: (integration only) P0-005/006/007 os + shell, P1-030
repo: both
worktree: /var/tmp/apex-work/int-os and /var/tmp/apex-work/int-shell
branch: roadmap/v2.2

## NEXT
Orienting. Nothing checked out mid-rebase yet. Baseline cargo test running.

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
