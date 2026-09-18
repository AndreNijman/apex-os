# p2-d — secure browser automation capsule

items: P2-008, P2-009, P2-012
repo: apex-os
worktree: /var/tmp/apex-work/wt-p2-d
branch: **task/p2-d-4** (off roadmap/v2.2 @ 266dcc57, merged in)

> **THE DISPATCH BRIEF FOR ROUND 4 WAS STALE ON TWO POINTS.** It said the
> worktree sat on `roadmap/v2.2` and that work should go to `task/p2-d-2`.
> Neither was true: the worktree was already on `task/p2-d-4` (local, off
> `roadmap/v2.2` @ 654fa854), and `task/p2-d-2` is LANDED — it is in P2-012's
> own evidence as round 24. Committing there would have been committing onto a
> merged branch. Round 4 is on `task/p2-d-4`, pushed.

> **PROTOCOL 8 -> 9 IS DONE AND LANDED ON THE BRANCH** (`c88327d9`), for
> `RunRequest::allow`. `SESSION_ALLOWLIST_VERSION = 9` beside it. Do not bump
> again for the same field; route B's TLS fields need **10**, and
> `docs/browser-capsule-auth.md` now says so.

Rounds 1-3 are landed (`14650ca3`, `13d53c01`, `be9844cc`); their long account
is `/var/tmp/apex-work/scratch-p2-d/p2-d-card-round3-archive.md`, and the
durable version is `docs/browser-capsule.md` + `docs/browser-capsule-auth.md`.
Read those, not this, for what was built.

## NEXT
The `policies.json` shape assertion in `Containerfile.base` (beside line ~501)
— round 3's own NEXT, still not done, and it is the only item on this card that
is neither blocked nor finished. See FOUND. Read
`tests/check-containerfile-assertions.sh` first and shape the assertion to what
it requires: five unpassable Containerfile assertions once cost five days of
image builds, and an image build cannot be run from here to check.

After that, P2-012's remaining work is route B (`docs/browser-capsule-auth.md`),
which is a TLS server in `apex-agentd` plus protocol 10 — a round of its own,
not a follow-up.

## DONE (round 4, branch task/p2-d-4, 8 commits, all pushed)
The previous agent's 365 uncommitted lines were **finished, not redone**: they
were a correct, incomplete implementation of per-session allowlist narrowing,
four compile errors from green. What round 4 added is the CLI half, the daemon
error type, the version gate, the engine wiring, and every assertion.

- `c88327d9` — `RunRequest::allow`, `Allowlist::narrow`, `Rule::covered_by`,
  `PROTOCOL_VERSION` 8 -> 9, `SESSION_ALLOWLIST_VERSION`, `AllowlistRefused`,
  `apex agent run --allow`, and the CLI's refusal to send `--allow` to a daemon
  below 9.
- `8821e77e` — `apex browser` passes its destinations to the session, so
  `--capability`'s pin is enforced by the egress proxy instead of announced by
  a shell script. `session_allowlist` split out of `session::start` so the rule
  is testable without a daemon. `SessionInfo.allowlist`, printed by
  `apex agent status <id>` as `destinations`.
- `4b8c461f`, `a58bfe08` — `apexd/apex-agentd/tests/session_allowlist.rs`
  against a real daemon on its own socket, including a `CONNECT` through the
  session's own egress proxy.
- `62a3854e` — docs.
- `d5466e8f` — the browserlab's `narrowing` flow, verified live (6 observations),
  and a fix to this round's own harness: it orphaned a `bwrap` namespace for 23
  minutes, because `pty::spawn` calls `setsid` so `SessionInfo.pid` is the
  process GROUP leader and `bwrap` is a separate process inside it.

Full browserlab on the L16 afterwards: **7 verified, 1 could-not-run, 0
failed**. The could-not-run is `authentication`, which IS P2-012's unmet
criterion and is reported as such rather than skipped.

## FOUND (round 4)
- **A test that inspected nothing, caught by mutation rather than by review.**
  `SessionInfo.allowlist` was added with four assertions around it, and setting
  it to `None` outright broke none of them. Then the fix's own commit message
  claimed the new file caught a proxy given the wrong list — and the SURGICAL
  mutation (change only the `egress::start` call site, leave the binding alone)
  passed all four. Both are now covered, the second by asking the proxy.
  The lesson for whoever picks this up: on this unit, the mutation that
  matters is the smallest one, not the one that is easy to write.
- **`android/core/src/test/resources/requests.json` was deliberately NOT
  extended**, against the dispatch brief. `allow` is a FIELD on an existing
  request, not a new request; `Sessions.kt`'s `run` builder sends no policy of
  any kind, so the phone cannot compose a narrowed session at all, and a Kotlin
  builder for it would be a builder for a request the app has no way to make.
  The compatibility contract that DOES matter — the wire key is `allow`, an
  absent key is `None`, an explicit null is `None` — is asserted in
  `protocol.rs` instead, and shown to fail under a `rename`.
- Round 3's own NEXT (the `policies.json` shape assertion) is still absent.
  Round 3 measured that `/etc/firefox/policies/policies.json` is the ONLY
  policy file any Firefox on the machine reads — capsule or not — so an
  unasserted shape is every capsule's trust surface.
- **A small inconsistency left in place, recorded rather than fixed.** The
  engine's pre-check at `apex-browser` ~L505 falls back to the bare host, so
  `--allow e.example:8443` passes it when only `e.example` (= 443) is allowed,
  and the DAEMON then refuses it. Fail-closed, and the daemon's message is the
  better one — but the two do not agree, so the pre-check's "the refusal is
  better before a capsule exists" is true only when they do.

## BLOCKED ON
- **P2-008's USB passthrough is not closable by any suite.** It means detaching
  a physical device from the machine running the tests. It needs a manual run
  on a machine with a spare USB device somebody is willing to have detached.
  Nothing in round 4 changes this and nothing should pretend to.
- **P2-009 needs a guest image that carries an agent CLI.** What is verified is
  the BOUNDARY — read-only copy-in, nomination-only egress, no network,
  teardown — with an arbitrary command as the task. Building a guest that
  carries `claude` or `codex` is separate work, and `docs/virtualization.md`
  already says so.
- **P2-012's route B is decided and scoped, not built.** It is a TLS server in
  `apex-agentd` and protocol 10. Round 4 closed a fail-open in the destination
  pin; it did not make a capsule able to authenticate to a site.
