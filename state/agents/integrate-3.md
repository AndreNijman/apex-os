# integrate-3
items: (integration only) chore/run-clippy, task/p1-045-updates-trust,
       task/p1-002-cloudflare (apex-os); fix/locked-hint (apex-shell)
repo: both
worktree: /var/tmp/apex-work/int-os and /var/tmp/apex-work/int-shell
branch: roadmap/v2.2

## NEXT
BRANCH 1 PUSHED: 7f14a00 (baseline reverified 1614/0, 28 bins).
BRANCH 2 PUSHED: 5cbf072. 1685/0 (+71, zero removed). ZERO CONFLICTS.
  shell suites schema 44/0, trust 25/0, channel 57/0; doc-verbs 18 valid 0 bad.
BRANCH 3 p1-002 IN PROGRESS (SKIP 8110944, already landed as bb5b355):
  9af2b08 -> 16a32e3  CLEAN (Cargo.lock auto-merged, --locked build ok)
  ddc80fa -> a66b457  2 conflicts resolved + 5 compile adaptations (see below)
  74f1055 -> 54e9779  CLEAN
  3d2dc43 -> 1322181  CLEAN textually; 1 compile adaptation folded IN (see below)
  2514e09 -> 34b2903  CLEAN
Next action: full suite running on 34b2903; expect ~1750.
FALLBACK: `git reset --hard 5cbf072` (pushed).

## BRANCH 3 RESOLUTIONS
- broker.rs (ddc80fa, 1 hunk): kept HEAD `drop_to`, deleted the branch's
  `pub fn drop_privileges` (37 lines) that auto-merge had left ALONGSIDE it.
  Bodies were behaviourally identical (same libc setgroups/setgid/setuid +
  same post-drop verification + same error string); the tip's doc comment is
  strictly better because it names curl as well as git. Made `drop_to`
  `pub(crate)` so providers::cloudflare::api can reach it. NET CHANGE TO
  broker.rs FOR THIS WHOLE LANDING = that one visibility keyword, deliberately,
  because p1-018-mcp-auth is live in this file.
- providers/mod.rs (ddc80fa, 2 hunks): both "keep both sides".
  * register: git, then cloudflare, then mcp (run_dir preserved).
  * operation_ids: 7 cloudflare + 3 git + mcp.request, sorted. 11 total.
- NOT conflicts, compile breaks the cherry-pick could not see (new files
  apply clean; only the build reveals them). All in
  providers/cloudflare/tests.rs:
  * 3x `service.add(peer, svc, host, scheme, None, value)` 6-arg ->
    `NewService{service,host,scheme,username,path:"",auth:None,port:None}`.
    auth:None chosen to preserve the branch's behaviour exactly (the provider
    hardcodes `Authorization: Bearer`); matches the tip's own bearer fixtures.
  * 2x `use_capability(peer, rec)` -> `+ Vec::new()` for P0-003's `body`.
    service.rs documents body as "for the one operation that carries a
    message" (mcp), so empty is right. Same adaptation integrate-2 made.
  * added `NewService` to the `use crate::service::{..}` import.
- api.rs: `crate::broker::drop_privileges` -> `crate::broker::drop_to`.
- **A SIXTH BREAK THE HANDOVER DID NOT LIST**, in 3d2dc43's new file
  apex/src/cloudflare.rs:201 `store()`. This is a DIFFERENT `add` — the CLI
  client `apex_secret_core::client::Client::add`, which P0-003 grew from 5
  args to 8: `path: &str`, `auth: &str`, `port: Option<u16>`.
  Passed `path: ""`, `auth: "bearer"`, `port: None`.
  `auth` is NOT cosmetic and is the one real judgement here. Evidence it is
  right, not guessed:
  * store.rs `header_value()` maps "raw" => the value verbatim, anything else
    => `format!("Bearer {value}")`. The Cloudflare provider hardcodes
    `Authorization: Bearer {token}` (api.rs:323), so "bearer" reproduces the
    branch's own behaviour byte for byte; "raw" would have sent the token
    unprefixed and broken every call.
  * `apex secret add` clap default is `default_value = "bearer"` (secret.rs:82).
  * migrate.rs's rule is bare token => "bearer", full header => "raw"; a
    Cloudflare API token is a bare token.
  * store.rs `default_auth()` is "bearer", `default_username()` is
    "x-access-token" (which the branch already passes explicitly).
  Folded INTO 1322181 rather than added on top, so every commit on the tip
  builds (integrate-2's precedent, and the reason I re-picked the last two).

## PLAN
1. apex-os chore/run-clippy (1 commit) -> roadmap/v2.2 e4e221f
2. apex-os task/p1-045-updates-trust (6 commits, 5eb7f1a)
3. apex-os task/p1-002-cloudflare (6 commits, 2514e09) MINUS 8110944
4. apex-shell fix/locked-hint (1 commit, 87c84f3) -> roadmap/v2.2 0fd12ee

## FACTS FOUND SO FAR
- int-os roadmap/v2.2 = e4e221f CONFIRMED. int-shell roadmap/v2.2 = 0fd12ee CONFIRMED.
- **8110944 ALREADY LANDED** as bb5b355 by integrate-2 (out-of-order landing,
  with a one-token `Vec::new()` adaptation for P0-003's third arg on
  use_capability). So during the p1-002 rebase it must be DROPPED, not
  resolved. integrate-2's card documents this explicitly.
- Known pre-existing flake: apex-secretd
  `tests::a_stale_socket_from_a_dead_daemon_is_replaced`.
