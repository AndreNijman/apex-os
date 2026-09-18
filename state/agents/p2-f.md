# p2-f
items: P2-017, P2-018, P2-019  (P2-016 — see NOTE at the bottom, unchanged)
repo: apex-os
worktree: /var/tmp/apex-work/wt-p2-f
branch: task/p2-f-5   (cut from origin/roadmap/v2.2 @ 6b531503, which is round 29's
          task/p2-f-4 landed; nothing of this unit's is unlanded)

## NEXT
**ROUND 30 IS COMPLETE ON THE BRANCH. Both commits pushed on `task/p2-f-5`
(cut from `roadmap/v2.2` @ `6b531503`):**
  `9f8d2468`  the `gdrive` provider
  `bdd9b51f`  `files.read -> gdrive.file.read`, `drive.file` at the sign-in,
              and the three places that said a Google token can be spent on
              nothing
Worktree CLEAN, `git diff --exit-code` silent, every mutated file restored
with plain `cp` and `cmp` silent.

**NEXT ACTION: `msgraph`. Copy `apexd/apex-secretd/src/providers/gdrive.rs`
and its tests wholesale — the shape transfers exactly. Differences, all
known: the account table fixes `microsoft` to `graph.microsoft.com` with path
`/v1.0/me`; Graph's file content endpoint is
`GET /v1.0/me/drive/items/{item-id}/content` (an ITEM id, still
`ResourceKind::Name`), which redirects to a pre-authenticated download URL —
so unlike Drive's `alt=media` this one has to decide what to do about
`location`, and following a redirect that carries the Authorization header is
the thing not to do; and `MICROSOFT_OAUTH` needs `Files.Read` added to its
scopes for the same reason Google needed `drive.file`. Microsoft is a public
client (`ClientSecret::None`), so unlike Google its refresh is expected to
work on the client id alone. Then `GRAPH_SCOPES` gains `files.read ->
msgraph.file.read` and `empty` in both crates becomes `[]` — at which point
the "providers with nothing grantable" assertion needs a deliberate decision
about what an empty set means there, because an empty `empty` is exactly the
vacuous-loop shape this program hunts.**

After that, in descending size: gvfs — **a design paragraph only**, and
P2-019's three remaining unsettled entries, two of which depend on other
roadmap items. P2-019 is a DESIGN item and a design landed; do not build a
fleet daemon. P2-016's `ensure_private_dir` ownership check is the one P2-016
follow-up that needs no screen and no boot.

**P2-018 criterion 2 is CLOSED — do not reopen it, and do not widen the
watchdog.** Round 29 ran every command in `menu.xml` from inside the real
session. What is left on P2-018 needs an image build and a real boot, which
this unit cannot do, plus one thing that needs synthetic pointer input: the
menu is never OPENED (right-click -> ShowMenu -> Execute is not driven). What
IS proved is that every command the menu would run does run, and that labwc
accepts every action the menu names.

## ROUND 4 PLAN (settled with the advisor; do not re-litigate)
1. ~~OAuth vocabulary + tests~~ **DONE, `9e0a8c7d`, pushed.** Tip merged in.
2. **`replaces` is a `Vec`, not an `Option`.** RFC 6749 §6 lets the server issue
   a NEW refresh token, and Wrangler writes one back — so assume Cloudflare
   rotates. Storing only the new access token would leave the old refresh token
   dead at the server and the account broken until `apex cf connect`. So a
   refresh replaces TWO credentials: `cloudflare` (access) and
   `cloudflare-refresh` (the rotated refresh).
3. **`Replaced { name: String, value: SecretValue }` — value-only, on purpose.**
   No host, no scheme, no username: the framework reuses the EXISTING
   `ServiceInfo` and swaps the value, so a refresh structurally cannot repoint
   a credential's pin. That is a property of the type rather than a check.
   `store.put` overwrites in place and leaves grants/approvals alone (verified
   in `store.rs`), so no new store method and no window with no credential.
4. Framework checks in `service.rs`, mirroring the `creates` block: name is
   `valid_service_name`; the name must ALREADY EXIST (a replace of a missing
   name is a `creates` that skipped the free-name check); `!replaces.is_empty()
   && !may_supersede_credentials(op)` refuses; post-perform each returned name
   must be in the declared set; `replaced` values join the `scrub_all` slice —
   a token endpoint's reply IS the secret.
5. `oauth` provider: copy `s3/mod.rs::call`'s curl-config pattern into
   `broker::run_curl`. `api::call` cannot be reused (hardwired `/client/v4` and
   a Bearer header, no form body). Needs a `#[cfg(test)]` table-injection point
   like `CloudflareProvider::at(port)`, because a loopback double is
   `127.0.0.1` and `oauth_for_auth_host("127.0.0.1")` is `None` by design.
6. `same_everywhere: false` on the refresh op — `true` would drag it into the
   two-projects bind fixture, which is MCP-shaped and panics on a bind failure.
7. Check whether `apex cf refresh` can pass the per-project grant check at all,
   or whether `cf connect` must record the grant when it stores the refresh
   token. That answer decides what `cloudflare.rs:681`'s status line says.

Round 2's branch `task/p2-f-2` landed as `f2229185`.

**Round 3's queue changed on the first look, and this is the reason:**
**item 1 of round 2's list — the S3/R2 SigV4 signer — WAS BUILT BY ANOTHER
UNIT and is on the tip already**, as `5054be77` "feat(secretd): an S3
provider, with a SigV4 signer pinned against botocore" (verified
`git merge-base --is-ancestor 5054be77 origin/roadmap/v2.2`). Do not build it
again. What it leaves behind is in FOUND.

Remaining, in the order this round takes them:

1. **NEXT ACTION**: the OAuth vocabulary in `apex-secret-core/src/account.rs`
   (`OAuth { device_url, token_url, auth_host, scopes, client_secret }` + a
   `oauth_for_auth_host()` lookup covering Google, Microsoft AND Cloudflare),
   then `ProviderSpec::may_supersede_credentials` + `Bound::replaces` +
   an `oauth` provider in `apex-secretd` that performs RFC 6749 §6 refresh.
   Prove it on CLOUDFLARE's shape, because Cloudflare is the only one of the
   three with a transport that can spend the refreshed token.
2. ~~The vapour-scope defect~~ **DONE, `c224eea7`.**
3. P2-019 fleet transport + server side.
4. gvfs — a design paragraph only; not enough budget to build it.
5. P2-018 criterion 2 — the exact recipe that would close it.

## DESIGN DECISIONS TAKEN THIS ROUND (so a successor does not re-litigate them)

- **Refresh is `Bound::replaces`, a sibling of `Bound::creates`.** Rejected:
  a new `Request` variant (that is the second write path `account.rs`'s module
  note refuses) and framework-driven refresh at `Use` time (the right product
  end-state, but `ServiceInfo` has no `expires` and no `refresher` field —
  `added` is its only timestamp — so it needs a protocol change to `Add` and
  orchestration in `service.rs`; that is a round of its own, recorded here as
  the follow-on with those field names).
- `service.rs:1053` refuses a `creates` name that is already taken, for a
  reason that is true of creation and false of refresh: the far side issues
  once. A refresh's whole purpose is to supersede. So `replaces` is gated by a
  STATIC flag on `OperationSpec`, enumerated by a test in `providers/mod.rs`
  mirroring `the_everywhere_gate_reads_the_operations_own_declaration`.
- One `oauth` provider serves Google, Microsoft and Cloudflare: RFC 6749 §6 is
  the same request at all three. `bind` maps the refresh credential's pinned
  host to its token endpoint from a hard-coded table, which is pin-consistent.

## DONE
Round 9 (round 30 of the program), on task/p2-f-5, BOTH PUSHED:
  9f8d2468  **the `gdrive` transport** — the thing every card since round 27
            has named as the biggest unblocked piece. One operation,
            `gdrive.file.read`, `ResourceKind::Name` because a Drive file is an
            opaque id and not a path, and the count is Google's arithmetic:
            the limited-input-device grant will not issue `drive` or
            `drive.readonly`, and `drive.file` cannot list a Drive, so there
            is no `gdrive.file.list` rather than one that returns an empty
            listing and reads as an empty Drive. TWO PINS: the URL is the
            stored record's own scheme/host/port/path with
            `/files/<id>?alt=media` under it and the framework pins that, but
            that pin only checks the provider agrees with the store — so
            `bind` ALSO refuses any host but the one `account::PROVIDERS`
            fixes `google` to, read off the table. A non-2xx travels with
            Google's own words; a 401 and only a 401 also names
            `apex account refresh <account>`. 11 tests against a loopback
            double that reads method, target, query and Authorization off the
            wire, and which answers a METADATA document with a 200 when
            `alt=media` is missing — which is what a real Drive does and what
            makes the dropped-`alt=media` mutant red instead of green.
            8 mutations, all red.
  bdd9b51f  `DRIVE_SCOPES` gains `files.read`, and with it the positive
            assertion round 3 had to DELETE. `GOOGLE_OAUTH.scopes` gains
            `https://www.googleapis.com/auth/drive.file` — the line between a
            grant and a 403, deliberately absent while nothing could spend it.
            `apex account add`'s closing status line now READS the provider's
            table instead of being a fixed sentence about a missing transport.
            A stale number found while editing around it:
            `docs/online-accounts.md` said "three transports" while the table
            held four, stale since `5b153599`; corrected and now computed in
            `the_criterions_five_providers_are_all_present_and_unique`.
            8 mutations, all red — including the two that only show up under
            `-p apex-secretd`, because `cargo test --workspace` stops at the
            first failing target and the core-crate failure was hiding the
            cross-crate gate.
  Workspace 3297 -> 3298 passed / 0 failed / 2 ignored. clippy --locked
  --workspace --all-targets -D warnings exit 0. secret-broker 76/0;
  apex-verbs 65/0; doc-verbs 191 / 114 / 0 / 0; shellcheck 165 / 0 / 0;
  suites-in-CI 75 / 71 / 4 / 0; containerfile 194 / 0; no conflict markers.
  **NOT RUN AND NOT CLAIMED: no Google endpoint was contacted.** Whether
  Google accepts `drive.file` on a real limited-input-device client, and
  whether its refresh works without the client secret this build has nowhere
  to store, are both still unproven — card item 2 from round 29 is UNCHANGED.

Round 8 (round 29 of the program), on task/p2-f-4, ALL THREE PUSHED:
  79027b51  which `apex` the remedies ran was ANNOUNCED, not measured:
            `APEX_BIN=/nowhere` planted a dangling symlink, `command -v`
            stepped over it, the remedies used the machine's own copy, and the
            suite printed the path it had been handed. Now refused and named,
            and the resolution line asks the session. Also probed the one CI
            unknown in the error-channel assertion: re-run with a PATH of 2,777
            symlinks minus `Xwayland`, still 86/0/0 with an empty startup log,
            because wlroots starts Xwayland lazily.
  fb884d8f  the recovery session logged `[ERROR] Empty string is not allowed
            for name.theme` on EVERY start — `<theme><name></name></theme>` in
            the shipped rc.xml is not "labwc's built-in theme", omitting the
            element is. Found by RUNNING the session, which is the point of
            the round. The suite now requires the shipped configuration to put
            nothing on the compositor's error channel, snapshotted at startup,
            with a control: a menu naming `ApexNoSuchActionExists` must make
            labwc complain, and it does. That control is what turns menu.xml
            from "well-formed XML" into "actions labwc will perform" —
            xmllint and labwc do not agree automatically.
  653a6d5d  P2-018 criterion 2: every command in menu.xml is read out of the
            file and RUN from inside the session, under `env -i` with the
            environment the compositor handed a client its own autostart
            started. Wrapped entries go through `apex-safe-graphics-terminal`,
            which had no live coverage at all. `sudo apex rollback` is never
            run (its verb is asked for `--help`, with sudo/pkexec tripwires on
            the session's PATH); `nmtui` with no terminal exits ZERO having
            drawn nothing, which is now an assertion and is why the menu wraps
            it; labwc's `Exit` is not a command. `thunar` runs as a real
            client with a negative control every time and a gate that refuses
            to start it unless the client environment carries a DISPLAY that
            is not the developer's and NO session bus address. Also fixed five
            `printf | grep -q` sites — 141 on a MATCH under pipefail, one of
            them the negative quickshell assertion.
  59 -> 86 passed, 0 failed, 0 skipped. Fourteen mutations, all red, all
  restored byte-identical with plain cp. Gates: shellcheck coverage 165 / 0
  known-failing / 0 newly failing; suites-in-CI 75 / 71 / 4 exempt (no fifth).
  P2-018's evidence was rewritten in full with set-status.py, rounds 1-28
  preserved verbatim and checked by re-parsing.

Round 7 (round 28 of the program), on task/p2-f-3, ALL SIX PUSHED:
  8f60dd5e  removing an account that never had a refresh token is not a
            failure — the commoner path the commit below left untested, which
            rested on the daemon answering `NoSuchService` for a `Remove` of a
            missing service. It does; now asserted end to end. Plus the
            transport correction: `client_secret` goes on the poll only, which
            is the only place either provider documents one.
  5e742f22  P2-019's transport and server-side design, the two gaps its own
            evidence said kept it partial. The transport is a POLL and
            `relay/` is not in the fleet path — a relay carrying fleet traffic
            learns which machines are awake, which is the liveness map
            `docs/update-channels.md` promises not to collect. A response is a
            signed DOCUMENT, never a command. Tenancy is key separation, not
            rows; read is a server permission and write is a cryptographic
            one. `apex fleet report` added to `tests/doc-verbs-allow` and the
            entry mutation-checked (removed, the gate exits 1).
  bda510ed  `apex account rm` leaves no refresh token behind. The commit
            before it made `add` store one for the first time, which turned a
            dormant doc-comment claim into a criterion-3 failure: `rm` sent
            ONE `Remove`, and the refresh credential cannot be reached any
            other way (`.` is illegal in an account name, so it does not parse
            as an account — `list` never shows it, `rm` refuses to be pointed
            at it). `names_to_remove` + an end-to-end section in
            tests/test-secret-broker.sh, 66 -> 73 passed.
  5b153599  `apex account add --client-id` signs Google and Microsoft in by
            device code, files the refresh token pinned to the auth host WITH
            THE CLIENT ID IN THE USERNAME FIELD — which is what makes the
            daemon's oauth provider able to renew them WITH NO DAEMON CHANGE,
            exactly as its module note predicted — and `apex account refresh`
            spends it. `--client-secret` is now in the forbidden-flag gate;
            it did not match `--secret`.
  202ee465  the device grant moved to `apexd/apex/src/oauth_device.rs`, taking
            client, scopes and wording as arguments. Three interop defects no
            loopback double could catch: `verification_url` (Google's
            spelling), no `client_secret` slot on the poll, and a 4096 token
            cap a Microsoft JWT exceeds.
  eeaf08ac  `apex cf status` asks whether this project may actually renew,
            through `Grants::allows` over `Request::Grants` rather than a
            second copy of the daemon's rule.
  Fifteen mutations run, all red, all restored byte-identical with plain cp,
  plus one on `tests/doc-verbs-allow`'s new entry (removed, the gate exits 1).
  tests/test-secret-broker.sh 66 -> 76 passed; tests/test-apex-verbs.sh 65/0
  including its reverse pass.
  Workspace 3267 passed / 0 failed / 2 ignored; clippy exit 0; doc-verbs 191
  documented / 0 undocumented and undeclared; shellcheck coverage 165 / 0
  newly failing; suite coverage 75 / 71 in CI / 0 unrun and undeclared.
  P2-017's evidence was updated with set-status.py (full text rewritten,
  round-1 text preserved).

Round 6 (round 27 of the program), on task/p2-f-3 (pushed):
  33483c37  the `oauth` provider — RFC 6749 §6 against the server that
            issued the token — registered in `default_registry`,
            `oauth.token.refresh` added to the vocabulary test, and the
            superseding set changed from EMPTY to exactly `["oauth.token
            .refresh"]`. 11 provider tests against a loopback double that
            parses the form body that actually arrived.
            Full suite on the branch: 3235 passed / 0 failed / 2 ignored.
            `clippy --workspace --all-targets -- -D warnings` exit 0.
  0a8783e3  `account::renewed_service` + `REFRESH_SUFFIXES` — the inverse
            of `AccountRef::refresh_service`, widened to `-refresh`.
Round 5, on task/p2-f-3 (pushed):
  306e79e1  `Bound::replaces` / `Performed::replaced` / `Replaced { name,
            value }`, the five framework checks in `use_capability`,
            `Service::swap`. Five mutations run and red. 3225 / 0 / 2.
  862d1c37  `OperationSpec::supersedes_credentials`, mandatory, `false` at all
            64 literals; `ProviderSpec::validate` refuses a superseding
            operation that calls itself `Effect::Read`. Both new tests
            mutation-checked red. 3220 passed / 0 failed / 2 ignored.
Round 1: landed as merge 4e8969ef (10 commits).
Round 2: landed as merge f2229185 — P2-018 criterion 1, the recovery verb.
Round 3, on task/p2-f-3 (pushed):
  c224eea7  a scope may not name an operation no provider offers — the
            cross-crate gate in apex-secretd, both mutants run and red.

## IN PROGRESS (round 9 — FINISHED)
- Worktree CLEAN at `bdd9b51f` on `task/p2-f-5`, pushed, cut from
  `roadmap/v2.2` @ `6b531503`. Nothing half-written; `git diff --exit-code` is
  silent and every mutated file was restored with plain `cp`, `cmp` silent.
  Pristine copies of the mutated files are in
  `/var/tmp/apex-work/scratch-p2-f/round30/*.orig` if anything needs checking.
- set-status.py was called for **P2-017 only**, with the full stored text kept
  verbatim and round 30 appended, then re-parsed to confirm rounds 1-29 are
  still there.
- P2-018 and P2-019 were NOT touched this round and set-status was
  deliberately NOT called on them — it REPLACES evidence.

## ROUND 5 COMMIT SEQUENCE (settled with the advisor)
1. ~~`supersedes_credentials` + the 64 `false` literals + the registry test~~
   **DONE, `862d1c37`, pushed.** The superseding set is asserted EMPTY there;
   commit 3 changes that line to `["oauth.token.refresh"]`. The gate function
   `may_supersede_credentials` moved to commit 2 — CI runs
   `clippy --all-targets -- -D warnings` and an unused `pub(crate) fn` is a
   dead-code warning, so it lands with its caller.
2. ~~`Bound::replaces` / `Performed::replaced` / `Replaced` + the framework
   checks + the `Replacer` test provider~~ **DONE, `306e79e1`, pushed.**
3. ~~the `oauth` provider + a loopback double~~ **DONE, `33483c37`, pushed.**
4. `apex cf refresh` + the `cloudflare.rs` status line + the step-7 answer.

## FOUND
- **xmllint and labwc do not agree about what a configuration is.** Both the
  shipped rc.xml (an empty `<theme><name>`) and a menu `<action>` whose name
  labwc does not have are WELL-FORMED XML that labwc rejects with an `[ERROR]`
  line and then carries on with a default — up, apparently fine, not doing what
  the file says. Anything in this repo that validates a labwc file with
  `xmllint --noout` alone is checking the weaker of the two things. Start the
  compositor and read its error channel, with a control so a clean log is a
  measurement rather than a log nobody writes to.
- **`5054be77` landed the S3 provider and SigV4 signer.** Round 2's card
  listed it as this round's item 1; it is done, and by somebody else.
- **VAPOUR SCOPES — a real defect the S3 landing exposed.**
  `apex-secret-core/src/account.rs` lets a user run `apex account grant
  google files.read`, which records a grant for operation `gdrive.file.read`.
  **No provider in `default_registry()` offers `gdrive.*` or `msgraph.*`, and
  `S3_SCOPES` names `s3.object.list` which the new S3 provider does not offer
  either** (it has `s3.object.read` and `s3.object.write`). Nothing checks
  this today because `apex-secret-core` cannot see the registry — but
  `apex-secretd` depends on `apex-secret-core`, so a test THERE can, and that
  is where the gate belongs.
- ~~Nothing in this build refreshes any token~~ **DONE.** Cloudflare renews
  (round 27), Google and Microsoft sign in and renew (round 28), and the
  status line was corrected in `eeaf08ac`.
- ~~**A token stored for Google or Microsoft can be spent on NOTHING**~~
  **HALF DONE, round 30.** Google can now be spent: `gdrive.file.read` exists
  (`9f8d2468`) and `files.read` names it (`bdd9b51f`). **Microsoft still
  cannot** — no `msgraph` transport, `GRAPH_SCOPES` still empty, and
  `apex account add` still says so for that provider. What a Google token
  reaches is narrower than "your Drive" and the narrowness is Google's:
  `drive.file` sees only files this OAuth client created or the user picked, so
  on a Drive APEX has never written to **every file id answers 404** — the
  scope working, not the transport failing. `files.write` is what makes the
  first file readable and it needs `/upload/drive/v3/files`, a different path
  from the stored `/drive/v3`.
- **Google's refresh is NOT proven, and round 30 did not change that.** Its
  guide lists `client_secret` as required on the poll and optional on the
  refresh; this build stores no secret. Rather than hardcode a refusal off a
  doc reading that cannot be checked from here, the daemon lets Google
  answer — a non-2xx replaces nothing and returns Google's own
  `error_description`. NOT RUN THIS ROUND AND NOT CLAIMED: no Google endpoint
  has ever been contacted from this repository, so whether Google accepts
  `drive.file` on a real limited-input-device client is also unproven.

## BLOCKED ON
(nothing)

## FLAKE, not a regression — do not chase it
`apex-secretd`'s `tests::a_stale_socket_from_a_dead_daemon_is_replaced`
failed once in a full `--workspace` run and passed 3/3 when run alone. It
is the known apex-os "suites interfere in a sequential loop" family; the
socket staleness probe races another suite's `/tmp` socket. Re-run it
alone before believing it.

## NOTE — card/dispatch mismatch, unresolved on purpose
Round 2's dispatch also named **P2-016**, whose evidence belongs to unit
`p2-016-multiuser-2` (landed 2026-09-12 as merge 5eca2402). Rounds 2 and 3 did
no P2-016 work and did NOT call set-status on it — set-status.py REPLACES
evidence, and writing it would have destroyed that unit's record. Whoever owns
the queue should settle which unit holds P2-016.
