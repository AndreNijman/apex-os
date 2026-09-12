# p2-d — secure browser automation capsule

Task: **P2-012**. (P2-008 and P2-009 are nominally this unit's and were already
landed `partial` as merge `14650ca3`. They were NOT redone. Their remainders —
USB passthrough, a guest image carrying an agent CLI — belong elsewhere.)

Repo: apex-os. Branch `task/p2-d-browser-capsule`, worktree
`/var/tmp/apex-work/wt-p2-d`, from `origin/roadmap/v2.2` @ `4b1e797f`.

## Criterion, quoted from roadmap.yaml

P2-012: "Isolated browser profile/cookies/downloads and capability auth."

Four things. Three are isolation and one is auth, and they are not equally
buildable — see the capability section below, which is why this is `partial`.

## The pivot, and why it is one

The dispatch said P2-012 sits on P2-009's VM egress boundary. It does not, and
the divergence is recorded rather than taken silently. Three measured reasons:

1. **No guest image carries a browser.** P2-009 is itself `partial` because no
   guest image carries an agent CLI; a browser is the same gap one step over.
2. **`apex vm run` refuses `--network` by design** — an outbound interface
   would make its file-egress boundary decorative. A browser with no network is
   not a browser.
3. **The virt stack is deliberately not in the image**, `Containerfile.base`
   asserts its absence, and p2-virt's card forbids installing it on the L16.

What IS extended is the *rule* rather than the VM: the nomination-only egress
loop, default-deny at both ends, the no-clobber rule, the destination fenced
outside the teardown tree, and the four-way fence on the recursive removal.
Each commit names the guarantee it rests on. `docs/virtualization.md`'s "what
is not built" list now records the VM-tier capsule as not built, so the gap is
visible from the page a reader of `apex vm` would be on.

## What was built

* `files/system/libexec/apex-browser` — the engine. It contains **no
  confinement of its own and never invokes `bwrap`**; it asks `apex agent` for
  a confined, allowlisted session, so the masked home and the egress proxy have
  one implementation rather than two. `Containerfile.base` makes that a build
  refusal.
* `apexd/apex/src/browser.rs` — the clap surface, wired as `Cmd::Browser`.
* `tests/test-apex-browser.sh` — 85 assertions against a recording `apex` stub,
  run by CI beside `tests/test-apex-vm.sh`.
* `tests/browserlab/run-browserlab` — the live lab.
* `docs/browser-capsule.md` — the composition table and the not-built list.

`--sandbox project`, not `strict`: strict IS project with the network removed,
and the CLI refuses `--sandbox strict --network allowlist` rather than
pretending they combine. Both mask `$HOME`, `/run` and `$XDG_RUNTIME_DIR`.

## Measured before any code was written

1. **A dev `apex-agentd` built under `/var/tmp` can serve allowlist sessions.**
   `session.rs::bridge_program` refuses a bridge under `/tmp` or `$HOME`
   because a confined session cannot see either, and its comment says
   `/var/tmp` is reachable "which is what makes a live test of this mode
   possible at all". The installed `apex` predates this branch (`apex agent`
   has no `allow` verb there), so the lab uses a dev daemon on a private
   `XDG_RUNTIME_DIR`/`XDG_CONFIG_HOME`/`XDG_STATE_HOME` and never touches the
   live runtime.
2. **Firefox starts headless inside a strict-shaped bwrap and renders** — a
   15 KB PNG with no `DISPLAY`, no `WAYLAND_DISPLAY`, `/run` a tmpfs and
   `--no-remote`. No window appeared.
3. **The fresh `/proc` is load-bearing, found by breaking it.** With `/proc`
   inherited read-only, Firefox's own content sandbox cannot write
   `/proc/self/uid_map`: `EROFS`, every content process dies on `SIGSEGV`, no
   screenshot, and the parent still exits 0 — a silent nothing. The shipped
   sandbox already pushes `--proc /proc`, so the browser's own sandbox nests
   inside APEX's and a capsule has two boundaries. Same defect class as
   p2-virt's virtiofsd finding: a namespace sandbox that cannot nest.
4. **Loopback is reachable through the allowlist only for an ADDRESS rule.**
   `accepts_address` allows a local address when the rule wrote it down and
   denies it (`Denial::LocalAddress`) when the rule wrote a name. That pair is
   the lab's falsifying control for the network flow.
5. **The proxy is CONNECT-only** — absolute-form `GET http://…` is answered 405
   deliberately.

## Defects found, all by running rather than by reading

1. **`--profile` is not rejected by the CLI.** clap's trailing var-arg absorbs
   an unrecognised option into the browser's arguments — the same way
   `apex vm run` absorbs a `--network` — and a second `--profile` WINS on a
   Firefox command line. A capsule would have run against a directory it
   neither created nor deletes. The engine now refuses `--profile`, `-P`,
   `-ProfileManager` and `-display` among the browser's arguments, by name.
2. **Nothing the browser wrote by path could be nominated, and the first
   explanation was wrong.** Downloads lived in `<capsule>/downloads` while the
   session's working directory is the capsule root, so `--screenshot shot.png`
   and the nomination loop were looking in different places. That was fixed —
   the capsule directory is now the download directory, and the profile moved
   to `.profile`, whose leading dot makes it unnominatable by construction
   since a nomination may not start with one — **and the screenshot still did
   not appear.** Measured directly afterwards: Firefox's `--screenshot` writes
   nothing at all when given a relative filename, anywhere, while the run
   still exits 0. An absolute path works, and a caller cannot type the path of
   a capsule this engine names for them, so `{capsule}` in any browser
   argument now expands to the capsule directory. A token rather than a
   rewrite of `--screenshot`, because the engine does not know which of a
   browser's flags take paths.
3. **The browser phoned home and the allowlist was carrying it.** The lab's
   daemon log showed a capsule being denied `firefox.settings.services.
   mozilla.com`, `aus5.mozilla.org` and `location.services.mozilla.com`, over
   and over, for the rest of its run. The allowlist did its job. The profile
   now turns those services off — **and that reduces it rather than stopping
   it**: with every preference applied the daemon still logs occasional
   denials for the settings and update services. Stated in the engine and in
   the docs rather than left to be rediscovered, because the conclusion is the
   one that matters: the allowlist is the boundary and the preferences are
   hygiene in front of it.
4. **Three answers, three times.** The capability lookup, the wait loop and the
   allowlist probe each collapsed "could not ask" into "the answer is no". The
   wait loop's version was the dangerous one: an unanswered `agent status` read
   as "finished" runs the nomination loop over a directory the browser may
   still be writing into, and the mutation proving it turns "nothing is copied
   out on that assumption" red with "report.csv was copied anyway".
5. **The wait loop waited on a state that does not exist.** `killed` is not an
   `AgentState`, so that arm could never fire; `exited` is one and was missing,
   so such a capsule was held until its timeout and then reported as one.
6. **The lab was measuring the wrong thing for ENETUNREACH.** Its unproxied
   probe went to 127.0.0.1 and got ECONNREFUSED against the capsule's own
   loopback — true, and nothing to do with routing. It now probes a TEST-NET-2
   literal and a name, and a new assertion covers what that left uncovered:
   that the BROWSER uses the bridge, proven by a denial in the daemon's log.
7. **`could-not-run` did not beat `verified` in the lab's own driver.** The
   capability flow came out "verified" with the unexercised half demoted to a
   clause in the reason. The vmlab's precedence is restored.
8. **`--profile=DIR` walked around the refusal.** Firefox accepts the `=`
   spelling — measured, it creates and uses a profile there — and the engine's
   refusal listed only the space-separated forms. One character from pointing a
   capsule at a directory it neither created nor deletes, which is the exact
   thing that refusal exists to stop. The two arms were also inconsistent with
   each other: `-display` already carried its `=` form and `--profile` did not.
9. **CI runs suites by name, and nothing invoked this one.**
   `tests/test-apex-browser.sh` would have gated nothing the moment this branch
   landed. It now runs beside `tests/test-apex-vm.sh`, and the lab gained
   `--out`/`--list` so CI reads its bundle the way it reads the vmlab's:
   any flow that came back `failed` or gave no reason fails the job. Simulated
   locally with a missing browser — 6 could-not-run, each naming why, exit 0.

## The live lab's verdicts, last full run on the L16

`tests/browserlab/run-browserlab`, on the machine rather than in a container
(the vmlab needs qemu and libvirt, which are deliberately absent from the
image; a capsule needs bwrap, agentd and a browser, which are all present, so a
container would test a stack that is not the shipped one).

| flow | verdict | what the capsule or the daemon observed |
|---|---|---|
| profile | verified (7) | from inside: `~/.mozilla` absent, the host's profile directory unreachable, no `.mozilla` in the home listing, the capsule's own profile present and pinning the proxy. From outside: no capsule directory survived, and no new file appeared in the host's own Firefox profile |
| headless | verified (3) | no compositor socket in the namespace, the interface list is `lo` and nothing else, and the browser rendered a page and wrote a PNG anyway |
| network | verified (8) | the address the rule wrote down answers; the SAME server by a name is refused (`accepts_address`); the daemon logged that denial itself; unproxied, an address outside loopback connects to nothing and a name cannot be resolved at all; and the BROWSER's own request produced `apex-agentd: session 2 denied browser-only.test:8443` — which nothing but a proxied request could have made |
| downloads | verified (6) | the capsule's own log says it created an unnominated `leak.txt`; `leak.txt` did not reach the host and the nominated file did; **an engine whose copy loop runs over the directory instead of the nominations DOES hand it over**; and without `--download-to` nothing leaves and no destination is invented |
| capability | **could-not-run** | `apex-secretd` is a root service this lab does not start. What ran live is the three-answer refusal: a lookup that could not be made says so, names `apex-secretd`, and does not claim the credential is absent |
| teardown | verified (3) | no capsule directory, no session left running, and the user's own agent runtime socket still there |

**5 verified, 1 could-not-run, 0 failed.** 27 observations.

Each of the four failures along the way was a defect in this unit's own work,
found by the lab and fixed: the relative `--screenshot`, the lab deleting its
own evidence, an assertion expecting a string curl does not print, and the
browser-proxy flow calling the engine wrong.

## Gates

* `tests/test-apex-browser.sh` — **85 passed, 0 failed**. Every new assertion was
  watched going RED against a mutated engine and the engine restored
  byte-identical with plain `cp` (sha256 compared, `git diff` clean).
* `tests/check-containerfile-assertions.sh` — 106 checked / 0 failed / 0 inert
  before, **127 checked / 0 failed / 0 inert** after. Measured both ways
  against `origin/roadmap/v2.2`'s own copy of the file rather than assumed.
* `shellcheck -S warning -x` clean on the engine, the suite and the lab.
* `stop_slop` — `slopcheck.py` over `docs/browser-capsule.md`: 99 hits before,
  **89** after a pass that cut the throat-clearing openers and the adverbs
  doing no work. What remains is em dashes and passives, which are this
  repository's house voice rather than slop: `docs/virtualization.md` scores
  28 per 1000 words, `docs/recovery.md` 33, `docs/agent-runtime.md` 35, and
  this page 34. Measured rather than waved at, because rewriting to 0 would
  have made one page read unlike every other.
* `cargo clippy --locked --workspace --all-targets -- -D warnings` — clean.
* `cargo test --locked --workspace` — **3063 passed, 0 failed**.

## NEXT

**This round is complete and pushed. Do not re-derive any of the above.**
Read `docs/browser-capsule.md` first: it carries the composition table, the
pivot rationale, the capability section that says what is and is not decided,
and a "what is not built" list that is accurate.

If a round 2 is dispatched, these are the gaps, in order of how much they are
worth:

1. **A capsule cannot authenticate to a site.** This is P2-012's unmet
   criterion and the reason it is `partial`. `--capability` binds the
   capsule's destination to a stored credential's pin and the capsule never
   holds the value — but the framework's model is that the DAEMON performs the
   operation, and agentd's proxy tunnels CONNECT, so a TLS tunnel has nowhere
   to put a header. Closing it needs either a driver inside the capsule that
   can be handed a minted token for one request, or a provider that performs a
   login and hands back a session. Both are more than a flag.

2. **The pin-binding half is stub-tested only.** `apex-secretd` is a root
   service and the lab does not start one, so the `capability` flow reports
   `could-not-run` with that reason. What IS exercised live is the refusal and
   its three answers. A round that wants the pin proven live has to decide
   whether starting secretd on the L16 is acceptable; this round decided it
   was not.

3. **Per-run narrowing of the allowlist.** `--allow` is checked against the
   runtime's allowlist and cannot widen it, which is enforced. It also cannot
   NARROW below it: the daemon snapshots `runtime_config.allowlist()` when the
   session starts and there is no per-session allowlist on the wire. Adding one
   is a protocol change and a protocol BUMP, because a field that narrows is a
   restriction and an old daemon ignoring it fails open — `protocol.rs` says so
   about `--network offline` in as many words.

4. **`--console-to` is argv-tested only.** The stub proves the engine asks
   `apex agent logs` and writes the file under the no-clobber rule; no live
   flow reads a real transcript back.

5. **A CA a capsule could be told to trust.** A fresh profile trusts the system
   store and nothing else, so an intranet site behind a private CA cannot be
   automated at all. It is a real flag with a real argument behind it, not an
   oversight.

6. **Chromium.** The sandbox, the allowlist and the nomination loop are
   browser-agnostic; the profile writer is not.

Do **not**: pass a RELATIVE path to `--screenshot` (Firefox writes nothing
anywhere and still exits 0 — use `{capsule}/name`, which the engine expands);
run the lab against the user's own `apex-agentd` (it starts its
own, on a private `XDG_RUNTIME_DIR`, and kills it by pid); put the lab or a
build under `/tmp` or `$HOME` (`session.rs::bridge_program` refuses a bridge
there, and every allowlisted flow would report could-not-run for the wrong
reason); point a capsule's browser at the lab's HTTPS server expecting a page
(the certificate is self-signed and a fresh profile refuses it); or edit
`files/system/libexec/apex-browser` while the lab is running — that cost one
whole run on 2026-09-12.

