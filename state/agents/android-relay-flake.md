# android-relay-flake — a flaky Android test is making every CI gate unreliable

items: none (no roadmap id — CI reliability, found by the orchestrator)
repo: apex-os
worktree: /var/tmp/apex-work/wt-android-relay-flake (create it)
branch: task/android-relay-flake, cut from origin/roadmap/v2.2
lab: /var/lab-scratch/android-relay-flake

Dispatched round 39, 2026-09-21 ~17:50 AWST, by the orchestrator.

## WHY THIS UNIT EXISTS

CI run **35584033343** on `roadmap/v2.2` (merge `84407857`) went red on
`Android client / App unit tests`:

```
> Task :app:testDebugUnitTest FAILED
53 tests completed, 1 failed
RelayDiallerTest > a failed dial leaves no socket open() FAILED
    org.opentest4j.AssertionFailedError at RelayDiallerTest.kt:143
```

**That commit changed one markdown file.** `84407857` is the
`task/kernel-publish` merge and its whole diff is
`ROADMAP/evidence/kernel-publish-20260921.md`. Nothing Android moved. The same
job passed on the commit before it. So this is a FLAKE, not a regression — and
a flaky gate is worse than a missing one, because it teaches everybody to
re-run red CI instead of reading it.

## THE DEFECT, as far as the orchestrator got (verify, do not trust)

`android/app/src/test/kotlin/com/apexos/remote/pairing/RelayDiallerTest.kt`.
The test dials 20 times against a `Double` that answers `409 Conflict` with
`upgrade = false`, then asserts immediately:

```kotlin
repeat(20) { assertThrows<RelayException> { RelayDialler.dial(...) } }
assertEquals(20, relay.closedByClient, "sockets the client left open")
```

`closedByClient` is incremented on the double's **acceptor thread**, inside
`serve()`, only after `input.read()` returns `< 0` — i.e. after that thread
observes the client's close:

```kotlin
if (!upgrade) {
    if (input.read() < 0) closedByClient++
    socket.close()
    return
}
```

The client's 20th `dial` can throw, and the test thread can reach the
assertion, before the acceptor thread has returned from its 20th `input.read()`.
The expected observed value is then 19. Nothing synchronises the two.

**Do not "fix" this by loosening the assertion.** The test exists for a real
thing its own comment names — one leaked descriptor per attempt is a phone that
eventually cannot open a file, and the symptom appears nowhere near the cause.
A fix that makes a genuine leak pass is worse than the flake.

The shape that keeps the meaning is to WAIT for the double to observe all 20
closes, with a bounded timeout, and fail if it does not: a `CountDownLatch`
counted down where `closedByClient++` is today, `await(n, SECONDS)` in the
test, and the count asserted after. A leak then still fails — it just fails on
the timeout instead of instantly.

## WHAT DONE LOOKS LIKE

1. The race is CONFIRMED, not assumed — reproduce it, e.g. by running the one
   test in a loop, or by inserting a delay on the acceptor side and watching
   the assertion fail deterministically.
2. Fixed so that it cannot flake, and **proven both ways**: it still FAILS
   when the client genuinely leaks a socket. Simulate the leak (a dialler that
   does not close, or a stubbed close) and show red.
3. Check the other 52 tests in that file and its siblings for the same shape —
   an assertion on a counter another thread writes. If the pattern repeats, say
   so; fix what is cheap and list what is not.
4. Evidence file `ROADMAP/evidence/android-relay-flake-20260921.md`: the run
   id, the reproduction, the fix, and the both-ways proof.

## BOUNDS

- `android/` only, plus the evidence file. Do NOT touch the relay, the daemon,
  or anything outside `android/app/src/test/`unless the defect turns out to be
  in production code — and if it does, say so loudly on this card first,
  because that is a different and much more interesting finding.
- Gradle offline/flaky network: if the toolchain cannot run locally, say so on
  the card rather than guessing, and use CI to verify (`gh run`).

## NEXT

- Read `RelayDiallerTest.kt` and confirm the race described above.

## DONE

## IN PROGRESS

## FOUND

## BLOCKED ON

- nothing
