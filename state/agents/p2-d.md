# p2-d — secure browser automation capsule

items: P2-008, P2-009, P2-012
repo: apex-os
worktree: /var/tmp/apex-work/wt-p2-d
branch: **task/p2-d-7** (off roadmap/v2.2 @ bb229745, merged up to 36535383; tip 82b1efbc)

> **PROTOCOL_VERSION IS 11 AS OF ROUND 31.** Route B took it
> (`BROWSER_PRESENT_VERSION = 11`, `RunRequest::present`); gap 5's
> `RunRequest::trust_ca` still holds 10. `BROWSER_CA_VERSION <
> BROWSER_PRESENT_VERSION` is a **compile-time** assert, so a build that
> renumbered them does not link — that is also the strongest of this round's
> ten mutations. The next wire field on this unit takes **12**.

> Rounds 1-6 are landed. `task/p2-d-2`, `-4`, `-5` and `-6` are merged — do not
> commit onto any of them. The durable account of what was built is
> `docs/browser-capsule.md` + `docs/browser-capsule-auth.md`; round 3's long
> archive is `/var/tmp/apex-work/scratch-p2-d/p2-d-card-round3-archive.md`.

## NEXT

**Round 31 is complete and everything is pushed** (`task/p2-d-7`, six commits
`8c3b54bf` → `a932e707`, plus a merge of `roadmap/v2.2` @ `36535383`; tip
`82b1efbc`). **P2-012 is `done`** — its last criterion is closed: a capsule can
present a credential to a site and is never given it.

**No CRITERION on this unit is closable by another round.** The three things
that would close one are named under BLOCKED ON and none of them is a test:

- P2-008's USB passthrough means detaching real hardware.
- P2-009 needs a guest image carrying an agent CLI — a build.
- Route C (a form login, an OAuth redirect chain) needs `geckodriver` in the
  image, which is a product decision about what APEX carries, not a round.

What IS left is two small pieces of engineering, neither of which closes a
criterion and both of which are too small to be a unit on their own:

- **`SessionInfo` carries neither `trust_ca` nor `present`**, so `apex agent
  status` cannot show that a capsule trusts an extra root or that one of its
  destinations is authenticated. Additive optional fields on a stability
  surface; it folds into whichever round next touches `SessionInfo`.
- **`present_pin`'s secret-service half is code-reviewed, not tested.**
  `present_allowlist_is_only` was split out of it precisely so it could be
  exercised without a secret service, and it is; the half that remains — the
  `List` call, `find(|s| s.service == service)`, the port defaulted from the
  scheme — is reached by no test in the tree, because nothing starts the daemon
  against a private `apex-secretd` and then asks it to `Run` with `present`.
  The mutation that proves the gap is `.find(|s| s.service == service)` →
  `.next()`: it would pin a capsule to the FIRST stored credential's host
  whatever was named, and every suite in the repo stays green. The fix is a
  `browser_ca_bind.rs`-shaped integration test (agentd + private secretd +
  `Run { present }`), which is the same fixture the `SessionInfo` item above
  wants in order to assert the new field end to end.

**If this unit is dispatched again, it should be to fold those two in together
and for nothing else** — or not dispatched at all.

## DONE (round 31, branch task/p2-d-7) — six commits and a merge, all pushed

Route B, built because Andre answered the question round 30 refused to answer
for him: **yes**, the runtime may read the plaintext of a capsule's connection
to the one destination it was pinned to, in order to add a credential the
capsule is never given.

- `8c3b54bf` — **`browser.present`, the grant the credential needs.** A
  provider in `apex-secretd` whose one operation both trait methods refuse: it
  exists so `apex secret grant NAME browser.present --everywhere` can be typed,
  because `Service::grant` refuses an operation no provider offers and an
  interception with no grant behind it would let anything running as the user
  spend a credential on arbitrary requests to the pinned host. `--everywhere`
  because a capsule's cwd is a throwaway tree. Two shipped gates fired on it
  and both were EXTENDED rather than loosened — in particular the one that
  panics on an operation binding in NEITHER project, which now carries a named
  list of operations the framework cannot carry at all, checked to refuse
  identically in both projects and to be non-empty.
- `b6964f3a` — **`Request::Present`: the daemon that holds the credential is
  the one that adds it.** The decision is P0-002's and it is the reason the
  design is split: `apex-agentd` runs as the user, so a value it held an
  unconfined session of that user could read, and `broker.rs` states the
  invariant — no verb in `apex_secret_core::protocol` returns a credential.
  **Andre's yes was about PLAINTEXT and was not a decision to give the runtime
  a credential**, so the build was arranged so that second question never had
  to be asked. `apex-secretd` gains rustls as its one TLS client that is not
  `curl`. One request per connection (`Connection: close` forced, chunked
  refused, body by declared length), so the parsing surface in a root process
  is one head. The response direction is scrubbed for the value, same length
  in place. Eight assertions against the real binary, eight mutations red.
- `17f4715e` — **PROTOCOL_VERSION 11, `RunRequest::present`, and the guard.**
  Four refusals at session start (unconfined; not `--network allowlist`;
  `--trust-ca` alongside; an allowlist that is not EXACTLY the pin), exact
  host-and-port equality in the proxy, and the pin re-checked in `apex-secretd`
  because the runtime is not the boundary. ALPN pinned to `http/1.1` — Firefox
  offers h2 through a tunnel and an h2 stream would be carried as frames
  nothing adds a header to. The CA private key is deleted the moment the leaf
  is signed. Ten mutations red, one of them at compile time.
- `b476e6da` — **the demonstration.** Two loopback TLS origins with an
  authority each and a client trusting one root set at a time: the pin verifies
  against the per-run CA and NOT against the origin's own, another allowed
  destination verifies against the origin's own and NOT against the per-run CA,
  and a session that named no credential has nothing terminated. Plus the whole
  chain against a private `apex-secretd` and a site that echoes the header back
  on purpose.
- `6c218bb5` — **`apex browser run --capability NAME --present`.** The flag
  takes no argument: `--capability` already names the credential and has
  already made its pin the only destination, so a second spelling would be a
  second thing that can disagree with the first. 130 passed / 0 failed (was
  116).
- `a932e707` — **the docs.** The question is an answered section with what the
  answer did NOT cover; "What it would take" is "What it took" with the two
  places the estimate was wrong marked; `docs/browser-capsule.md` states the
  bounded version of "a tunnel is opaque".

## FOUND (round 31)

- **`docs/browser-capsule-auth.md` asserted something false and load-bearing:**
  "the daemon already holds the credential". `apex-agentd` does not and must
  not. Corrected at the top of route B rather than silently.
- **A mutation of `apex-secretd` measured GREEN twice.** `cargo test -p
  apex-agentd` does not rebuild another package's binary, so a fixture that
  locates `apex-secretd` beside its own executable runs against whatever was
  last compiled. Both mutations were red the moment it was rebuilt. `cargo test
  --workspace` — what CI runs — does not have the problem; the note is in the
  fixture.
- **A defect in this round's own pump, found by a test and not by reading.**
  Plaintext was drained only inside the socket-readable branch, and rustls
  consumes application data during the handshake — a TLS 1.3 client may send
  its first request in the same flight as its `Finished`. The capsule's first
  request sat in rustls for ever and the capsule waited out its timeout.
- **An assertion that inspected nothing, caught by its own mutation.**
  `trail.contains("http://127.0.0.1")` passed with the `used` audit line
  carrying no endpoint at all, because the `added` and `granted` lines name the
  same host. It parses the trail as JSON now.
- **A flaky test of this round's own**, fixed before it was committed: one
  `complete_io` then `read_exact` passes on an idle machine and answers
  `WouldBlock` on a loaded one.
- **`rustls-native-certs` reads `SSL_CERT_FILE`**, which is what let the
  full-chain test point `apex-secretd` at a CA it minted — the same variable
  `curl` and `git` honour, not a hook asked for by the test. It is also what
  finally measures "re-origination TLS", which the auth doc had listed as
  unmeasured.

## FOUND (earlier rounds, still load-bearing)

- **`/etc/firefox/policies/policies.json` EXISTS on an APEX machine and is read
  inside every capsule** — the sandbox binds `/` read-only. That is what makes
  both gap 5 and route B's CA install possible, and why the round-29 image
  assertion exists.
- **Firefox tells Mozilla when an enterprise root is installed.** A capsule
  under `--capability` refuses that host because the pin does not contain it.
- **`--ro-bind-try` over a path that does not exist is a silent no-op.**
- Round 4's recorded inconsistency is still there and still unfixed: the
  engine's pre-check at `apex-browser` ~L505 falls back to the bare host, so
  `--allow e.example:8443` passes it when only `e.example` is allowed and the
  DAEMON refuses it. Fail-closed; the two just do not agree.

## BLOCKED ON

- **P2-008's USB passthrough is not closable by any suite.** It means detaching
  a physical device from the machine running the tests.
- **P2-009 needs a guest image that carries an agent CLI.** A build, not a test.
- **Route C needs `geckodriver` in the image.** A form login and an OAuth
  redirect chain are what route B cannot do; that is a product decision about
  what APEX carries, not a round of work.
