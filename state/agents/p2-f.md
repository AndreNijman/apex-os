# p2-f
items: P2-016 (per dispatch only — see NOTE), P2-017, P2-018, P2-019
repo: apex-os (the greeter QML is in apex-os, NOT apex-shell)
worktree: /var/tmp/apex-work/wt-p2-f
branch: task/p2-f-2   (5 commits + 1 merge off roadmap/v2.2 @ e799422b, all PUSHED)

## NEXT
Round 2 closed P2-018's criterion 1. The remaining open work, in the order a
round should take it:

1. **S3/R2 SigV4 signer** (P2-017 criterion 1). The wall P1-011's temporary.rs
   already hit. Rust, in apex-secret-core; needs its own clippy + cargo test
   cycle, so give it a round of its own rather than sharing one with a
   boot-critical change.
2. **RFC 8628 device-code for Google and Microsoft** (P2-017 criterion 1).
   `apex/src/cloudflare.rs` has a complete implementation to copy, including
   its best idea: the refresh token under a SEPARATE service pinned to a
   different host, so the endpoint pin makes it unspendable as an API token.
   Nothing refreshes any token today, Cloudflare's included.
3. **gvfs / file-manager integration** (P2-017 criterion 2). Not started.
4. **Fleet transport and server side** (P2-019). `docs/fleet.md` is a design
   with a "must never be built" list and no daemon; that is deliberate. What is
   missing is enrollment/inventory over a wire.
5. **P2-018 criterion 2 is still STRUCTURAL**: the menu entries are asserted to
   exist AND their binaries are now asserted to be in the image, but none has
   been RUN from inside the session.

## DONE
Round 1: landed as merge 4e8969ef (10 commits).
Round 2 (task/p2-f-2, pushed, NOT merged): P2-018 criterion 1 — the verb.

  c4515644  the watchdog + greeter hooks + 80-assertion suite
            (recovered UNCOMMITTED from the worktree; the killed agent had
            written it and never committed — it was not redone)
  0b4d1e6b  Containerfile.base bakes it and refuses a build that cannot count;
            check-containerfile-assertions.sh taught to resolve directory
            COPYs; thunar/nmtui asserted present in the image
  e4ca169f  CI runs the suite, floored on failures, SKIPS and count drops
  c30defc6  docs/recovery.md: the third route in
  6ff58f9b  §5 — `_selectWanted` extracted and EXECUTED under node

## FOUND
- `files/desktop/apex-greet/GreetContext.qml` lives in **apex-os**.
- `/var/lib/apex-greet` is tmpfiles-created, `0755 greetd greetd`.
- The greeter's launch path is `persistProc` and `Greetd.launch()` is deferred
  to `persistProc.onExited` — the single safest hook point, and also why a
  hanging hook there is a LOCKOUT. Everything added is `timeout`-capped and
  `|| true`'d, and last-user/last-session are written BEFORE the helper runs.
- **`tests/test-apex-greet-layout.sh` already parses both QML files with
  qmllint-qt6.** There is no need to reach for `qs -p` (which is banned) to
  find out whether a greeter edit would boot. Run that suite.
- **`tests/test-apex-greet-sessions.sh` already executes `_selectWanted` under
  node(1).** Its ctx has no recovery fields, so `undefined !== ""` walked the
  new branch and matched nothing — it stayed green without executing the
  change. A suite passing is not the same as a suite covering.
- **check-containerfile-assertions.sh could not resolve a grep against a file
  inside a directory COPY whose destination has no trailing slash.** The
  greeter is copied exactly that way, so every assertion about the LOGIN SCREEN
  sat in the "could not check" bucket. Fixed here: 171 -> 181 checked.
- **`nmtui` is named by no APEX dnf list.** It is inherited from
  fedora-bootc:43, nothing in the image requires it, and it is not a weak
  dependency of NetworkManager. The safe-graphics menu offers it. There is now
  a build assertion; if it ever fires, add NetworkManager-tui to
  Containerfile.core.
- **CI runs as root, and a chmod-based "unwritable directory" test is a no-op
  there.** Make a directory uncreatable by SHAPE (parent is a regular file →
  ENOTDIR for uid 0 too). `unshare --user --map-root-user` reproduces CI's uid
  without sudo and without a prompt.
- Build order is **core → base → apex** (`Containerfile.base` is `FROM
  ${CORE}`), so a base assertion CAN see a core package. Thunar is one.
- `check-doc-verbs.sh` is red on the tip and not because of this unit:
  `apex browser doctor` undocumented, `apex secret list` stale. Identical on
  origin/roadmap/v2.2 — verified, not assumed.

## NOTE — card/dispatch mismatch, unresolved on purpose
The card carries P2-017/018/019; the round-2 dispatch also named **P2-016**.
P2-016's evidence belongs to unit `p2-016-multiuser-2` (landed 2026-09-12 as
merge 5eca2402). This round did no P2-016 work and did NOT call set-status on
it — set-status.py REPLACES evidence, and writing it would have destroyed that
unit's record. Whoever owns the queue should settle which unit holds P2-016.

## BLOCKED ON
(nothing)
