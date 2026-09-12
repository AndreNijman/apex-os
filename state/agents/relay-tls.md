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

Round 1, in progress.

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

### NEXT

`wrangler dev --local` against the real Worker — see below.

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
