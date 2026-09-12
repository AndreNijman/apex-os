# p2-d — secure browser automation capsule

Task: **P2-012**. (P2-008 and P2-009 are nominally this unit's and are already
landed `partial` as merge `14650ca3`. They were NOT redone. Their remainders —
USB passthrough, a guest image carrying an agent CLI — belong elsewhere.)

Repo: apex-os. Branch `task/p2-d-browser-capsule`, worktree
`/var/tmp/apex-work/wt-p2-d`, from `origin/roadmap/v2.2` @ `4b1e797f`.

## Criterion, quoted from roadmap.yaml

P2-012: "Isolated browser profile/cookies/downloads and capability auth."

Four things. Three are isolation and one is auth, and they are not equally
buildable — see the pivot and the capability note below.

## The pivot, and why it is one

The dispatch said P2-012 sits on P2-009's VM egress boundary. It does not, and
the divergence is recorded rather than taken silently. Three measured reasons:

1. **No guest image carries a browser.** P2-009 is itself `partial` because no
   guest image carries an agent CLI; a browser is the same gap one step over.
2. **`apex vm run` refuses `--network` by design** — an outbound interface
   would make its file-egress boundary decorative. A browser with no network is
   not a browser, so this is a different design and not that verb plus a flag.
3. **The virt stack is deliberately not in the image** and `Containerfile.base`
   asserts its absence; p2-virt's card additionally forbids installing it on
   the L16.

What IS extended is the *rule* rather than the VM: the nomination-only egress
loop, default-deny at both ends, the no-clobber rule, the destination fenced
outside the teardown tree, and the four-way fence on the recursive removal.
Each commit names the guarantee it rests on.

A VM-tier browser capsule is recorded under "what is not built".

## The substrate, all of it already in the image

* `apex-agent-core/src/sandbox.rs` — bwrap: `$HOME` a tmpfs, `/run` and
  `$XDG_RUNTIME_DIR` tmpfs, `--proc /proc`, `--unshare-net`.
* `apex-agentd/src/egress.rs` — `NetworkPolicy::Allowlist`: an in-namespace
  bridge on `127.0.0.1:3128` over AF_UNIX to the daemon, which asks
  `destination::Allowlist::decide` and `accepts_address` and connects to the
  address it checked. CONNECT only.
* `files/system/libexec/apex-vm` `cmd_run` — the egress loop and the fence.
* `apex-secretd` — the capability framework (P1-001), host pin included.

`bwrap`, `apex-agentd` and `/usr/bin/firefox` are all on a stock machine.

## Measured before any code was written

1. **A dev `apex-agentd` built under `/var/tmp` can serve allowlist sessions.**
   `session.rs::bridge_program` refuses a bridge under `/tmp` or `$HOME`
   because a confined session cannot see either, and its comment says
   `/var/tmp` is reachable "which is what makes a live test of this mode
   possible at all". The worktree is under `/var/tmp`. The installed `apex`
   predates this branch (`apex agent` has no `allow` verb), so the lab uses a
   dev daemon on a private `XDG_RUNTIME_DIR`/`XDG_CONFIG_HOME`/`XDG_STATE_HOME`
   and never touches Andre's live runtime.
2. **Firefox starts headless inside a strict-shaped bwrap and renders.**
   `--screenshot` produced a 15 KB PNG with no `DISPLAY`, no
   `WAYLAND_DISPLAY`, `/run` a tmpfs and `--no-remote`. No window appeared.
3. **The fresh `/proc` is load-bearing, and this was found by breaking it.**
   With `/proc` inherited read-only (`--ro-bind / /` and no `--proc /proc`),
   Firefox's own content sandbox cannot write `/proc/self/uid_map`: `EROFS`,
   every content process dies on `SIGSEGV`, no screenshot is produced, and the
   parent still exits 0 — a silent nothing. The shipped sandbox already pushes
   `--proc /proc`, so the browser's own sandbox nests inside APEX's and the
   capsule has two boundaries. Same defect class as p2-virt's virtiofsd finding:
   a namespace sandbox that cannot nest.
4. **A loopback test server is reachable through the allowlist, and only as an
   address rule.** `accepts_address` allows a local address when the rule wrote
   that address down and denies it (`Denial::LocalAddress`) when the rule wrote
   a name. That pair is the lab's falsifying control for the network half.
5. **The proxy is CONNECT-only** — absolute-form `GET http://…` is answered 405
   deliberately. So the lab's server is HTTPS and the capsule profile has to
   trust its CA.

## Shape

* `files/system/libexec/apex-browser` — the engine. Bash, like `apex-vm` and
  `apex-disposable`. It shells out to `apex agent`, **never to bwrap**: the
  confinement and the proxy stay in one implementation.
* `apexd/apex/src/browser.rs` — the clap surface.
* `tests/test-apex-browser.sh` — argv and profile assertions against stubs.
* `tests/browserlab/run-browserlab` — the live lab, three-state verdicts.
* `docs/browser-capsule.md` — written first, and it carries the not-built list.

## NEXT

(in progress — see the report below when this round closes)
