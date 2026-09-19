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

- nothing yet.

## IN PROGRESS

- nothing yet.

## FOUND

- (p1-053e, round 33) `TerminalScreen` can never be hosted by
  `createAndroidComposeRule` — its `withFrameNanos` loop keeps the Recomposer
  busy and `setContent`'s own `waitForIdle` never returns.
- (p1-053e, round 33) `MachineLink.request` returns error replies as **strings**;
  the throw lives in the `Agentd.readX` parsers. A test that only caught
  `AgentError` once reported "a phone approved a root operation" when the daemon
  had refused in writing. Assert on the reply, not on the exception type.

## BLOCKED ON

- nothing. The phone was reachable at dispatch.
