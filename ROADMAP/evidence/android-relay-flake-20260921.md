# A flaky Android gate, confirmed and closed — 2026-09-21

Unit `android-relay-flake`, round 39. Branch `task/android-relay-flake`,
cut from `roadmap/v2.2` at `69253336`.

## What went red, and why it could not have been a regression

CI run **35584033343**, `Android client / App unit tests`, on `roadmap/v2.2`
at merge `84407857`:

```
> Task :app:testDebugUnitTest FAILED
53 tests completed, 1 failed
RelayDiallerTest > a failed dial leaves no socket open() FAILED
    org.opentest4j.AssertionFailedError at RelayDiallerTest.kt:143
```

`84407857` is the `task/kernel-publish` merge and its entire diff is one
markdown file, `ROADMAP/evidence/kernel-publish-20260921.md`. Nothing under
`android/` moved, and the same job passed on the commit before it. A gate that
goes red on a markdown file is not reporting a defect in the change; it is
teaching everybody to press re-run, which is worse than having no gate at all.

## The race, reproduced rather than reasoned about

`android/app/src/test/kotlin/com/apexos/remote/pairing/RelayDiallerTest.kt`
dialled a loopback `Double` twenty times, each dial refused with `409
Conflict`, and then read the double's counter directly:

```kotlin
repeat(20) { assertThrows<RelayException> { RelayDialler.dial(...) } }
assertEquals(20, relay.closedByClient, "sockets the client left open")
```

`closedByClient` is incremented on the double's **acceptor thread**, inside
`serve()`, only once `input.read()` has returned end-of-stream — that is, only
once that thread has observed the client's close. Nothing ordered the two
threads.

Attempts 1..19 are flushed by the dial that follows them: the double serves one
connection at a time, so dial *N+1* cannot get its response until `serve(N)` has
returned, and `serve(N)` returns only after the increment. **Only the twentieth
is unordered.** The expected observed value under the race is therefore exactly
19, which is what CI saw.

Proved by injecting a delay on the acceptor side, uncommitted, immediately
before the increment:

```kotlin
Thread.sleep(200) // RACE-PROBE, uncommitted
if (input.read() < 0) closedByClient++
```

```
RelayDiallerTest > a failed dial leaves no socket open() FAILED
    org.opentest4j.AssertionFailedError: sockets the client left open
        ==> expected: <20> but was: <19>
```

Deterministic, same assertion, same line 143 as CI. The orchestrator's
diagnosis was correct in every particular.

## The measurement that changed the shape of the fix

Waiting for the acceptor is necessary but is not sufficient, and the reason is
not visible from the test.

`Opening.accept` (`android/core/src/main/kotlin/com/apexos/remote/core/Relay.kt`,
line 584) catches **every** `IOException` from the response read and rethrows it
as a `RelayException`. A `SocketTimeoutException` is an `IOException`. The
double serves one connection at a time. So a client that genuinely leaked a
descriptor would leave the acceptor's EOF read blocked for ever, and every
later dial would fail on its own 25-second `handshakeTimeoutMs` — as a
`RelayException`, which `assertThrows<RelayException>` accepts.

Measured, not assumed. With the leak mutation below in place and the double's
read left unbounded, a probe dialled once (leaking) and then dialled again:

```
MEASURED: dial #2 threw com.apexos.remote.core.RelayException after 25026ms;
message=the relay refused the connection: Read timed out; isRelayException=true
```

Nineteen of those is **~475 seconds of the right exception thrown for the wrong
reason** before the count assertion is even reached. A bounded deadline on the
double's EOF read is therefore part of the fix and not tidying: it is what makes
the failing direction fail in a time a person will wait, and say what it saw.

## The fix — `aaf2783d`

`android/app/src/test/kotlin/.../RelayDiallerTest.kt`, plus one line of
`android/app/build.gradle.kts`.

**The assertion is unchanged**, including its message. The test exists for a
real thing its own comment names — one leaked descriptor per attempt is a phone
that eventually cannot open a file, and the symptom appears nowhere near the
cause — so the fix waits for the observation instead of loosening what is
observed.

1. `closedByClient` becomes `closedByClient(expecting: Int): Int`, a bounded
   wait that returns however many closes the acceptor had seen when it returned.
   It is the shape `carried(n)` in the same file already had, with the same
   5-second deadline, for the same reason: both are the test thread waiting on
   the acceptor thread.
2. The counter itself is private. A bare getter hands the test thread whatever
   happened to be true at the instant it looked, which is a race with a volatile
   read in it, not a fixed one.
3. `serve()`'s EOF read gets `soTimeout = PATIENCE_MS`, and a timeout is **not
   counted** — a connection the double had to give up waiting on is precisely
   the descriptor the test is looking for.
4. `:app` gains the `testLogging { exceptionFormat = FULL }` block `:core` has
   had all along. Everything CI could say about this failure was
   `AssertionFailedError at RelayDiallerTest.kt:143` — a line number and no
   value, when 19 (a race) versus 0 (a real leak) is the entire diagnosis.

## Proven both ways

| Direction | Change | Result |
|---|---|---|
| The race is closed | `Thread.sleep(200)` on the acceptor before the increment — the probe that was deterministically `expected: <20> but was: <19>` before the fix | **GREEN**, 7 s |
| A genuine leak still fails | `RelayDialler.dial`'s second `catch` no longer closes the socket, and retains it | **RED**: `sockets the client left open ==> expected: <20> but was: <0>`, 1 m 45 s |

The leak mutation is in **production** code — `dial`'s `runCatching {
socket.close() }` is the line the test exists to defend — and the socket is
retained in a list on purpose. Since JDK 13, `NioSocketImpl` registers a
`Cleaner` that closes an unreachable socket's descriptor on GC, so an
unretained "leak" closes itself nondeterministically and would have proved
nothing. A real leak is a retained socket.

Both mutations were reverted; `git status` is clean on the branch.

## The sibling sweep — no second instance

All **65** Kotlin test files under `android/app/src/test/` (8),
`android/core/src/test/` (45) and `android/app/src/androidTest/` (12) were
swept for the same shape: a field one thread writes and another asserts on with
nothing between them. Twelve files contain any cross-thread construct at all;
all twelve were read in full, along with the production classes they assert on.
There are no coroutines or executors in the tests — only raw `Thread`, piped
streams, and `runBlocking`, which blocks the test thread.

**No second assertion of this kind exists.** The ones that look like it are
ordered, and by something nameable each time: `CountDownLatch.await`,
`Thread.join`, `@Volatile` on the production field (`PtyAttachment.attachments`,
`PtyAttachment.attempts`, `MachineLink.connections`), `AtomicInteger`, a
`LinkedBlockingQueue.poll(30, SECONDS)`, or a polling deadline.

One near-miss, fixed as `252d0d5b`: `Double.live` in this same fixture is
assigned on the acceptor thread and read on the test thread by `sendCarried()`
and `close()`, and was not `@Volatile`. It is ordered today only because every
caller of `sendCarried` reaches `carried()` first, and `carried()` takes the
`received` monitor the acceptor released after the assignment. That is an
accident of the order two tests happen to call things in, not a property of the
field. The keyword costs nothing.

**Listed, not fixed** (different shape, and in `:core`, outside this unit's
bounds): `core/src/test/.../InsecureStorageTest.kt` lines 133 and 184 run
`assertEquals` on the responder thread with no `finally { toDevice.close() }`.
A failure there kills the responder silently, leaves the client blocked in
`PipedInputStream.read`, and the test reports as a 20-second `joinOrFail`
rather than naming the mismatched byte. Not a flake — a legibility problem in
the failing case.

## Verification

Local, and on CI.

```
:app:testDebugUnitTest   53 tests, 0 failures   (the count CI reports)
:core:test              526 tests, 0 failures   (the workflow's floor is 526)
```

CI run **35586292689**, `workflow_dispatch` on `task/android-relay-flake`.

## One trap for the next agent on this laptop

`/usr/lib/jvm/java-21-openjdk` is **broken and owned by no RPM**. Its `conf` is
a dangling symlink to `/etc/java/java-21-openjdk/...`, which does not exist;
only `java-latest-openjdk` (27-ea) is installed. Any JVM started from it dies
with `InternalError: Error loading java.security file` before Gradle's wrapper
finishes unpacking. Almost certainly a leftover of the 2026-09-17 Steam
recovery file copy, which added `/usr` files and deliberately replayed no
`/etc` payload. Not repaired here — that is machine state, and outside this
unit.

The Android toolchain otherwise works locally and the card's "if the toolchain
cannot run locally" escape was not needed: a warm run of the whole `:app` suite
is 13 seconds. What it needs is

```
ANDROID_HOME=/var/tmp/android-sdk          # already present: android-36, build-tools 35 + 36
JAVA_HOME=<a working JDK 21>               # AGP refuses 27-ea; the system's 21 does not start
```

A Temurin 21 was unpacked into `/var/lab-scratch/android-relay-flake/tools/`
for this run.
