# followups-int3
items: (hardening)
repo: apex-os
worktree: /var/tmp/apex-work/wt-followups-int3
branch: task/followups-int3
forked from: origin/roadmap/v2.2 @ b2d7905 (2026-09-07)
env: CARGO_TARGET_DIR=/var/tmp/apex-build-cache/followups-int3
     XDG_STATE_HOME=/var/tmp/apex-fi3-state XDG_CONFIG_HOME=/var/tmp/apex-fi3-config

## NEXT
ALL FOUR ITEMS LANDED AND PUSHED; unit complete. Branch `task/followups-int3`
at **9c380b6**, base still b2d7905 (deliberately NOT rebased). Nothing is
outstanding for this agent — the next action belongs to the INTEGRATOR: merge
`task/followups-int3` into `roadmap/v2.2` (now 1ab542c). Flag for that merge:
`apex-secretd/tests/end_to_end.rs` renamed
`a_grant_held_in_every_project_is_only_for_an_operation_that_names_nothing`
-> `..._that_reaches_the_same_thing` (item 1), and `docs/agent-runtime.md`
grew by 179 insertions and 1 deletion in item 4 (the one deletion is a reworded
sentence, not removed content), so a doc conflict with another unit is textual only.

## DONE
- **#1 SECURITY — 400af1c** `same_everywhere` is now a mandatory field on
  `OperationSpec` (apex-secret-core/src/operation.rs). No `Default`, no `..`
  anywhere in the tree, so a new operation does not COMPILE until its author
  states it. `ProviderSpec::validate` refuses `same_everywhere && !names_nothing()`.
  `service::may_be_granted_everywhere` is now just `op.same_everywhere` — the
  `id == "mcp.request"` allow-list is deleted.
  The load-bearing test is `providers::tests::an_operation_that_claims_to_reach_
  the_same_thing_everywhere_binds_the_same_in_two_projects`: it binds every
  claiming operation in a project with an `apex.toml` account binding AND in a
  bare one, and requires identical `Bound`. Two EMPTY dirs would not catch it.
  MUTATIONS red/green:
    M2 cloudflare.account.read declares true -> 110/3, bind test prints
       "read account example-account [0123...]" vs "list the accounts this
       credential can see". ENDPOINT ALONE WOULD NOT CATCH IT — both are
       https://api.cloudflare.com. `detail` is what does it.
    M3 gate computes names_nothing() again -> 111/2
    M4 validate coherence rule removed -> apex-secret-core 74/1
  Behaviour unchanged: mcp.request only; cloudflare.account.read still
  per-project, so `apex cf status` still needs a per-project grant.
  RENAMED, flag for the integrator: end_to_end.rs
  `a_grant_held_in_every_project_is_only_for_an_operation_that_names_nothing`
  -> `..._that_reaches_the_same_thing`. Same test the journal calls flaky.
  Also corrected: `apex secret grant --everywhere` clap help said "Only for an
  operation that names nothing", which is the rule that was wrong.
- **#2 SECURITY — 0fc6a4d** `run_curl` now runs `/usr/bin/curl -q` (const
  `broker::CURL`), `-q` FIRST so `.curlrc` is disabled before it is read.
  New live test `a_curlrc_in_the_owners_home_cannot_configure_the_brokered_request`
  in broker.rs: fake owner home with a planted `.curlrc`, loopback header
  recorder, real `perform_http`. MUTATION M1 red/green: drop `-q` -> the
  recorded headers contain `X-Curlrc` -> restored, apex-secretd bin 112/0.

- **#3 FEATURE — 3a50741** DONE, not deferred. `run_curl` now returns
  `CurlOutput { code, stdout, stderr }` with the streams APART; the merge moved
  into `merged()`, called only by `perform_http`, so the MCP path is
  byte-identical. api.rs's 25-line child is one call to `broker::run_curl`;
  its `const CURL` and `std::process::{Command, Stdio}` are gone.
  THREE DELIBERATE BEHAVIOUR CHANGES, all in the commit message: Cloudflare
  gains `NO_PROXY=*`, Cloudflare gains the 3 MiB reply cap, MCP gains
  USER/LOGNAME. `TransportError::NoCurl`'s message changed from "could not be
  started" to "could not complete the request" because it now also carries the
  cap.
  New test `a_connection_that_never_happens_is_a_status_of_zero_and_curls_own_
  message` (port 9, discard) covers the `status == 0` path, which NOTHING
  covered before.
  MUTATION M5 red/green: put the stderr merge back inside `run_curl` -> that
  test fails "stderr was dropped" (curl's "Failed to connect" lands in stdout,
  fails the status parse, body comes back empty). Restored: secretd 114/0,
  e2e 15/0, `apex --bin apex cloudflare::` 13/0.
- **#4 DOCS — 9c380b6** `apex mcp list` / `connect` / `run` / `confine` /
  `policy` and `apex secret grant --everywhere` are documented in
  `docs/agent-runtime.md`. Three insertions: `list`+`connect` at the end of the
  existing "MCP servers, and the one line of JSON that undid the store"
  subsection; a new `### A grant held in every project`; a new
  `### One sandbox per MCP server` (§10.2, which was undocumented in ALL of
  `docs/` — grep for `mcp confine`/`sidecar` found nothing outside m0-results).
  Plus one line in *What is not built* for the `--strict-mcp-config` remainder.
  THE ASKED-FOR REASONING IS IN: why `cloudflare.account.read` does not qualify
  for `--everywhere` even though it names nothing — its `bind` reads the
  project's `apex.toml`, `GET /accounts/{id}` bound vs `GET /accounts` bare, two
  projects/two requests/one token. The doc carries the *shape* of the fix too:
  naming nothing is a fact about the declaration, where a request ends up is a
  fact about `bind`, so `same_everywhere` is a mandatory field with no `Default`
  and no `..` and a new operation does not compile until its author answers; and
  the bind test compares `detail`, not the endpoint alone, because both
  Cloudflare answers are on api.cloudflare.com.
  VERIFICATION, all three green:
    check-doc-verbs.sh  43 -> **49 valid, 0 deliberate, 0 not a command**
                        (+6: mcp list/connect/run/confine/policy, cf status)
    run-clippy.sh       **PASS clippy is clean**
    mcp_sidecar_live    **6/6** — the evidence for every sandbox claim written
  EVERY command was asked of the build before being written (11 verbs, exit 0),
  and every quoted block is REAL output: `apex mcp list`, `apex mcp policy` and
  `apex mcp confine --dry-run` run against a fixture HOME
  (/var/tmp/apex-fi3-demo-home), not hand-typed.
  Incidental correction: the prose under the `apex secret` block said "you
  rarely type the third line" while meaning `apex secret use`, the fourth.
  Inserting a line would have worsened it, so it now names the command.
- **test hardening — 55832ce** the bind-invariance test's `(Err, Err)` arm used
  to PASS. A provider whose `bind` fails in both dirs for a fixture reason would
  have bound nothing and been reported proven. Now panics.
- **RE-VERIFIED items 1-3 on the final tip 9c380b6** (the brief's suggestion for
  spare time), suites re-run by name rather than trusted from the cards:
    apex-secretd --bin apex-secretd     **114 passed / 0 failed**  (items 2+3 claimed 114/0)
    apex-secretd --test end_to_end      **15 passed / 0 failed**   (item 3 claimed 15/0)
    apex-secret-core (lib)              **75 passed / 0 failed**   (consistent with item 1's
                                        M4 mutation measuring 74/1 — one test flips red)
    apex --bin apex cloudflare::        **13 passed / 0 failed**   (item 3 claimed 13/0)
    apex --test mcp_sidecar_live        **6 passed / 0 failed**    (item 4's evidence)
  And each load-bearing test individually, 1 passed / 0 failed each:
    item 1  providers::tests::an_operation_that_claims_to_reach_the_same_thing_
            everywhere_binds_the_same_in_two_projects
    item 1  end_to_end a_grant_held_in_every_project_..._that_reaches_the_same_thing
            (the RENAMED one — it exists under the new name and passes)
    item 2  broker::tests::a_curlrc_in_the_owners_home_cannot_configure_the_
            brokered_request
    item 3  a_connection_that_never_happens_is_a_status_of_zero_and_curls_own_message
  No flake seen this time in the apex-secretd bin run (the card's known 111/1
  one-off did not reproduce across these runs).
  MEASUREMENT TRAP worth passing on: `cargo test -q -p apex-secret-core | tail -6`
  reports "running 0 tests" — that is the DOC-TEST target, the last binary to
  print. The real 75 are in the lib target above it. Truncating with `tail` can
  turn a passing suite into an apparent empty one; grep for `test result:` across
  ALL targets instead.

## IN PROGRESS
- Nothing. All four items are committed and pushed; branch PUSHED at 9c380b6
  and `git status` is clean. Item 4 was docs-only, so the workspace count is
  unchanged from item 3's measurement: 1804 passed / 1 failed (the
  environmental scheduled-job one, present at the fork point — see FOUND).

## FOUND
- **BASELINE IS NOT 1801/0 IN THIS ENVIRONMENT.** At the untouched fork point
  b2d7905 I measure **1800 passed / 1 failed**, 29 binaries. The one failure is
  `apex-agentd --test system_grants
  renewing_a_grant_that_does_not_exist_is_refused_without_asking_anybody`:
  "a scheduled-job request cannot renew a system-access grant".
  CAUSE, verified not guessed: `cat /proc/self/cgroup` here is
  `/user.slice/user-1000.slice/user@1000.service/app.slice/apex-roadmap-resume.service`
  — I am running *inside the autoresume systemd service*, and
  apex-agentd/src/origin.rs maps a systemd service cgroup to `scheduled-job`,
  which §7 refuses elevation from. integrate-3 measured 1801/0 from a login
  session. Same defect class as the journal's "a gate whose only caller cannot
  fail it": these tests ask the rule about the *running process*.
  CONSEQUENCE FOR THE ROADMAP: the resume timer's own environment cannot pass
  the workspace suite. Anything dispatched by `apex-roadmap-resume.service`
  will see this failure and must not read it as a regression.
- Pre-existing flake seen once: an apex-secretd bin run gave 111/1, the next
  two gave 112/0. integrate-3's card names
  `tests::a_stale_socket_from_a_dead_daemon_is_replaced` as the known one.
- `run_git` already sets `GIT_CONFIG_GLOBAL=/dev/null` for exactly the reason
  `run_curl` was missing `-q`. The precedent was in the same file.
- **A THIRD hardening drift between the two curls, not in the brief:** the
  broker's `run_curl` set `NO_PROXY=*` and `providers/cloudflare/api.rs` did
  not. So until 3a50741 a Cloudflare call would honour `http_proxy` if one were
  ever in the daemon's environment. Closed by the move.
- **The `status == 0` path in the Cloudflare provider had NO test.**
  `mod.rs:605` branches on it to say "the api could not be reached", and
  nothing exercised it. Added with the move.
- **NO `apex remote` FALSE ALARM, checked rather than assumed.** The brief
  warned that `task/p1-050-remote-protocol` landing on roadmap/v2.2 (1ab542c)
  might make check-doc-verbs.sh demand `apex remote …` be documented from my
  older base. It does not: `docs/agent-runtime.md` on b2d7905 mentions no
  `apex remote` verb at all, and the checker was **43 valid / 0 not a command
  BEFORE I touched the file**. Nothing to record beyond that, and no other
  unit's verbs were documented from this stale tree.
- **§10.2 (per-MCP-server confinement) was documented NOWHERE in `docs/`.**
  Not a gap item 4 was told about. `grep -rn 'mcp confine|mcp run|mcp policy|
  mcp list|mcp connect|sidecar' docs/` hit only `m0-results.md:418`, about
  SQLite `-wal` files. The whole sidecar — default-deny filesystem/network/
  secrets, `network = true` being a ceiling not a grant, per-MCP identity at
  the broker NOT existing — was discoverable only by reading `sidecar.rs`.
  It has its own subsection now.
- **The clap help was already correct and better than the docs.** Item 1 fixed
  `--everywhere`'s help text; item 4 found nothing to correct there. The doc was
  the only place still silent.
- Watch out when checking verbs by hand: **the checker is bash and word-splits
  `$c`; zsh does not.** `for c in "mcp list"; do $B $c --help; done` reports
  every verb BAD under zsh and every verb OK under `bash -c`. Cost one confused
  round trip; the script itself is fine (`#!/usr/bin/env bash`).
- Vault notes written (the vault is UP):
  `decisions/2026-09-07-same-everywhere-declared-fact.md` and
  `errors-and-fixes/autoresume-service-cgroup-reads-as-scheduled-job.md`.

## BLOCKED ON
- nothing
