# integrate-1
items: (integration only)
repo: both
worktree: /var/tmp/apex-work/int-os and /var/tmp/apex-work/int-shell
branch: roadmap/v2.2

## NEXT
apex-os is DONE and PUSHED. Start Job 2 (apex-shell): land
origin/task/p0-018-display-finish on roadmap/v2.2 in /var/tmp/apex-work/int-shell.
One conflicted file: src/services/config_tab/pages/DisplayPage.qml. Keep
P0-023's staging layer (stage/unstage/revertStaged/applyStaged) on top of
P0-018's _writeFiles/_genLua changes underneath, and no banned control label
(Discard/Undo/Forget/Restore/Commit/Write).

## APEX-OS RESULT
- roadmap/v2.2: a141cee -> ba087f4 (10 commits), pushed with --force-with-lease.
- cargo test: 1477 before -> 1512 after, 0 failed.
- cargo clippy --locked --all-targets -- -D warnings: clean (run in
  docker.io/library/rust:1.97 + `rustup component add clippy`; there is NO
  clippy installed on L16's host toolchain -- /usr/bin/cargo is Fedora's
  rustc 1.98 with no clippy component and no rustup).
- tests/check-no-conflict-markers.sh: PASS at every round and at the tip.
- No test lost from either side. Verified by name-set comparison:
  * of the 27 tests P0-003 adds, 26 are present; the 27th
    (`an_mcp_record_names_the_service_as_its_resource`) is the genuine
    contradiction, see FOUND.
  * of P1-001's 1252 test fns, all present; one renamed
    (`the_shipped_registry_builds_and_offers_the_git_vocabulary` ->
     `..._every_provider_s_vocabulary`, now asserting mcp.request too).
- FLAKY, pre-existing, not caused by this landing:
  `blueprint::tests::a_live_converger_cannot_be_built_while_the_guard_is_set`
  failed once ("\"0\" is still set") and passed on re-run -- it mutates a
  process-global env var while the rest of the `apex` bin tests run in
  parallel.

## DONE
- baseline verified on roadmap/v2.2 @ a141cee: cargo test = 1477 passed / 0 failed
- tag int-os-prelanding set on a141cee (recovery point)
- rebase started; round 1/10 (a0fad9d, profile/sandbox/session) applied clean

## ROUND 2/10 COMMITTED as 9854884 -- cargo test 1500 passed / 0 failed
(baseline 1477). All P0-003 MCP end-to-end tests pass against the ported
provider, including the loopback bearer-token server and the SSE variant.

## RESOLVED IN ROUND 2
- apex-secret-core/src/capability.rs -> roadmap (P1-001) side wholesale.
  P0-003's `Capability::McpRequest` variant, `remote() -> Option`, `resource()`,
  `is_git()` and its two enum tests all describe a type P1-001 deleted. The MCP
  security argument (no caller-supplied field, so the endpoint can only be the
  stored record's) is carried forward into the new providers/mcp.rs instead.
- apex-agent-core/src/protocol.rs -> composed. Both sides bumped 4->5 with
  different constant names; resolved as ONE revision:
  GENERIC_CAPABILITY_VERSION = 5, MCP_BRIDGE_VERSION = GENERIC_CAPABILITY_VERSION,
  ordering guard BROKERED < GENERIC kept, P0-003's `BROKERED < MCP_BRIDGE`
  replaced by `MCP_BRIDGE == GENERIC` (it would otherwise be a false claim that
  they are different revisions). SecretUse now carries
  operation/resource/params (roadmap) AND body: Option<String> (P0-003).
  Both entries kept in the version-guard test list; both assert_eq'd.

- apex-agentd/src/{broker,main}.rs -> composed, mechanical: roadmap's
  operation/resource/params parameter list PLUS P0-003's body pass-through.
- apex-secretd/src/broker.rs -> roadmap's GitOp signatures for resolve_url and
  perform (P0-003's Option<remote> handling existed only for McpRequest);
  P0-003's perform_http/run_curl/quote/TempFile/mcp_session_id kept whole.
  Dropped P0-003's unreachable `Capability::McpRequest` arm in perform().
- apex-secretd/src/service.rs -> roadmap's framework flow wholesale, PLUS
  P0-003's `body: Vec<u8>` parameter, handed to the provider through a new
  `Bind::body` field. P0-003's is_git()/else branch deleted: the registry
  routes it. P0-003's Service.mcp_sessions map MOVED into McpProvider (a map
  of MCP sessions in the framework is the coupling P1-001 removed).
- NEW apex-secretd/src/providers/mcp.rs -> P0-003's McpRequest re-expressed as
  a Provider: operation `mcp.request`, alias `mcp-request`, ResourceKind::None,
  no params, Effect::Write. bind() = Endpoint::from_url(service.url()) plus
  P0-003's two refusals (no --path, empty message). perform() calls
  broker::perform_http unchanged.
- apex-secretd/tests/end_to_end.rs -> roadmap's `git.` spellings + P0-003's
  body_len framing; P0-003's 4 MCP tests kept, retargeted at
  CapabilityRecord::new(svc, "mcp.request", "").
- apex/src/secret.rs -> roadmap's generic CLI; P0-003's SecretUse gains body.
  Both version-guard tests kept (see FOUND).
- apex/src/mcp.rs -> sends operation "mcp.request", resource "", params {}.

- ROUND 3/10 committed 73bd136 (1503 passed). broker.rs doc hunk -> incoming
  text with the `mcp.request` correction. service.rs -> framework flow kept;
  the incoming `run_dir` argument to perform_http is now McpProvider's own
  field, threaded through default_registry(run_dir) from secretd main.rs.
- ROUND 4/10 committed 13ececb (1503 passed). service.rs -> incoming
  NewService struct for add(), roadmap's longer "evil capability" list kept
  (it names the new `git.clone`/`cloudflare.dns.delete` spellings too).
  Converted the add() call sites I had written in round 2, plus bearer.rs's.
- ROUND 5/10 committed 14d4e69 (1509 passed). apex/src/secret.rs -> roadmap's
  generic "a resource is named" paragraph PLUS P0-003's gitshim paragraph.
  tests/test-secret-broker.sh -> roadmap's `git.fetch` spelling PLUS P0-003's
  four shim checks. gitshim.rs ported: capability_for now returns `git.push`
  etc. (canonical §13.2 ids) and SecretUse carries operation/resource/params.
  NOTE: its `("git-push", ...)` match arms had to move with it -- the test
  `the_three_brokered_operations_are_recognised` caught that, which is the
  only actual regression this landing produced and it is fixed.

- ROUND 6/10 committed 782e42a (1514). secret.rs -> roadmap's Use arm PLUS
  P0-003's Migrate arm. migrate.rs ported off the `Capability` enum to
  ("git.ls-remote","origin") / ("mcp.request","").
- ROUND 7/10 committed ad1316b (1512). Honoured 4839e4f: the BROKERED store
  guard became the const asserts at the top of secret.rs and its runtime test
  was deleted; the roadmap's `GENERIC_CAPABILITY_VERSION == PROTOCOL_VERSION`
  test was KEPT, because `==` is a claim about what a future edit must do,
  not just a fact about two constants.
- ROUND 8/10 committed 846fdf8. docs/agent-runtime.md -> P1-001's "Where a
  provider plugs in" section AND P0-003's "MCP servers" + "Moving what a
  machine already has" sections, all three present, respelled to `mcp.request`
  and `git.ls-remote`. "What is not built" now says two providers.
- ROUNDS 9-10 applied clean (0e066c7, ba087f4).

## IN PROGRESS
- apex-shell

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
