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
  3d2dc43 -> 7eebd86  CLEAN textually; 1 compile adaptation folded IN (see below)
  2514e09 -> bc4f06e  CLEAN
BRANCH 3 PUSHED at bc4f06e. 1750/0 (+65, zero removed). secret-broker 65/0.
Per-commit build check: all 5 rc=0.

## MY OWN ERROR, CAUGHT AND FIXED (report it)
I ran `git cherry-pick -n 3d2dc43`, then `git apply <fix>` (working tree ONLY,
not staged), then `git commit -C 3d2dc43` — which commits the INDEX. The fix
was therefore NOT in the commit. cargo build/test read the WORKING TREE, so
they were green (1750/0) while the committed tip did not compile. I pushed
that tip (34b2903) before spotting the `M apexd/apex/src/cloudflare.rs` in
`git status`. Fixed by re-picking with `git apply --index` and verifying
`git show HEAD:<file>` rather than the working tree; force-pushed bc4f06e over
it. Broken-remote window ~4 minutes.
LESSON, now a habit for the rest of this job: after every commit, assert
`git status --porcelain` is EMPTY, and verify content with `git show HEAD:path`,
never by reading the working tree.
Added a per-commit build check (percommit.sh) so "every commit builds" is
measured, not asserted. NOTE its first run reported 5/5 FAILURES that were the
SCRIPT's bug (built from the repo root; the manifest is apexd/Cargo.toml, there
is no root one) — a failure to look, not an absence. Fixed, then 5/5 rc=0.

## COORDINATOR MESSAGE (mid-task)
- chore/run-clippy as landed is DEFECTIVE: podman run has no --network=host,
  rust:1 downloads clippy, so no DNS on katana; and `>/dev/null 2>&1` on
  `rustup component add` hides "Temporary failure in name resolution" and
  reports it as "could not add the clippy component" — a failure to look
  reported as an absence. Corrected version is on task/p1-018-mcp-auth at
  16fc8ae. Take it, and VERIFY ON KATANA.
- ALSO LAND task/p1-018-mcp-auth (8 commits, tip 16fc8ae) after the three.
  Carries security fix c38075f. Touches apex-secret-core::paths
  (new control_socket_in) -> expect interaction with the Cloudflare branch.
FALLBACK: `git reset --hard bc4f06e` (pushed).

## REMAINING ORDER (do not let added scope orphan the shell work)
1. p1-018: `git cherry-pick 526e16c..16fc8ae` (9 commits; 526e16c is ALREADY
   upstream as my 7f14a00 — `git cherry` confirms it as `-`). It carries the
   run-clippy.sh fix as 16fc8ae.
2. per-commit build over the 9, then push.
3. katana + L16 clippy runs (independent machines).
4. api.rs-on-run_curl decision (feature vs resolution).
5. apex-shell fix/locked-hint.
6. report.
GUARD ADDED to runtests.sh + percommit.sh: refuse to run on a dirty tree
(exit 3). Mutation-proved: dirtied cloudflare.rs -> rc=3, restored -> runs.

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
