## LANDABLE

**`dc7a470f`** — the flake is confirmed, closed and proven red both ways; the
assertion it guards is byte-for-byte unchanged. Merges cleanly onto
`roadmap/v2.2` (checked against tip `11c45d36`, not against the cut point).
`:app` 53/0, `:core` 526/0. Three files, all inside the unit's bounds:
`android/app/src/test/.../RelayDiallerTest.kt`, one `testLogging` block in
`android/app/build.gradle.kts`, and the evidence file.

---

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

- Orchestrator lands `dc7a470f`. Nothing else is outstanding; the only thing
  left running is CI run **35586292689** (`workflow_dispatch` on
  `task/android-relay-flake`), which is confirmation, not a gate — both suites
  were already run locally at the counts CI asserts.

## DONE

- Read the card, `RelayDiallerTest.kt` and `RelayDialler.kt`.
- **The test runs locally.** The card's "if the toolchain cannot run locally"
  escape is not needed: 13 s warm, whole class green.
- **RACE CONFIRMED, deterministically.** `Thread.sleep(200)` before
  `closedByClient++` gives `sockets the client left open ==> expected: <20>
  but was: <19>` — the same assertion, same line 143, as CI run 35584033343.
  The orchestrator's diagnosis was right in every particular.
- **Fixed and pushed: `aaf2783d`.** 53 tests, 0 failures (the same count CI
  reports). Assertion untouched; the test now WAITS for the acceptor.
- **PROVEN BOTH WAYS.**
  - *Race closed*: the same `Thread.sleep(200)` probe that was deterministically
    `expected: <20> but was: <19>` before the fix is **GREEN** after it (7 s).
  - *A genuine leak still fails*: deleting `runCatching { socket.close() }`
    from `RelayDialler.dial`'s second catch — **production** code, the exact
    line the test defends — and retaining the socket gives **RED**:
    `sockets the client left open ==> expected: <20> but was: <0>`, 1 m 45 s.
    The socket is retained on purpose: since JDK 13 `NioSocketImpl` registers
    a `Cleaner` that closes an unreachable socket's fd on GC, so an unretained
    "leak" closes itself and proves nothing.
  - Both mutations reverted; tree clean.
- **Sibling sweep done: no second instance.** All 65 Kotlin test files under
  `android/` swept (app 8, core 45, androidTest 12). Twelve contain any
  cross-thread construct; all twelve read in full. Every other cross-thread
  read is ordered by something nameable — `CountDownLatch.await`, `join()`,
  `@Volatile` on the production field, `AtomicInteger`, `poll(30, SECONDS)`,
  or a polling deadline. One near-miss fixed as `252d0d5b` (below).
- Evidence written and pushed: `ROADMAP/evidence/android-relay-flake-20260921.md`.

## IN PROGRESS

- Nothing. Watching CI run 35586292689 only.

## FOUND

- **`Double.live` was ordered only by accident** — assigned on the acceptor
  thread, read on the test thread by `sendCarried()`/`close()`, and not
  `@Volatile`. Safe today only because every caller of `sendCarried` reaches
  `carried()` first and `carried()` takes the `received` monitor the acceptor
  released after the assignment. That is the order two tests happen to call
  things in, not a property of the field. Fixed in `252d0d5b`.
- **LISTED, NOT FIXED** (different shape, in `:core`, outside this unit's
  bounds): `core/src/test/.../InsecureStorageTest.kt` lines 133 and 184 run
  `assertEquals` on the responder thread with no `finally { toDevice.close() }`.
  A failure there kills the responder silently, leaves the client blocked in
  `PipedInputStream.read`, and reports as a 20-second `joinOrFail` instead of
  naming the mismatched byte. Not a flake — a legibility problem in the
  failing case. Worth a cheap follow-up unit.
- **A genuine leak was, before this fix, EIGHT MINUTES of the right exception
  for the wrong reason.** `Opening.accept` (`core/.../Relay.kt:584`) catches
  every `IOException` and rethrows it as `RelayException`. A
  `SocketTimeoutException` is an `IOException`. The double serves one
  connection at a time, so a client that really leaked would block the
  acceptor's EOF read for ever, and each later dial would fail on its own
  25 s `handshakeTimeoutMs` — wrapped into a `RelayException`, which
  `assertThrows` happily accepts. **Measured, not assumed:** a probe dialled
  once (leaking) then again, and got
  `RelayException ... "the relay refused the connection: Read timed out"
  after 25026ms; isRelayException=true`. Nineteen of those ≈ 475 s before the
  count assertion was even reached. So the fix also puts a bounded deadline on the double's EOF
  read: not tidying, it is what makes the failing direction readable.
- **CI could not say what it saw.** The whole log for run 35584033343 was
  `AssertionFailedError at RelayDiallerTest.kt:143` — a line number and no
  value, when 19 (race) versus 0 (real leak) is the entire diagnosis. `:core`
  has had `testLogging { exceptionFormat = FULL }` all along and `:app` had
  not. Added. **This is one step outside `android/app/src/test/`** — it is
  `android/app/build.gradle.kts`, still inside the unit's `android/` bound,
  and flagged here deliberately.
- **The local Android toolchain works, with one trap.** An Android SDK is
  already at `/var/tmp/android-sdk` (platforms/android-36, build-tools 35+36)
  and the Gradle cache is warm (3.1 GB), so `:app:testDebugUnitTest` takes 13 s.
- **`/usr/lib/jvm/java-21-openjdk` on this laptop is BROKEN and owned by no
  RPM.** Its `conf` is a dangling symlink to `/etc/java/java-21-openjdk/...`,
  which does not exist; only `java-latest-openjdk` (27-ea) is installed. Any
  JVM started from it dies with `InternalError: Error loading java.security
  file` before Gradle's wrapper even unpacks. Almost certainly a leftover of
  the 2026-09-17 Steam recovery file-copy (`/usr` files added, no `/etc`
  payload replay). Not mine to fix — worked around by fetching Temurin 21 into
  scratch. **Any agent running the Android build on this laptop must use**
  `JAVA_HOME=/var/lab-scratch/android-relay-flake/tools/jdk-21.0.12.1+1` and
  `ANDROID_HOME=/var/tmp/android-sdk`; `/usr/bin/java` (27-ea) is refused by
  AGP and `/usr/lib/jvm/java-21-openjdk` does not start.

## BLOCKED ON

- nothing
