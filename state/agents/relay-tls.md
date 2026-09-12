# relay-tls — make `apex-remoted` able to dial a `wss://` relay

Unit: `relay-tls`. Closes the first of the two gaps P1-052 named in §6 of
`ROADMAP/design/P1-052-relay.md`: *"No `wss://` from the client. This is the
one that stops 'deployable' from meaning 'deployed and working'."*

Repo: **apex-os only**.
- Branch `task/relay-tls`, forked from `origin/roadmap/v2.2` @ `b79838a4`.
- Worktree `/var/tmp/apex-work/wt-relay-tls`.
- Pushed with `-u` before any work. **No rebase.**

## The decision, already taken

§5.1 of the design doc costed three routes and flagged the choice as Andre's.
He delegated it; the orchestrator chose **option 1 — `rustls` + a root store,
in `apex-remote-core`**. This card does not re-open it. Why the other two lost,
so it is on the record and in the commit message rather than restated as a
menu:

- **Option 3** (custom domain, HTTPS off, `ws://` on port 80) — rejected.
  The design doc is honest that TLS to the relay hides the rendezvous id and
  the traffic pattern from an *on-path observer* — an ISP, a café network —
  which is a different party from the relay operator the disclosure already
  names. This project pins a Sigstore root, refuses an image whose signature
  will not verify, and asserts that its sandbox is a sandbox. Shipping
  cleartext transport for a remote-control rendezvous contradicts all of it.
- **Option 2** (an `openssl s_client` child per connection) — rejected. The
  `curl` precedent it leans on is for **one-shot** requests. A relay
  connection is long-lived and must reconnect across network changes, so every
  reconnect becomes a process lifecycle problem and backpressure runs through a
  child's stdio.
- **Option 1** costs crates in `Cargo.lock` and the WebSocket client is already
  generic over `Read + Write`. That is the trade, taken deliberately.

## The one decision left to this unit: which root store

**Chosen: `rustls-native-certs` — the machine's own store at `/etc/pki`.**
Not `webpki-roots`.

The discriminator is *what is being verified*. APEX pins the Sigstore root
(`files/system/trust/fulcio-root.pem`, fingerprint-checked at build) because
there the thing being verified **is the OS**: you cannot ask the OS store to
vouch for the OS image that carries it. A relay dial is the opposite case — it
happens on an already-booted, already-trusted OS, and at that point the machine
is the authority on who it trusts. Two consequences that decide it:

1. An admin who runs `update-ca-trust` to **remove** a CA expects that removal
   to apply to everything on the box. A vendored bundle would silently keep
   trusting exactly what was removed. That is a security regression relative to
   the OS, dressed up as reproducibility.
2. The realistic private deployment of this relay — the one that answers §6's
   "the relay is unauthenticated" — is behind an internal CA or an enterprise
   proxy. `webpki-roots` cannot reach it at all.

The cost of choosing the machine is that a broken `/etc/pki` becomes a relay
outage. That is met head-on rather than papered over: **an empty system store
is a named refusal, never a fall back to a vendored bundle**, because a
fallback would re-trust precisely the CA the admin deleted. `TlsError::NoRoots`
names `/etc/pki` and `SSL_CERT_FILE` in its message. CLAUDE.md's own record of
`apex-pkg` eating `/etc/pki/nssdb/{cert9.db,key4.db}` is why that error has to
say where to look instead of being a bare "handshake failed".

## Status

Round 2. Complete; nothing in progress.

### Done

- Worktree + branch created off `b79838a4`, pushed with `-u`.
- Orientation: read `ROADMAP/design/P1-052-relay.md`,
  `apexd/apex-remote-core/src/relay.rs` (1145 lines),
  `apexd/apex-remoted/src/relay.rs` (387), `apexd/apex-remoted/tests/relay.rs`
  (1144), `relay/src/{index,room}.js`, `relay/wrangler.jsonc`.
- Checked rather than assumed: **`ring` needs a C compiler at image-build
  time.** `Containerfile.core:1396-1406` (stage 5a-devstack) installs
  `gcc gcc-c++ clang llvm lld make cmake` into the **final** core image and
  nothing removes them afterwards (`FROM` lines are only 78 and 166, and no
  `dnf5 remove` runs after 1398). `Containerfile.base:50` builds apexd
  `FROM ${CORE}`. So the toolchain is there and this is **not** a
  deploy-blocker. Recorded because it would have been a silent one.
- `Cargo.lock` before: **178 packages**, `cc` absent
  (snapshot: scratchpad `Cargo.lock.before`).

### RESULTS — round 1

**Landed: `f4e854f0` on `task/relay-tls`, pushed.**
`feat(remote): dial a wss:// relay with rustls and the machine's root store`

- New `apexd/apex-remote-core/src/tls.rs`: `Trust` (root store + client
  config), `TlsReader` / `TlsWriter` (the split), `TlsError`.
- `apexd/apex-remoted/src/relay.rs`: `DialError::NeedsTls` becomes
  `DialError::Tls(TlsError)`; `Joined`'s halves become boxed trait objects;
  `dial` takes the root store; `supervise` reads the store ONCE at startup and
  refuses to run rather than failing every two seconds for ever.
- New `apexd/apex-remote-core/tests/tls.rs`: 5 tests. Plus 3 unit tests in
  `tls.rs` and 2 new ones in `apex-remoted`'s `relay.rs`.

**Crate cost, measured not estimated: 178 -> 193 = 15 crates.** §5.1 predicted
"roughly 15". Only **9** compile on Linux — `cc`, `openssl-probe`, `ring`,
`rustls`, `rustls-native-certs`, `rustls-pki-types`, `rustls-webpki`, `shlex`,
`untrusted`. The other 6 (`core-foundation{,-sys}`, `find-msvc-tools`,
`schannel`, `security-framework{,-sys}`) are macOS/Windows entries
`rustls-native-certs` carries in its manifest and which never build on this
target.

### What the refusal tests assert

Each by **variant** and by **message**, because a client that accepted any
certificate would also connect and would pass every happy-path assertion:

| test | asserts |
| --- | --- |
| `a_certificate_from_an_untrusted_ca_is_refused_and_the_error_says_so` | `CertificateError::UnknownIssuer`; message contains `UnknownIssuer` |
| `a_certificate_for_another_host_is_refused_and_the_error_says_so` | `NotValidForName{,Context}`; message names **both** `relay.test` and `other.test` |
| `an_expired_certificate_is_refused_and_the_error_says_so` | `Expired{,Context}`; message says `certificate expired` and `not valid after` |
| `a_certificate_that_verifies_carries_a_stream_in_both_directions_at_once` | a write and a read overlap on two threads; a deadlock is a red test, not a hang, because the result is collected through `recv_timeout` |
| `the_name_checked_is_the_relays_name_and_not_the_address_that_was_dialled` | the same loopback destination, accepted as `relay.test` and refused as `127.0.0.1` |

Built against a CA minted per test with the **`openssl` CLI** — an independent
implementation, so "expired" is expired by somebody else's definition — served
from a `rustls` listener bound on loopback before the dial, so there is no port
race and no child process. Nothing reaches the network.

### Mutation proofs — five, all with `Compiling` printed and plain-`cp` restores

Each mutant makes one refusal **accept**; restores verified by md5 against a
pristine copy (`cp`, never `mv`, never `cp -p`).

| mutant | what it does | what went red |
| --- | --- | --- |
| **A** | `handshake` swallows `InvalidCertificate(UnknownIssuer)` and returns Ok | `a_certificate_from_an_untrusted_ca_is_refused_and_the_error_says_so` **only** (4 passed, 1 failed) |
| **B** | same for `NotValidForName{,Context}` | `a_certificate_for_another_host_is_refused_and_the_error_says_so` **and** `the_name_checked_is_the_relays_name_and_not_the_address_that_was_dialled` (3 passed, 2 failed) — the second is correct coupling: its refusal half asks for `127.0.0.1` against a `relay.test` certificate |
| **C** | same for `Expired{,Context}` | `an_expired_certificate_is_refused_and_the_error_says_so` **only** (4 passed, 1 failed) |
| **D** | `TlsError::Refused`'s `Display` drops `{e}`, so it says "refused" and not why | all **three** `_the_error_says_so` tests red, and the two variant-only tests stayed green — which is what makes "the error says so rather than being swallowed" a proven claim and not a description |
| **E** | `dial`'s `if endpoint.secure` becomes `if false`, so `wss://` falls through to plaintext | `a_wss_relay_that_cannot_be_verified_is_refused_and_never_downgraded` **and** `a_wss_relay_with_no_root_store_is_refused_rather_than_dialled_in_the_clear` (2 passed, 2 failed) |

Mutant A's restore checked at `c3ecaa2c4fc0fa8afce803d429edcf44`, the same md5
as the pristine copy, before B ran; the same check after C, D and E. `git
status` clean after the last restore.

### Gates

- `cargo clippy --locked --workspace --all-targets -- -D warnings` — **clean**.
- `tests/in-login-session.sh cargo test --locked` — everything green except
  **one pre-existing flake in a crate this branch does not touch**:
  `apex-secretd`'s `tests::a_stale_socket_from_a_dead_daemon_is_replaced`
  (`apex-secretd/src/main.rs:513`, `assert!(UnixStream::connect(&socket)
  .is_err())`). It passed **180/180 six consecutive times** when run on its
  own; it failed once under the full parallel workspace run. Not caused by this
  branch — nothing here can make a connect to a dead unix socket succeed — and
  recorded rather than fixed or ignored.
- `tests/check-doc-verbs.sh`, `tests/check-no-conflict-markers.sh` — pass.

### The Worker, run for the first time — `0ad66ab2` and `0bb075a7`

`wrangler` is installed as a dev-dependency of `relay/` (**4.131.1**).
`wrangler dev --local` runs the Worker on this machine and **needs no
Cloudflare account**, so §4's gap was a missing dependency and never a missing
permission. Nothing deployed, no account authenticated to, no real relay
dialled.

**Running `relay/src/index.js` for the first time found a defect in it:**

```
Uncaught TypeError: Can't call WebSocket send() after close().
    at webSocketMessage (relay/src/index.js:185)
```

`ctx.getWebSockets()` goes on returning a socket the object has already closed.
Ordinary sequence: a guest leaves → `webSocketClose` → `partnerLost` tells the
host and closes the host's socket → a frame the desktop had already sent
arrives → `webSocketMessage` runs on that closed socket, finds its peer gone,
and calls `send()` on it.

The same staleness had two consequences nobody had seen because nobody had run
the file. A closed-but-listed socket satisfies `waiting()` — which would hold a
room **for ever** and answer that desktop's every later dial with 409, the
exact failure §2 says the design had engineered out — and satisfies
`socketFor`, so a live peer's frames would be copied into a socket nobody
reads. Fixed by checking liveness where occupancy and routing are decided:
`isOpen()` guards `waiting()` and `socketFor()`, and `farewell()` replaces the
two open-coded send-then-close pairs. `relay/test/room.test.mjs` still 9/9 —
this was never a rules bug, which is why the room.js/index.js split could not
catch it.

**Why the Rust suite could never have found it — measured, not argued:**

| Worker | 3 consecutive `cargo test -p apex-remoted --test relay` runs | uncaught exceptions in wrangler's log |
| --- | --- | --- |
| pre-fix | `8 passed; 0 failed` every time | **2** |
| fixed | `8 passed; 0 failed` every time | **0** |

Identical traffic in the fixed window (60 × 101, 33 × 409), so the same paths
ran. A Worker that throws inside a WebSocket event still leaves the socket
closed, which is what the client was waiting for — so the client is *satisfied
by the failure*. Only the Worker's own log sees it.

### §4's last row: **no → yes**

> **the Worker and the Durable Object** | `relay/src/index.js` | **yes — 6 of 8
> suites under `wrangler dev --local`, plus pairing and a session end to end**

Pointed at it with `APEX_RELAY_URL=ws://127.0.0.1:8787`. The default run is
byte-identical: 8 tests, all against the double.

| suite | against the Worker |
| --- | --- |
| `a_device_reaches_the_desktop_through_a_relay_neither_of_them_listens_on` | **ran** — pairing and a live session, end to end |
| `a_relayed_session_is_recorded_as_relayed_and_a_direct_one_is_not` | **ran** |
| `a_guest_that_arrives_with_no_desktop_waiting_is_refused_by_status_not_by_silence` | **ran** — the Worker's own 409 |
| `two_desktops_cannot_hold_one_rendezvous` | **ran** — the Worker's own 409 |
| `the_desktop_measures_the_connection_and_a_device_cannot_invent_the_answer` | **ran** |
| `no_relay_is_configured_unless_the_owner_configures_one` | **ran** |
| `the_relay_carries_the_session_and_can_read_none_of_it` | **skipped**, prints `SKIP-EXTERNAL` — the claim is about what the OPERATOR sees, asserted by reading everything the double copied. A real relay does not hand that over and inferring it from outside would be weaker than the claim. |
| `a_network_change_costs_the_session_and_costs_nothing_else` | **skipped**, prints `SKIP-EXTERNAL` — cuts every socket the relay holds, from inside it. Nothing outside a relay can do that; cutting the client end tests a different event. |

Both skipped suites stay fully covered against the double, which is where they
belong.

`wait_ready` and `hosts_seen` could not survive the switch — both asked the
double what had arrived. In external mode they ask the **relay** instead, by
dialling as a host and expecting **409**: somebody already holds this room.
That is the stronger of the two questions, because it is the Durable Object's
own `waiting()` answering about *live* sockets rather than a tally of
connections that once arrived and may since have died — which is exactly the
bug found above. `seen()` and `cut_everything()` **panic** in external mode
rather than returning an empty `Observed`, because an empty one would let
`assert!(x.is_empty())` pass and turn a skipped claim into one that looked
proved.

`ws://` deliberately: what TLS refuses is proved in apex-remote-core's tls
suite against a minted CA, and pointing this at wrangler's self-signed
certificate would only re-test that refusal.

### What Andre would now run to deploy

Checked rather than copied: `apex-remoted.service` really is a **user** unit
(`Containerfile.base:207` installs it to `/usr/lib/systemd/user/`), so the
design doc's `systemctl --user` is right. `Containerfile.base:332` asserts the
shipped unit's `ExecStart` carries no `--relay`; a `systemctl --user edit`
drop-in does not touch the shipped file, so that assertion still holds.

```
cd relay
npx wrangler login          # opens a browser; grants the CLI the account
npx wrangler deploy         # prints https://apex-remote-relay.<subdomain>.workers.dev
```

`npm install --save-dev wrangler` is **already done and committed** — it is no
longer a step. Then on each machine:

```
systemctl --user edit apex-remoted
#   [Service]
#   ExecStart=
#   ExecStart=/usr/bin/apex-remoted --relay wss://apex-remote-relay.<subdomain>.workers.dev
systemctl --user restart apex-remoted
apex remote status          # "relay" and "rendezvous" both filled in
```

The `wss://` in that ExecStart is the thing this unit made possible. Before it,
that line started a daemon that refused its own configuration.

**Still not done, and not pretended** (§6, unchanged by this unit): the relay
is unauthenticated — anyone who learns a rendezvous id can occupy it. A denial
of service, not a disclosure, because Noise refuses an impostor at either end,
but a public URL wants a token or Cloudflare Access in front of it first. No
rate limiting and no connection cap.

### Round 2 — two holes found by reviewing what round 1 actually proved (`72f1cfca`)

**1. The headline claim had never run.** Every proof was at one layer or the
other: `tests/tls.rs` proved what the connector *refuses* over a raw byte
stream, and the wrangler run proved the room protocol over `ws://`. Nothing had
ever run `dial`'s secure branch to a **success** — the upgrade request written
into a `TlsWriter`, the response read a byte at a time back out of a
`TlsReader`, then real frames through `Sender`/`Receiver` over the split
connection. "apex-remoted can dial a `wss://` relay" was the whole point of the
unit and no test executed it.

`a_wss_relay_is_dialled_verified_and_carries_frames_both_ways` closes it
against a relay that really terminates TLS on loopback, certificate minted per
run by the `openssl` CLI with `subjectAltName = IP:127.0.0.1` — an IP SAN and
not a DNS name, because `dial` checks against `Endpoint.host` and for a
loopback relay URL that host *is* the address. Asserts, in order: the
`{"relay":"waiting"}` notice decoded from an encrypted frame; a keepalive ping
sent **from another thread** while this one is parked in `receiver.message()`,
answered with a pong; and a payload out and the same bytes back.

The middle assertion is the one that matters — it is the overlap
`wait_for_a_device` creates for as long as a desktop waits, and the one that
deadlocks if the lock invariant is wrong.

| mutant | what it does | what happened |
| --- | --- | --- |
| **F** | `dial` verifies against `"relay.example.com"` instead of `endpoint.host` | `a_wss_relay_is_dialled_verified_and_carries_frames_both_ways` **only** (4 passed, 1 failed) |
| **G** | `TlsReader::read` takes the connection lock **before** its blocking socket read — the invariant violated | cargo printed `has been running for over 60 seconds` and the run had to be killed. A deadlock is not a failing assertion, which is exactly why the ping is sent from a thread that cannot be the one parked in the read. |

`rustls` is now a **dev**-dependency of `apex-remoted` so the test can be a TLS
*server*. It was already in the lock through `apex-remote-core`, so this adds
**no crate**: still **193** packages, and the only `Cargo.lock` movement is two
dependency edges.

**2. "the relay's TLS certificate was refused" was a lie on two paths.**
`handshake` mapped *every* `process_new_packets` error to `TlsError::Refused`.
This tree's own anti-downgrade test hits the case where that is wrong: dialling
`wss://` at something speaking plain HTTP yields a record-decode error, not a
certificate complaint, and an operator would go and inspect a CA store over a
server that presented no certificate at all. Tests were never fooled —
`certificate()` correctly returned `None` — but a human reading the daemon log
would have been. `TlsError::Handshake` now carries everything that is not
`rustls::Error::InvalidCertificate`; the anti-downgrade test asserts the
*variant*, so the distinction cannot quietly collapse again.

### Two decisions worth knowing about, stated rather than buried

- **The store path, exactly.** `openssl-probe` on Fedora resolves to
  `/etc/pki/tls/cert.pem` and `/etc/pki/tls/certs`. `TlsError::NoRoots` names
  `/etc/pki/tls/certs`, `$SSL_CERT_FILE` and `$SSL_CERT_DIR`.
- **A partially-readable store still dials.** `Trust::system()` treats
  `load_native_certs().errors` as advisory **when at least one root parsed**.
  The alternative — refuse on any read error — is defensible, and was not
  taken: one unreadable file in `/etc/pki/tls/certs` should not take a machine
  off its relay when the other four hundred roots loaded. Zero roots is still
  a hard, named refusal. If that trade is wrong, it is one `if` in
  `Trust::system`.

### NEXT

**Nothing is in progress. The worktree is clean and all four commits are
pushed.** `task/relay-tls` @ `72f1cfca`, forked from `roadmap/v2.2` @
`b79838a4`, never rebased.

Whoever follows should know:
1. `relay/node_modules/` and `relay/.wrangler/` are gitignored; a fresh
   worktree needs `npm install` inside `relay/` before `wrangler dev --local`
   will run.
2. Deploying is **Andre's decision and his account** — `npx wrangler login`
   is the first step and no agent should take it.
3. The two `SKIP-EXTERNAL` suites are a deliberate boundary, not a gap to
   close. Closing them would mean an observing pass-through proxy between the
   client and the Worker, which would measure the proxy rather than the
   Durable Object.

### Housekeeping

- `relay/.wrangler/` (5.5 MB of miniflare sqlite) was committed by accident in
  `0ad66ab2` and untracked + gitignored in `0bb075a7`. Fixed additively rather
  than by amending a pushed branch.
- Final full-workspace `cargo test --locked`: **everything green**, including
  the `apex-secretd` test that flaked earlier — which confirms that failure was
  load, not this branch.

## The implementation shape, and the trap in it

The design doc's "about 40 lines to wrap the stream" **underestimates**, and
the reason is worth writing down because it is the only hard part.

`apex-remoted::relay::dial` does `socket.try_clone()` twice and hands one fd to
a `Receiver` that blocks in a read while another thread writes through a
`Sender`. Two fds to one socket is fine for TCP. It is **not** fine for TLS: a
`rustls::ClientConnection` is one state machine for both directions and cannot
be cloned, and the obvious `Arc<Mutex<StreamOwned>>` deadlocks the moment the
reader blocks in a read while holding the lock.

The shape that works, and the invariant that makes it work:

> **The reader never holds the connection lock while blocked on socket IO.**

- Handshake with `complete_io` **before** anything is split.
- Read half: lock → `reader().read()`; `WouldBlock` means no plaintext yet →
  **unlock** → blocking `sock.read(raw)` on its own `try_clone` → lock →
  `read_tls(&mut &raw[off..])` (which may not consume the whole slice, so the
  remainder is carried) → `process_new_packets()` → drain any alert
  `write_tls` → retry.
- Write half: lock → `writer().write_all` → drain `write_tls` while
  `wants_write()`. Writing to the socket under the lock is safe precisely
  because the reader never blocks under it.
- `Joined.socket` stays a real `TcpStream` so `shutdown()` still unblocks the
  reader — that is what tears a relay session down today.

## Constraints

- katana is **off-limits entirely**. Headless only, no polkit or keyring
  prompts, never `pkill apex-agentd`.
- Never push `roadmap/v2.2` or `main`; never open a PR; no rebase.
- No AI attribution trailers in commits.
- `tests/run-clippy.sh` is the gate of record.
- **No real relay is dialled, nothing is deployed, and Andre's Cloudflare
  account is never authenticated to.** The refusal tests run against a
  loopback listener with a CA this suite mints — the pattern
  `tests/test-apex-trust-enforcement.sh` already uses against a minted CA
  rather than production sigstore. `wrangler dev --local` needs no account.
