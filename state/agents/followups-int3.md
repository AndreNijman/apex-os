# followups-int3
items: (hardening)
repo: apex-os
worktree: /var/tmp/apex-work/wt-followups-int3
branch: task/followups-int3
forked from: origin/roadmap/v2.2 @ b2d7905 (2026-09-07)
env: CARGO_TARGET_DIR=/var/tmp/apex-build-cache/followups-int3
     XDG_STATE_HOME=/var/tmp/apex-fi3-state XDG_CONFIG_HOME=/var/tmp/apex-fi3-config

## NEXT
Item 1: add `pub same_everywhere: bool` to `OperationSpec`
(apexd/apex-secret-core/src/operation.rs:404), state it at every construction
site (git.rs, mcp.rs, cloudflare/mod.rs, bearer.rs, operation.rs + provider.rs
test fixtures), make `ProviderSpec::validate` refuse `same_everywhere &&
!names_nothing()`, and reduce `service::may_be_granted_everywhere`
(service.rs:666) to `op.same_everywhere` — deleting the `id == "mcp.request"`
allow-list.

## DONE
- **#2 SECURITY — 0fc6a4d** `run_curl` now runs `/usr/bin/curl -q` (const
  `broker::CURL`), `-q` FIRST so `.curlrc` is disabled before it is read.
  New live test `a_curlrc_in_the_owners_home_cannot_configure_the_brokered_request`
  in broker.rs: fake owner home with a planted `.curlrc`, loopback header
  recorder, real `perform_http`. MUTATION M1 red/green: drop `-q` -> the
  recorded headers contain `X-Curlrc` -> restored, apex-secretd bin 112/0.

## IN PROGRESS
- nothing

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

## BLOCKED ON
- nothing
