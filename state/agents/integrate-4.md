# integrate-4
items: (integration only)
  apex-os:    task/p1-044-firewall-live, task/p2-005-device-maturity,
              task/p1-020-agent-graph-daemon
  apex-shell: task/p1-020-agent-graph, task/p1-044-firewall-settings,
              task/p1-038-labwc-parity, task/p1-048-guided-settings
repo: both
worktree: /var/tmp/apex-work/int-os and /var/tmp/apex-work/int-shell
branch: roadmap/v2.2

## NEXT
apex-shell landing 3: task/p1-038-labwc-parity (tip 2ea0790, 3 commits).
  cd /var/tmp/apex-work/int-shell
  git checkout -b land/p1-038 origin/task/p1-038-labwc-parity
  git rebase --onto roadmap/v2.2 d2d1f33cebd4bece0da32e5f0c676ecea4b1fe3f
  (expect a conflict in .github/workflows/ci.yml only — additive, keep both)
  git checkout roadmap/v2.2 && git merge --ff-only land/p1-038
  /var/tmp/apex-int4-logs/shelltests.sh p1-038 ; then push --force-with-lease
  DO NOT run tests/run-labwc-matrix-test.sh — it is a nested-compositor runner
  written before p1-048's headless guard. Hold it until p1-048 is on the tip.
Then apex-os landing 2: task/p2-005-device-maturity, fork 9a24d2b.

## DONE
- **apex-os task/p1-044-firewall-live LANDED AND PUSHED: roadmap/v2.2 = 9fafab4**
  (was b2d7905). 7 commits: b599ca6 f2d5289 a811277 c3d2657 df28052 c1f450b
  9fafab4. **ZERO CONFLICTS.**
  It changes NO Rust: `git diff --name-only b2d7905 9fafab4 -- apexd/` is EMPTY
  (only files/ and tests/), so cargo test and clippy are unchanged by
  construction rather than by assertion.
  tests/test-apex-firewall.sh, before -> after: **25 passed/0 failed/1 skipped
  -> 31/0/1**. (The 1 skip is "the ruleset parses", which needs root; the suite
  deliberately never LOADS the policy.)
  NOT RUN, deliberately: tests/test-apex-firewall-live.sh and
  test-apex-firewall-ssh.sh. Both ssh to katana and load a default-drop policy
  on it. The p1-044 card records that its own live run completed and left
  katana with an empty ruleset; re-running them here would risk exactly the
  lockout the suite exists to detect, on a machine I was told to keep
  read-only.
- **apex-shell task/p1-044-firewall-settings LANDED AND PUSHED:
  roadmap/v2.2 = b2bc964** (was ed1c466). 2 commits: 4397145 b2bc964.
  **ZERO CONFLICTS** — the anticipated qmldir/PageRegistry clashes did not
  happen; p1-020 and p1-044 add in different regions of both files.
  Counts before -> after:
     check-no-conflict-markers  PASS -> PASS
     settings-semantics         33/0 -> 33/0
     settings-pages             15/0 -> **16/0** (+1: the new FirewallPage is
                                       picked up by the page enumeration)
     check-color-tokens         22/0 -> 22/0    EXPECT_WHITE_FG 211, unchanged
     check-scale-tokens          5/0 ->  5/0
     agent-state                27/0 -> 27/0
  Branch's own suites: check-firewall-ui 29/0, firewall-test.js all-pass.
  qmldir has no duplicate type registration after the two landings.
- **apex-shell task/p1-020-agent-graph LANDED AND PUSHED: roadmap/v2.2 = ed1c466**
  (was 8d081ff). 5 commits: ce9eb79 6cbd472 56b7e2f 3af5e24 ed1c466.
  ONE conflict, in src/services/agents/SessionRow.qml, two hunks, both
  "keep both sides":
   * header comment block — HEAD's DIMENSION 1 / break-glass note and the
     branch's SECOND LAYER note are about different things; kept both.
   * the property block — kept HEAD's nativeLabel / breakGlass / nowMs / Timer
     AND the branch's computed `height: header.height + ...`. NOTE: a naive
     keep-both leaves TWO row-level `height:` properties (HEAD's
     `height: Theme.px(52)` moved into the new `Item { id: header }`), which is
     a QML duplicate-property error. Removed the stale one; verified only one
     row-level height remains and the header Item keeps Theme.px(52).
  qmldir and .github/workflows/ci.yml auto-merged clean.
  Counts, before -> after:
     check-no-conflict-markers  PASS -> PASS
     settings-semantics         33/0 -> 33/0
     settings-pages             15/0 -> 15/0
     check-color-tokens         22/0 -> 22/0   EXPECT_WHITE_FG 211, unchanged
     check-scale-tokens          5/0 ->  5/0
     agent-state                27/0 -> 27/0
  Branch's own suites after landing: agentgraph 67/0, notifybus 41/0,
     agenttelemetry 59/0, check-agent-settings 17/0, check-remote-agents 13/0,
     agent-policy all-pass.
  qmllint on the resolved SessionRow.qml: rc=0, 4 warnings, all
  `Unqualified access` on `row.session` inside Repeater delegates — the same
  class the branch's own file already had. Nothing new introduced.

## THE ONE JUDGEMENT CALL IN LANDING 1 (flag in the report)
`agenttelemetry-test.js` "SessionRow does not freeze the clock in a property"
FAILED after the rebase, 58/1. It is a REBASE-INTRODUCED cross-branch
interaction, not a defect in either author's work, and I verified that rather
than assuming it:
  - `git show origin/task/p1-020-agent-graph:...SessionRow.qml` has NO nowMs.
  - `git show 8d081ff:...SessionRow.qml` HAS `property double nowMs: Date.now()`
    plus the 1s Timer that writes it (break-glass, landed earlier).
  So each side is green alone; only the merged file trips the check.
The check was `!/property\s+\w+\s+\w+\s*:\s*Date\.now\(\)/`, and its own comment
says what it is for: "a binding whose only input is Date.now() has no
dependencies, so QML evaluates it once". SessionRow's `nowMs` is written every
second by a Timer, so it is the opposite of that defect — the regex is a
false positive on a HELD-AND-TICKED clock.
FIX (folded INTO ed1c466, not added on top): collect the property NAMES bound
to Date.now(), and flag only those that are never assigned again
(`\b<name>\s*=\s*Date\.now\(\)`). A `readonly property` cannot be assigned, so
the real frozen case is still caught by construction.
MUTATION-PROVED both directions:
  - SessionRow Timer changed to `nowMs = nowMs + 1000` (nothing re-reads the
    clock) -> FAIL, 58/1, naming nowMs.
  - `readonly property real nowFrozen: Date.now()` added to TelemetryStrip
    -> FAIL, 58/1, naming nowFrozen.
  - restored -> 59/0.
REJECTED alternative: changing SessionRow to `property double nowMs: 0` +
Component.onCompleted. That is a behaviour change to already-landed §3.4 code
to satisfy a heuristic, and it leaves the countdown at epoch 0 for up to a
second on first paint.

## IN PROGRESS
- nothing half-written. Tree clean, pushed.

## MEASURED BASELINES
apex-shell @ 8d081ff: 33/0, 15/0, 22/0 (WHITE_FG 211), 5/0, 27/0, markers PASS.
apex-os @ b2d7905: build rc=0. `cargo test --locked --workspace --no-fail-fast`
  = **1799 passed / 2 failed**, 29 binaries. BOTH failures are pre-existing and
  explained below; 1799+2 = 1801 = integrate-3's recorded total, so no test was
  lost. tests/run-clippy.sh roadmap/v2.2 -> **PASS clippy is clean (rc=0)**;
  the `--network=host` fix is present on b2d7905.
Scripts: /var/tmp/apex-int4-logs/{runtests.sh,count.sh,shelltests.sh}
  (runtests.sh refuses a dirty tree, exit 3 — integrate-3's guard, kept)
CARGO_TARGET_DIR=/var/tmp/apex-build-cache/int4

## FOUND — TWO REAL DEFECTS IN CODE I AM ONLY PASSING THROUGH
### 1. apex-agentd deadlocks in the forked child: `unsetenv` after `fork()`
`apexd/apex-agentd/src/pty.rs` `spawn()` forks and, in the child, calls
`libc::unsetenv()` and `libc::putenv()`. Neither is async-signal-safe: glibc
takes `envlock` inside both. In a multithreaded process — which apex-agentd is,
and which the Rust test harness is — a fork whose child touches the environment
deadlocks forever if any other thread held that lock at fork time.
NOT INFERRED. Measured on the baseline run:
  - the child hung 6+ minutes at 0% CPU, `wchan=futex_do_wait`, syscall 202.
  - `eu-stack -p 33423`:
        #0 __lll_lock_wait_private
        #1 unsetenv
        #2 apex_agentd::pty::spawn
        #3 apex_agentd::pty::tests::the_requested_window_size_reaches_the_child
  - the other thread is identified too: `apex-agentd/src/grants.rs:496,510,511`
    call `std::env::set_var`/`remove_var`. Their own comment says "`set_var` is
    process-global, so the tests that need it run one at a time" — but that
    mutex excludes only each OTHER, not the pty tests that fork.
The child path's comment claims "No allocation, no Rust I/O, no panicking: only
raw syscalls", and even notes "clearenv can allocate on some libcs" — so the
hazard was known and the chosen replacements take the same lock.
THIS IS NOT TEST-ONLY. apex-agentd is a live multithreaded daemon; any thread
inside a setenv-family call while a session spawns hangs that session forever
with a pty allocated and no exec, and no timeout anywhere rescues it.
The fix is to build the child's environment as a `Vec<CString>` in the PARENT
and hand it to `execve`/`execvpe` instead of mutating environ in the child.
egress.rs:404 forks too and needs the same audit.
I did NOT fix it: pty.rs is in none of my seven branches, and it is a
correctness change to a security boundary, not an integration resolution.
ROUTE IT TO THE COORDINATOR — it is also the true cause of what integrate-3
recorded as a "contention flake".
Unblocking recipe if it hangs again: `eu-stack -p <pid>` to confirm, then
`kill -9` ONLY the pid whose /proc/<pid>/exe is under
/var/tmp/apex-build-cache/int4/. Never the /usr/bin/apex-agentd daemons.

### 2. `renewing_a_grant_that_does_not_exist_...` fails FROM THIS CGROUP
`apex-agentd/tests/system_grants.rs:331` expects "no active system-access
grant"; it gets "a scheduled-job request cannot renew a system-access grant".
Cause, read out of the source rather than guessed: `apex-agentd/src/origin.rs`
`classify()` returns `ScheduledJob` for any cgroup containing `/user@` and
`.service`. This session's cgroup is
  /user.slice/user-1000.slice/user@1000.service/app.slice/apex-roadmap-resume.service
because the AUTORESUME TIMER started it. integrate-3 ran from a login
`session-N.scope` and saw 1801/0.
CONSEQUENCE FOR EVERY FUTURE AGENT: any agent dispatched by
apex-roadmap-resume.service will see this one failure and it is NOT a
regression. It is arguably also a real gap — the suite has no test asserting
its own origin, so it silently measures a different code path depending on who
started it.

## FOUND — smaller
- The brief's stated tips were the OLDEST commit for two shell branches:
  task/p1-038-labwc-parity tip is **2ea0790** (not d2523e2) and
  task/p1-048-guided-settings tip is **7266f50** (not 0b36897). report.txt
  lists commits oldest-first; the brief read the wrong end. Using origin/<b>.
- The brief said p1-044-firewall-settings forked at 0fd12ee. `git merge-base`
  says ALL FOUR shell branches fork at d2d1f33 (an ancestor of roadmap/v2.2).
- The `[0/10 added lines on tip]` entries are SETTLED for all three apex-os
  branches: the diff against the tip still carries the full branch insertion
  count (1338 / 2259 / 2677). Nothing landed early; nothing to skip.
- Containerfile.base and Containerfile.core both still exist on roadmap/v2.2,
  so p2-005's edits to them are not a modify/delete conflict.
- **HEADLESS HAZARD.** p1-048's 0b36897 found TWELVE tests/run-*.sh that start
  quickshell or a compositor on the INHERITED WAYLAND_DISPLAY — i.e. on Andre's
  desk — including run-nested-labwc.sh, which the brief tells an integrator to
  run. I will not run any of the twelve until p1-048 is on the tip:
  run-agent-center-smoke, run-nested-labwc, run-compositor-facade-test,
  run-hypr-configerrors-test, run-nexus-smoke, run-niri-keybinds-test,
  run-plugin-host-test, run-popup-smoke, run-remote-agent-smoke,
  run-scaling-test, run-service-tier-test, measure-idle-cost/-inhibit.
  VERIFIED SAFE (they unset WAYLAND_DISPLAY and make a private
  XDG_RUNTIME_DIR): run-settings-pages-test.sh, run-agent-state-render-test.sh,
  run-nav-geometry-test.sh.
- I will NOT run tests/test-apex-firewall-live.sh or -ssh.sh: they ssh to
  katana and load a default-drop policy there. Static review only.

## BLOCKED ON
- nothing
