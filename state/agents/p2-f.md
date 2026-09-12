# p2-f
items: P2-017, P2-018, P2-019
repo: apex-os (+ apex-shell half — but see FOUND: the greeter QML is in apex-os)
worktree: /var/tmp/apex-work/wt-p2-f
branch: task/p2-f-2   (off roadmap/v2.2 @ cd4a799e; round 1's task/p2-f is LANDED, 0 ahead)

## NEXT
Write `files/system/libexec/apex-session-watchdog` (verbs record/check/reset/status,
state dir `/var/lib/apex-greet`, override `$APEX_SESSION_WATCHDOG_STATE`) and
`tests/test-apex-session-watchdog.sh` alongside it.

## DONE
(round 1 is landed: 10 commits, see git log of task/p2-f)

## IN PROGRESS
Round 2 item 1 — P2-018's automatic entry.

## FOUND
- `files/desktop/apex-greet/GreetContext.qml` lives in **apex-os**, not apex-shell.
  Round 1's card called the `last-session` stickiness "an apex-shell QML change";
  it is not — no apex-shell worktree is needed for it.
- `/var/lib/apex-greet` is created by tmpfiles at `Containerfile.base:2796`,
  `0755 greetd greetd`. The greeter (running as `greetd`) can write there; a
  session user cannot, which is why `apex-session-select` is a root helper.
- The greeter's launch path is `persistProc` (a `sh -c` that writes
  last-user/last-session, every step `|| true`) and the real `Greetd.launch()`
  is deferred to `persistProc.onExited`. That is the single safest hook point —
  but also means **a hanging hook there is a lockout**, so anything added must
  be wrapped in `timeout` and `|| true`.
- `tests/test-apex-greet-sessions.sh` establishes the house pattern for testing
  the greeter without QML: lift the `sh -c` script out of the shipped
  GreetContext.qml and RUN it. Reuse that.
- `GreetSurface.qml:514` has one status line, priority error > capsOn. A third
  tier can say why a recovery session was preselected.

## BLOCKED ON
(nothing)
