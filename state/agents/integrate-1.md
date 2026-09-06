# integrate-1
items: (integration only)
repo: both
worktree: /var/tmp/apex-work/int-os and /var/tmp/apex-work/int-shell
branch: roadmap/v2.2

## NEXT
apex-os rebase round 2/10 (885930b). Resolve apexd/apex-secret-core/src/capability.rs
first: take the roadmap (P1-001) side wholesale, drop P0-003's Capability::McpRequest
enum variant and its two enum tests. Then `git add` that one file only.

## DONE
- baseline verified on roadmap/v2.2 @ a141cee: cargo test = 1477 passed / 0 failed
- tag int-os-prelanding set on a141cee (recovery point)
- rebase started; round 1/10 (a0fad9d, profile/sandbox/session) applied clean

## IN PROGRESS
- round 2/10 = 885930b "the broker carries an mcp message". 8 files conflicted:
  apex-agent-core/src/protocol.rs, apex-agentd/src/{broker,main}.rs,
  apex-secret-core/src/capability.rs, apex-secretd/src/{broker,service}.rs,
  apex-secretd/tests/end_to_end.rs, apex/src/secret.rs

## FOUND
- THE HEADLINE: the task brief says the roadmap side is "P0-002's secretd
  hardening". It is not. a141cee/49089ae/6abe8ec/36134c2 are **P1-001**, which
  DELETED the `Capability` enum in favour of `operation: String` + `params` +
  a `Provider` trait + a registry. P0-003 ADDED a variant to that enum. So this
  is a PORT onto P1-001's provider framework, not a merge. Every one of the
  eight files conflicts for that one reason.
- Contradicting assertions (real, not mechanical):
  P0-003 `an_mcp_record_names_the_service_as_its_resource` asserts
  `record.resource == "claude-memory"`; P1-001 `ResourceKind::None` refuses a
  non-empty resource (OperationSpec::check). Resolving toward P1-001.
- Both sides bumped agent PROTOCOL_VERSION 4 -> 5 with different named
  constants (GENERIC_CAPABILITY_VERSION vs MCP_BRIDGE_VERSION). Planning to make
  them the same revision 5.

## BLOCKED ON
- nothing
