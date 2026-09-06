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

## APEX-SHELL DONE AND PUSHED: roadmap/v2.2 = 8d081ff (was 0fd12ee)
fix/locked-hint landed as 8d081ff. ZERO conflicts (only 7c4e78d touched
Lockscreen.qml/qmldir since its base 05d9412, different regions).
Suites, run from the REPOSITORY ROOT, before -> after, all unchanged:
  check-no-conflict-markers  PASS -> PASS
  settings-semantics         33/0 -> 33/0
  settings-pages             15/0 -> 15/0
  check-color-tokens         22/0 -> 22/0   EXPECT_WHITE_FG 211, unchanged
NOTE: origin/fix/locked-hint (87c84f3) and refs/wip-remote/wt-lockhint-
(2f12fe5) are two snapshots of the SAME tree — `git diff` between them is
empty. No later work was stranded.
Gave it a real commit message; the original was the WIP snapshot placeholder
"wip snapshot of fix/locked-hint in wt-lockhint-". Author preserved as
AndreNijman. JUDGEMENT CALL — flagged; not a trailer rewrite.

## LOCKED-HINT REVIEW (it was unreviewed; verdict: LAND IT, with caveats)
Verified rather than assumed:
- `onSecureStateChanged` LOOKS wrong (property is `secure`) but is RIGHT.
  /usr/lib64/qt6/qml/Quickshell/Wayland/quickshell-wayland.qmltypes:
  Property `secure` has `notify: "secureStateChanged"` (and `locked` has
  `lockStateChanged`). Qt allows a notify name that is not <prop>Changed.
  qmllint on Lockscreen.qml reports no unknown-signal warning.
- `Quickshell.env(variable)` exists (quickshell-core.qmltypes, returns QVariant).
- The whole read-only chain, run live on the L16:
  `loginctl show-user andre -p Display --value` -> `3`
  `busctl ... GetSession s 3` -> `o "/org/freedesktop/login1/session/_33"`
  which confirms the comment's escaping claim EXACTLY (3 -> _33), and the
  regex /"([^"]+)"/ matches that output.
  `loginctl show-session 3 -p LockedHint` -> `LockedHint=no` — the live defect.
- NO POLKIT. org.freedesktop.login1.policy has no action for the lock hint
  (only inhibit-* and lock-sessions, which is for OTHER users' sessions).
  SetLockedHint is on the Session interface and logind checks the caller owns
  the session. Verified by READING the policy, not by calling it — I did not
  call SetLockedHint on Andre's live session.
- qmllint: rc=0. Its only 3 warnings are `QProcess::ExitStatus ... not found`
  on onExited, which is PRE-EXISTING codebase-wide (PowerProfileService.qml
  has the same one). Not introduced here.
- Binds `secure` (compositor-acknowledged) not `locked` — a lock that fails to
  engage is never reported to logind as engaged. Correct, and the point.
CAVEATS TO REPORT (none blocking):
 1. NO TEST of any kind. Nothing asserts the qmldir entry exists or that
    Lockscreen still wires it; a later edit could silently drop it and every
    shell suite would stay green.
 2. No initial sync. `onSecureStateChanged` fires on CHANGE, so if the shell
    restarts while logind still believes the session is locked, LockedHint
    stays "yes" until the next real lock/unlock.
 3. Never exercised end-to-end. It cannot be, headless, without locking
    Andre's session — which I will not do.

## NEXT
ALL FOUR BRANCHES + p1-018 LANDED AND PUSHED. Nothing in progress.
  apex-os    roadmap/v2.2 = b2d7905   (was e4e221f)  1801 passed / 0 failed
  apex-shell roadmap/v2.2 = 8d081ff   (was 0fd12ee)  33/0, 15/0, 22/0
Only remaining work would be the deferred api.rs item below (feature, not
integration) and the OperationSpec `same_everywhere` design question.

## CLIPPY — BOTH MACHINES, ON THE FINAL TIP
  L16    tests/run-clippy.sh          -> PASS clippy is clean (rc=0)
  katana clone of roadmap/v2.2 b2d7905 -> "info: downloading component clippy"
         then "PASS clippy is clean" (rc=0). The download line IS the proof the
         --network=host fix works; before it, katana had no DNS in the
         container and the discarded stderr reported it as a missing component.
  katana scratch cleaned (484M), /var there is 96% full — leave it clean.
It caught TWO REAL LINTS on first run, both in P1-002 code whose author had
recorded "clippy CANNOT BE RUN" (b2d7905):
  - clippy::type_complexity on providers/cloudflare/tests.rs every_operation()
    -> named the tuple `type OperationCase` rather than #[allow]ing it.
  - clippy::useless_format in apex/src/cloudflare/tests.rs device-flow double
    -> dropped format!, unescaped the {{ }} so the served bytes are identical.
      `cargo test -p apex --bin apex cloudflare::` 13/0 before and after.

## api.rs ON TOP OF broker::run_curl — DEFERRED DELIBERATELY, NOT FORGOTTEN
The brief said to do this "not a conflict but do it anyway". Measured, it is a
FEATURE CHANGE and not a resolution, so I did not do it blind:
  - api.rs asks curl to print the HTTP status on its own line after the body
    and parses it (`Reply { status: u16, body }`, api.rs:396). `run_curl`
    returns `Output { code, text }` where `code` is CURL'S EXIT CODE, not the
    HTTP status — the Cloudflare provider's entire success/refusal model
    (`is_ok()` = 200..300, "status 0 means curl never answered") has nothing to
    read.
  - `run_curl` APPENDS STDERR ONTO THE BODY. That alone breaks api.rs's
    "status is the last line" parse.
  - api.rs runs `/usr/bin/curl -q -K -` (absolute path; `-q` first so the
    owner's ~/.curlrc is not read). `run_curl` runs bare `curl` off PATH with
    NO `-q`, while setting HOME to the owner's. Moving Cloudflare onto it would
    be a downgrade in both respects.
  FINDING TO ROUTE ON: those last two are arguably defects in `run_curl`
  itself, on the MCP path — a root daemon spawning a curl that reads the
  owner's ~/.curlrc off a bare-PATH binary. I did NOT touch broker.rs for it:
  p1-018-mcp-auth was live in that file the whole time and the brief's rule was
  minimal edits there. It belongs to whoever owns the MCP provider.

## READING PASS (asked for explicitly; read, not run)
- tests/test-secret-migrate.sh: p1-018 DOES touch apex/src/migrate.rs, adding
  two new skip branches. Neither can fire on this fixture: the new
  `authorization.contains('$')` branch needs a literal `$` in the file, and the
  fixture's heredoc at line 149 is UNQUOTED (`<<JSON`), so `${FAKE_BEARER}` is
  expanded by the shell before the file is written — .claude.json holds the
  literal token. Lines 258/300 grep for the expanded value, confirming it.
  No image-test landmine.
- p1-045 removes NO identifier at all (`git diff | grep '^-'` over fn/struct/
  enum/const across apexd/ is empty), so nothing to re-spell.
- p1-002's only removal across the landing is the `fn drop_to` line itself,
  changed to `pub(crate) fn drop_to`. `drop_privileges` has ZERO references
  left anywhere in the tree.
- The message I changed ("acts on something you name") had exactly ONE
  assertion on it, in end_to_end.rs. Swept *.rs *.sh *.md *.yml *.qml.
- check-doc-verbs.sh on the FINAL tip, run properly (paths + APEX= set, since
  the no-arg form is a phantom pass): 61 valid, 1 deliberate, 0 BAD.
- p1-018 shipped NO docs. Neither `apex mcp connect` nor `--everywhere`
  appears anywhere in docs/ — a real gap, since the refusal message and the
  CLI hint both point users at those verbs.

## COUNT HONESTY (do not let the report imply every run was clean)
1801/0 was measured on TWO COMPLETE runs (test-final.log, test-final2.log),
both rc=0, 29 binaries, from a clean committed tree with the dirty-tree guard
armed. ONE EARLIER RUN did not complete: it hung on the documented
`apex-agentd pty::tests::spawning_runs_the_real_program_on_a_real_terminal`
contention flake with FOUR other agents' cargo runs live (futex_do_wait, 0 CPU,
5.5 min). I killed ONLY my own two test-binary PIDs in my own target dir —
`/var/tmp/apex-build-cache/int3/debug/deps/apex_agentd-<hash>` — after
confirming the real `/usr/bin/apex-agentd` daemons (pids 4599, 5426) were
untouched. `cargo test -p apex-agentd` alone then gave 109/0 in 2.02s, matching
integrate-2's recorded 86/0 in 2.01s for the same binary. Contention, not
regression.

## LIMITATION OF MY OWN GATE FIX (say it plainly)
`may_be_granted_everywhere` is a hardcoded allow-list. The next names-nothing
operation any provider adds is refused `--everywhere` by default — fail-closed,
which is correct — but p1-018's registry test will still PASS silently for it,
because it only asserts `== (id == "mcp.request")`. Intended behaviour, not a
defect, but the `same_everywhere` declaration fact is the real fix.
CONSEQUENCE THE COORDINATOR NEEDS: `cloudflare.account.read` is per-project
only now, which bounds what `apex cf status` can be granted for.
GUARD ADDED to runtests.sh + percommit.sh: refuse to run on a dirty tree
(exit 3). Mutation-proved: dirtied cloudflare.rs -> rc=3, restored -> runs.

## BRANCH 4 p1-018 IN PROGRESS — tip 8011c33 (NOT PUSHED), 1799/1 FAILED
Picks: 023f08c 3a3dea9 05e2db2 b6027e9 bb44763 d079134 f605d3a d23c48e 8011c33
One conflict-free landing EXCEPT one auto-merge defect I fixed inside the pick:
- apex/Cargo.toml gained `toml.workspace = true` TWICE (p1-045 added it with a
  comment; p1-018 added it again). cargo: "duplicate key". Kept p1-045's
  commented line, dropped p1-018's bare one, folded into 05e2db2 (=34016f8).
  Verified via `git show HEAD:` this time, not the working tree.

### THE REAL FINDING — a cross-branch SECURITY interaction, unresolved
`providers::tests::only_an_operation_that_names_nothing_can_be_granted_in_every_project`
FAILS: `'cloudflare.account.read' names nothing: true`, expected false.

p1-018 (853475a) adds `apex secret grant --everywhere` (ANY_PROJECT) gated at
RUNTIME on `op.names_nothing()` (= `resource == None && params.is_empty()`).
Its justification: an operation that names nothing "can only ever reach the
endpoint pinned when its credential was stored, so granting it everywhere
widens where it may be asked for and NOT what it reaches."

That justification is FALSE for `cloudflare.account.read`, which p1-002 added:
  mod.rs:251 resolve() -> if the project's apex.toml binds an account,
    Target::Account(account) -> GET /accounts/{id}   (THE PROJECT'S account)
    else Target::Accounts     -> GET /accounts       (every account the token sees)
So it takes no resource ARGUMENT but still resolves against the directory the
caller stands in. Granting it `--everywhere` therefore DOES widen what it
reaches: an agent in a project the owner never approved could read that
project's bound account with the one stored token.

`names_nothing()` is a STRUCTURAL proxy for "reaches the same thing in every
project", and Cloudflare is the first provider for which the proxy is wrong.
The p1-018 test caught exactly what its own comment says it exists to catch:
"a provider added later ... cannot quietly become grantable everywhere by
inheriting a default." It is the gate working, not a broken test.
DO NOT "fix" this by relaxing the test to `names_nothing() == whatever`; that
silently hands cloudflare.account.read an --everywhere grant nobody reviewed.

### HOW I RESOLVED IT (3e4795b) — JUDGEMENT CALL, FLAG IN REPORT
Fixed p1-018 against its OWN stated intent, using the case it never had.
Not reconciling two authors: the test WAS the spec ("only mcp.request"), the
runtime gate was the looser implementation of it.
- NEW `service::may_be_granted_everywhere(op)` = `names_nothing() && id ==
  "mcp.request"`. An explicit list, not a predicate over the declaration, so a
  provider added later cannot inherit `*` by declaring no resource.
- BOTH sites now use it: the gate (service.rs:237) and the `decided_by` hint
  (service.rs:327). Using it for the hint too closes the loop where the CLI
  would suggest `--everywhere` for an operation the daemon then refuses.
- p1-018's registry test now asserts the GATE instead of `names_nothing()`,
  which is what its own comment says it protects. Intent unchanged.
- NEW runtime test in cloudflare/tests.rs proving `grant("*",
  cloudflare.account.read)` is refused AND the named project still works
  (the declaration test alone proves nothing about runtime).
MUTATION PAIR: `may_be_granted_everywhere` reverted to plain `names_nothing()`
-> apex-secretd bin 109 passed / 2 failed (exactly my two); restored -> 111/0.
REJECTED alternatives: changing account.read to `resource: NAMED` breaks
`apex cf status` (a P1-002 feature change); adding a `same_everywhere` fact to
OperationSpec is the right long-term design but is feature work, not
integration — ROUTE IT TO THE COORDINATOR.

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
