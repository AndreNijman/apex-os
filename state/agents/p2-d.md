# p2-d — secure browser automation capsule

items: P2-008, P2-009, P2-012
repo: apex-os
worktree: /var/tmp/apex-work/wt-p2-d
branch: **task/p2-d-5** (off roadmap/v2.2 @ 7f6fc44d)

> Rounds 1-4 are landed. `task/p2-d-2` and `task/p2-d-4` are both merged — do
> not commit onto either. The durable account of what was built is
> `docs/browser-capsule.md` + `docs/browser-capsule-auth.md`; round 3's long
> archive is `/var/tmp/apex-work/scratch-p2-d/p2-d-card-round3-archive.md`.

> **PROTOCOL_VERSION IS 9** (`RunRequest::allow`, round 4). Route B's TLS
> fields need **10**. Do not reuse 9.

## NEXT

**There is no engineering next step on P2-012. There is a question, and it is
Andre's.** It is written out in full, with both answers costed, under
"The question, for Andre" in `docs/browser-capsule-auth.md`:

> May `apex-agentd` read the plaintext of a capsule's connection to the one
> destination that capsule was pinned to, in order to add a credential the
> capsule is never given?

Round 29 closed the last thing that could be settled without asking. Route B's
one unmeasured assumption — that Firefox accepts a leaf minted for the
requested name at the far end of a `CONNECT` tunnel it opened itself — is now a
three-arm measurement with two controls that fail differently (see DONE). So
everything left is either implementation or that decision, and the decision is
the harder half.

**If the answer is yes**, the build is: a TLS server in `apex-agentd`, per-run
certificate minting (`openssl` is in the image; `rcgen` is not a dependency and
`rustls` 0.23 is in the workspace as a client only), `PROTOCOL_VERSION` 10 with
two gated fields and a CLI that refuses to send them to an older daemon, a
guard in the daemon that the interception is only ever the pinned destination,
and the removal of "a tunnel is opaque" from `docs/browser-capsule.md`. That is
a round of its own, and it should not start before the answer exists.

**If the answer is no**, P2-012's "capability auth" is permanently unmet rather
than pending, and the item should say so. The page explains why there is no
fifth route.

**Smaller and genuinely independent**: the per-run CA bind (gap 5 — automating
an intranet site behind a private CA) closes without any of route B, and round
29's probe is also its evidence: the authenticated arm IS a capsule told to
trust one CA. It is a `protocol.rs` field plus an `apex-agentd` change, because
nothing populates `SandboxSpec.ro`/`ro_at` from the wire. Still listed as
undecided on the page — the flag has to name the file rather than default to
anything.

## DONE (round 29, branch task/p2-d-5, 4 commits, all pushed)

- `12ee0cbf` — **the Containerfile gate could not see a `python3 -c` assertion
  at all.** `tests/check-containerfile-assertions.sh` resolved `grep` and
  nothing else; a python segment missed the regex and hit `continue` — not a
  failure, not UNRESOLVED, not counted, which that file's own header calls the
  worst of the three outcomes. It was doing it to seven of its own assertions.
  Now 193 checked instead of 188, with the two it still cannot follow counted
  (one path from a `for` variable, one pipe).
- `73861e55` — **the `policies.json` shape assertion**, which had been round 3's
  NEXT and round 4's. Positive, not a denylist: `policies` carries `Preferences`
  and nothing else, every preference at `Status: "default"`. 194 checked, 0
  failed.
- `68f3e5c1` — **route B's join, measured**, and the question above written into
  `docs/browser-capsule-auth.md` under its own heading.
- `67861c53` — the runner half of `12ee0cbf`'s own argument was **reasoned, not
  measured**, in a file whose subject is that difference. Measured now: on a box
  with no `/etc/firefox` the unrewritten handler fails six assertions with
  `FileNotFoundError` and exits 1, and the shipped one reports 194 checked / 0
  failed there. Comment only.

## FOUND (round 29)

- **The rewrite in the new python handler is load-bearing, and the surgical
  mutation is what proves it.** `/etc/firefox/policies/policies.json` EXISTS on
  an APEX machine. With the image-path-to-repo-path rewrite removed and the
  repository's own `policies.json` replaced by `{ "policies": }`, the checker
  reports "193 checked, 0 failed" and exits 0 — it parsed the LIVE file. The
  mirror-image half is measured too (`67861c53`): on a box with no
  `/etc/firefox` the same unrewritten handler fails SIX assertions with
  `FileNotFoundError`, while the shipped one is clean there. Four content
  mutations
  (`Certificates.Install`, `Proxy`, a `locked` Status, a bare value) all go red
  with the rewrite in place.
- **The gate now EXECUTES repo scripts, so it has a second environment.** All
  five newly-checked assertions were cross-run under python 3.12 — ubuntu-24.04
  is what both workflows use — in a throwaway container. `tomllib`, both
  `ast.parse` checks and the new shape assertion pass there.
- **A denylist would have been the 706489ec defect again.** The policy file
  carries `//`, `//2` and `//3` comment keys, so `grep '"Certificates"'` as a
  refusal fires the day somebody explains in a comment why there is no
  `Certificates` block. Hence the positive form.
- **Firefox tells Mozilla when an enterprise root is installed.** The probe saw
  it reach for `mitmdetection.services.mozilla.com`. A capsule under
  `--capability` refuses that host because the pin does not contain it — worth
  knowing before somebody runs route B in a capsule that was allowed more.
- **"The route is B" was a claim about shape reading as a decision to build.**
  That is why the card kept saying "next" for something whose load-bearing
  question had never been put to anyone. The page now separates the two.
- Round 4's recorded inconsistency is still there and still unfixed: the
  engine's pre-check at `apex-browser` ~L505 falls back to the bare host, so
  `--allow e.example:8443` passes it when only `e.example` is allowed and the
  DAEMON refuses it. Fail-closed; the two just do not agree.

## BLOCKED ON

- **P2-008's USB passthrough is not closable by any suite.** It means detaching
  a physical device from the machine running the tests. Round 29 changed
  nothing here and ran no vmlab flow.
- **P2-009 needs a guest image that carries an agent CLI.** A build, not a test.
  Round 29 changed nothing here either.
- **P2-012's route B is blocked on a product decision, not on engineering.**
  See NEXT.
