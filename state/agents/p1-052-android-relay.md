# p1-052-android-relay — the relay leg the phone has never had

items: P1-060 C1 (the relay leg), P1-051 remainder, P1-052's Android half
repo: apex-os (`android/` in-tree)
worktree: **make your own** — `/var/tmp/apex-work/wt-p1-052-android`, off
`roadmap/v2.2`, branch `task/p1-052-android-relay`
scratch: `/var/tmp/apex-work/scratch-p1-052-android/` (per-agent; the shared one
is shared and an agent has committed another agent's commit message out of it)

> Dispatched 2026-09-19 (round 33) on a gap the `p1-053e` agent MEASURED rather
> than guessed, and whose own recommendation was that it get its own card: *"a
> half relay client would be worse than none."* Take that seriously — see
> "What not to do" at the bottom.

## The gap, stated exactly

**There is no relay client on Android at all.** Not a broken one, not a partial
one — none.

- `android/app/.../pairing/PairingService.kt:108` prints
  `" (and no relay client has been written yet)"` when `offer.relay != null`.
  The app's own error text is the evidence.
- `:core` has **no WebSocket code**, and there is **no OkHttp in the build**.
- `MachinesScreen.kt:288` already shows `· relay available` vs `· LAN only`, so
  the UI distinguishes a case the client cannot service.

Consequence: **P1-060 C1 is met except the relay leg**, and round 2 on the
device closed everything else in it — the terminal on a real screen, PTY output
read back over an attach, approvals. This is the last piece, and it is a missing
*feature*, not a missing test.

## What already exists, so you write the leg and not the design

**The Rust side is complete and is your specification.**
`apexd/apex-remote-core/src/relay.rs` is a hand-rolled RFC 6455 client, and its
module docs say why in detail. The wire contract is the load-bearing sentence:

> the relay leg is a WebSocket carrying, as binary payloads, exactly the byte
> stream the LAN leg carries over TCP: `[u32 big-endian length][Noise
> ciphertext]`, framed by `apex-remoted`'s `net` module. Nothing above the
> transport knows which leg it is on, which is what makes "the relay carries
> ciphertext it cannot read" a property of the code rather than a promise.

Preserve that property. If anything above the transport learns which leg it is
on, you have broken the thing the design exists for.

**Two pieces of the Android half are already written**:
`android/core/.../Rendezvous.kt` (the relay path `{prefix}/r/{id}?role=guest`,
and why the id is hashed so the relay operator learns nothing) and
`Base64Url.kt`. `PairingService.kt:21` already says `:core` is generic over
streams *because the LAN leg is TCP and the relay leg is not* — so the seam you
need was designed in.

Read `docs/remote.md` for what a relay path must DISCLOSE to the user; that is a
requirement, not documentation. The relay itself is deployed (P1-052 is `done`)
on a custom domain on andrenijman.com — not `*.workers.dev`.

## The device, and the one trap that will cost you 20 minutes if you skip it

Pixel 7a `lynx`, **GrapheneOS**, Android 17 / SDK 37, wireless debugging at
`192.168.1.98:35915`. **Bound twice, so `-s` is not optional.**

**A roadmap-dispatched agent cannot pair.** Your cgroup is
`…/apex-roadmap-resume.service`, so `origin::observe_pid` classifies the suite's
broker as `scheduled-job` and `apex-remoted` refuses every pairing offer —
protocol §7 reserves pairing for a human at the keyboard, correctly. Round 2 hit
this for **8 of 24 tests** after two minutes of gradle. Every run goes through
`tests/in-login-session.sh`, which mints a real PAM login session and does
**not** forward the environment:

```
tests/in-login-session.sh /bin/bash -c \
  'export ANDROID_HOME=/var/tmp/android-sdk JAVA_HOME=/var/tmp/apex-work/scratch-p1-053c/jdk21root/usr/lib/jvm/java-21-openjdk;
   exec android/tools/run-device-suite.sh -s 192.168.1.98:35915'
```

`run-device-suite.sh` now asks this up front and names the wrapper. The system
JDK 21 on the L16 is broken (`conf/security/java.security` is missing, so every
`MessageDigest.getInstance` throws and kills the Gradle *wrapper*) — the
`JAVA_HOME` above is not optional.

Baseline on the tip: device suite **OK (32 tests)**, TalkBack bound;
`:core:test` 459/0; `:app` 44/0; `cargo test --locked --workspace` 3407/0/2.
Anything you do must leave those where they are or better.

## NEXT

**LAND `task/p1-052-android-relay` @ `8dd2cc9e`.** Six commits, all pushed,
`git status --short` prints nothing, every gate below measured on that tip, and
`set-status.py` prepends written and verified (P1-060 23,229 → 27,234;
P1-052 8,080 → 9,172; prior rounds' text intact, yaml re-parsed).

**Gates on `8dd2cc9e`, every number out of a log:**

| gate | result |
| --- | --- |
| `run-device-suite.sh` | **OK (35 tests)** (was 32), TalkBack bound |
| `:core:test` | **496 passed, 0 failed** (was 459) |
| `:app:testDebugUnitTest` | **51 passed, 0 failed** (was 44) |
| `cargo test --locked --workspace` | 3407 passed, 0 failed, 2 ignored (unchanged) |
| `cargo clippy --workspace --all-targets --locked -D warnings` | zero |
| `android/tools/check-help-prose.sh` | 1755 words, TOTAL 0, no allowlist |
| `check-shellcheck-coverage.sh` | 168 discovered, 0 known-failing, 0 newly failing |
| `check-suites-run-in-ci.sh` | 77 suites, 73 run, 4 exempt, 0 unrun-and-undeclared |
| `check-containerfile-assertions.sh` | 194 checked, 0 failed, 0 inert |
| `check-no-conflict-markers.sh` / `check-doc-verbs.sh` | PASS / 0 undocumented-and-undeclared |

**What this does NOT prove, and a lander should say so:** the phone is still on
the same Wi-Fi. It takes the relay because the offer's LAN addresses were
withheld, not because they were unreachable. A phone on mobile data needs a
person to hold it, and that is the only part of the relay leg still unmeasured.

Old NEXT follows.

**Step (d): `--relay` through the suite.** `android/tools/run-device-suite.sh`
`start_remoted()` (line ~140) AND the embedded `broker.py`'s `restart_remoted`
(line ~213) both spawn `apex-remoted` and both need
`--relay wss://apex-relay.andrenijman.com` — the reconnect test goes through
the second, so patching only the first leaves half the suite on a daemon that
holds no rendezvous. Then (e): a device test that forces the relay leg by
building a `PairingOffer` copy with **`lan = emptyList()`** (NOT TEST-NET-1 —
empty is a valid payload per `Pairing.kt` and saves 4s of connect timeout per
bogus address, and `MachineStore.record` then stores an empty `lan` so
`open()` goes to the relay too, with no second trick).

**Before debugging the phone, read `$root/remoted.log`**: `supervise` refuses a
`wss://` endpoint with no root store, so confirm the daemon dialled and is
holding the rendezvous (its `waiting` notice) before believing a 409 from the
phone means anything.

## THE SEAM — measured, and `PairingService.kt:21` is telling the truth

`Client.pair(input: InputStream, output: OutputStream, …)` and
`Client.openSession(input: InputStream, output: OutputStream, …)` — `:core`
takes a stream pair and nothing else. `PairingService` is the TCP half and
supplies `socket.getInputStream()/getOutputStream()`. **So the entire relay leg
is one more implementation of that pair.** Nothing above the transport learns
which leg it is on, exactly as `relay.rs`'s module docs require.

Everything else on the Android side is already written and was waiting for this:

| piece | where | state |
| --- | --- | --- |
| rendezvous id + path | `Rendezvous.kt` | written, tested (`RendezvousTest`) |
| `PairedMachine.relay` | `Storage.kt:80` | written, serialised |
| `PairedMachine.rendezvousId()` | `Storage.kt:84` | written |
| `PairingOffer.relay` | `Pairing.kt` | written |
| `[u32 BE][ciphertext]` framing | `Transport.kt` | written — and it is what the WS carries |
| the affordance | `MachinesScreen.kt:288` | written, unserviceable |

## FOUND — this round, measured (the dispatch's own FOUND list is at the bottom)

- **The deployed relay is live and reachable from the L16.**
  `GET https://apex-relay.andrenijman.com/r/<id>?role=guest` → **HTTP 426**,
  body `this endpoint speaks WebSocket only`, `server: cloudflare`. So "end to
  end over the real deployed relay" is takeable, not hypothetical.
- **A guest never has to wait to be told it is paired.** `relay/src/index.js`
  sets the guest's `peer` attachment *and* the host's inside the same
  synchronous block that returns the 101, so a binary frame written straight
  after the upgrade forwards. A guest that arrives with no host waiting gets
  **409** (`room.js:93`, "no desktop is waiting at this rendezvous"), which
  `Opening.check` already turns into a message carrying the status.
- **`peer-gone` is the relay's word for the far end going away**, and on this
  side it has to become EOF on the carried stream — not an exception with the
  relay's name on it, or the layer above learns which leg it is on.
- **No new dependency is needed and OkHttp is the wrong answer here** — see
  "The dependency decision" below.
- **Four mutations into `Relay.kt`; only two were caught.** `peer-gone` treated
  as a word to ignore, and `MAX_FRAME` halved, both survived — the first
  because the test double had nothing left to send after the notice, the second
  because the parity assertion looked for the literal in the Rust source and
  compared it to nothing. Both gates fixed and re-mutated. **Mutate your own
  gates on this unit; two of eight were blind.**
- **The deployed relay works end to end, measured with an independent client.**
  A hand-rolled Python guest (`scratch-p1-052-android/guest.py`) dialled
  `wss://apex-relay.andrenijman.com` at the rendezvous `apex-remoted` was
  holding: `HTTP/1.1 101`, accept value matches, first frame
  `{"relay":"paired"}`, and a hello byte it wrote crossed the relay and reached
  the daemon (`connection from 127.0.0.1 ended: failed to fill whole buffer` —
  the splice working, then the handshake the probe never finished).
- **`am instrument -e relay ""` is not an empty argument.** `am` prints its
  usage and the run produces NO verdict at all. Omit the flag instead.
- **`SSLSocket` does not verify the hostname by default.** Chain yes, name no,
  unless `sslParameters.endpointIdentificationAlgorithm = "HTTPS"` is set
  before `startHandshake()`. Without it the rendezvous id goes to whoever
  answers the address and every "is TLS on?" test still passes.

## The dependency decision, argued rather than defaulted

**Hand-rolled, no OkHttp.** The reasons are specific to this module, not copied
from the Rust one:

1. `:core` is a **pure JVM module** (`kotlin.jvm`, jvmTarget 17), not an Android
   one. OkHttp in `:core` would be a first HTTP stack in a module whose entire
   dependency list is bouncycastle + kotlinx-serialization, and `:core`'s tests
   run on the JVM where its `Dispatcher` threads would outlive them.
2. **TLS costs nothing without it.** `javax.net.ssl.SSLSocketFactory` is in the
   JDK and on Android, and it is the platform trust store on both — the same
   position `relay.rs` takes ("the caller supplies the stream").
3. **OkHttp's `WebSocket` is callback-and-queue shaped.** It would have to be
   adapted back into a blocking `InputStream`/`OutputStream` pair to reach the
   seam, so the adapter gets written either way — and hand-rolling deletes the
   queue, its unbounded-buffer policy and a 700 kB dependency along with it.
4. It buys one thing this leg cannot use: **automatic HTTP/2, proxies and
   redirects**. A redirect on a rendezvous is a path this client must refuse,
   not follow.

## ORIGINAL PLAN FROM DISPATCH


1. Read `relay.rs` end to end, then `Rendezvous.kt` and `PairingService.kt`, and
   write on this card **what the seam is** before writing Kotlin. If the seam
   turns out not to exist where `PairingService.kt:21` claims it does, that is
   the finding and it changes the round.
2. The WebSocket client half of RFC 6455 that this needs is *one upgrade
   request, masked binary frames out, unmasked frames in, close and ping*. The
   Rust module says it is a few hundred lines with an exhaustively specified
   test vector — **use the same vector on both sides** so the two
   implementations are checked against one thing rather than each other.
3. Wire it behind the existing `offer.relay != null` branch and delete the
   apology string at `PairingService.kt:108` only when it is no longer true.
4. Prove the leg end to end on the phone, over the real deployed relay, and
   check the disclosure `docs/remote.md` requires actually reaches the user.
5. `ROADMAP/set-status.py` REPLACES evidence — no append flag. Read the existing
   text and prepend, then **verify the character count went up**. P1-060 is
   23,229 characters.

## What not to do

- **Do not merge a half client.** The agent who found this gap said a half relay
  client would be worse than none, and they are right: `MachinesScreen` already
  offers the affordance, so a client that dials and fails is a worse user
  experience than one that says it was never written. If the window runs out,
  push the branch, leave the design and the measurements on this card, and land
  NOTHING. That is a good outcome for this round.
- Do not add a WebSocket dependency without saying what it costs. The Rust side
  hand-rolled it deliberately rather than bring a runtime along; Android may
  well have a different right answer (OkHttp is already a common Android
  dependency), but it is a decision to argue on the card, not a default.
- Do not weaken the ciphertext property to make a test easier.

## Rules

- Headless on the L16 — never open a window on Andre's desktop. The phone's own
  screen is fine. Never run `qs -p`.
- No polkit or keyring prompts: `sudo` or `--user`.
- **Never `pkill apex-agentd`.** The suite stands up its own daemons in their own
  XDG root and stops them by pid. Round 2 found and fixed a leak that had left
  seven orphaned `apex-remoted` — kill by pid after reading `/proc/<pid>/exe`,
  never by name.
- Do not touch katana — another agent owns it.
- Never push `main`; never open a PR. Push only `task/p1-052-android-relay`.
- Restore the device baseline at the end and read every value back.
- **Write this card as you go, never at the end.** `NEXT` is load-bearing.

## DONE

| commit | what |
| --- | --- |
| `125e1d91` | `feat(android)` the RFC 6455 client the relay leg has never had — `:core` codec + `RelayLink` stream adapter |
| `f8b6fd7d` | `test(android)` two of the relay gates inspected nothing, found by mutation. **36 tests, 0 failures** |
| `f41667a6` | `feat(android)` dial the relay, and delete the apology at `PairingService:108`. **7 dialler tests, 0 failures** |
| `d0d2f857` | `test(android)` the relay leg from a real phone through the real deployed relay. **OK (3 tests)**; mutated with `APEX_DEVICE_SUITE_RELAY=` → all 3 fail naming the argument |
| `8dd2cc9e` | `docs(relay)` the README said it was not deployed and that no client could dial `wss` — three false claims |
| `b6758c78` | `feat(android)` tell the person which path they got, in the desktop's own words — `ConnectionReport.path`, the banner, `Help.kt`, and a disclosure-parity gate that reads `rendezvous.rs`. **`:core` 496/0 (was 459), `:app` 51/0 (was 44), help-prose TOTAL 0** |

## IN PROGRESS

- Worktree `/var/tmp/apex-work/wt-p1-052-android` on `task/p1-052-android-relay`
  off `roadmap/v2.2` @ `859cbb2b`. Read `relay.rs` (1145 lines), `room.js`,
  `index.js`, `Rendezvous.kt`, `Transport.kt`, `Client.kt`, `Storage.kt`,
  `PairingService.kt`. Seam confirmed (above).
- **Complete.** Six commits, pushed, tree clean.
- Commit 1 of ~6 done and pushed. Sequence, so a cutoff at any point leaves a
  complete artefact: (a) codec **DONE** (b) connector (c) `PairingService`
  fallback + delete the apology string (d) `--relay` through
  `run-device-suite.sh` `start_remoted` AND `broker.py` `restart_remoted`
  (e) device test (f) `Help.kt` + `HelpParityTest` (g) the disclosure UI.
  **(a) (b) (c) (f) (g) DONE and pushed — (f) and (g) were taken early because
  they are the part that makes a client honest, and the card forbids landing a
  half one. (d) and (e) remain: they are the PROOF, not the feature.**

## FOUND — inherited from the dispatch

- (p1-053e, round 33) `TerminalScreen` can never be hosted by
  `createAndroidComposeRule` — its `withFrameNanos` loop keeps the Recomposer
  busy and `setContent`'s own `waitForIdle` never returns.
- (p1-053e, round 33) `MachineLink.request` returns error replies as **strings**;
  the throw lives in the `Agentd.readX` parsers. A test that only caught
  `AgentError` once reported "a phone approved a root operation" when the daemon
  had refused in writing. Assert on the reply, not on the exception type.

## BLOCKED ON

- nothing. The phone was reachable at dispatch.
