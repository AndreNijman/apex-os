# p2-f — online accounts, safe graphics, fleet design

Task: **P2-017**, **P2-018**, **P2-019** (all `todo` at dispatch, 2026-09-12).
First agent on this unit; no prior card.

Repo: apex-os. Worktree `/var/tmp/apex-work/wt-p2-f`, branch `task/p2-f`
off `origin/roadmap/v2.2` @ `4b1e797f`.

## Criteria, quoted from roadmap.yaml

**P2-017 — APEX Online Accounts and brokered cloud identity layer**
1. Architecture supports providers such as Nextcloud, Google, Microsoft,
   WebDAV and S3/R2 without spraying credentials across user config.
2. Apps/services receive scoped capabilities or standard portal/account
   integrations where possible.
3. Account removal revokes local capabilities cleanly.

**P2-018 — Guaranteed Safe Graphics / desktop failover mode**
1. A graphics/compositor/shell failure can enter a conservative recovery
   desktop or safe graphical session.
2. User can inspect failure, roll back deployment/driver, collect
   diagnostics, and reach files/network.
3. Recovery does not depend on the broken normal compositor configuration.

**P2-019 — Design APEX Fleet / managed-device architecture** (DESIGN ITEM)
1. Blueprint/policy/update-ring architecture can scale from one machine to
   managed fleets.
2. Design covers enrollment, inventory, compliance, update rings, managed
   secrets/certificates, app policy, remote health and recovery.
3. Enterprise/fleet support remains optional and does not turn personal APEX
   installs into managed devices by default.

## What this program already knows that bears on it

- **Per-verb authorization is the house pattern now.** `apex-remoted` checked
  the caller only for `Pair`; `Status`, `Devices` and `Revoke` went through
  nothing, and a second account with both filesystem fences opened by hand read
  the owner's machine key, paired-device list, and reached the device store.
  The fix is `apexd/apex-remoted/src/control.rs::authorized`, a `match req`
  over every variant, proven by a table-driven test that names each verb.
  P2-017 is an identity broker — every verb gets the check, proven per verb.
- `apex user` (P2-016, merged `5eca2402`) already owns account creation and
  `docs/multi-user.md` is its surface. An online-accounts store must not be a
  second write path into local account state.
- P0-002 moved credentials out of `$HOME` into
  `/var/lib/apex-secretd/users/<uid>/`, root-owned `0700`, on purpose. Cloud
  tokens belong there, not in a provider's own config file.

## Traps

- P2-018 "desktop failover" means the compositor. Andre is using this machine.
  Headless / nested / fixture-driven only; never take down the live session,
  never `qs -p`, greetd is boot-critical.
- P2-019 is a written architecture. Do not build a fleet daemon.

## Round 1 — P2-017 landed on the branch

`task/p2-f`, pushed, four commits off `origin/roadmap/v2.2` @ `4b1e797f`:

* `dea4a18f` the account model in `apex-secret-core/src/account.rs`
* `e4827864` `apex account` + `docs/online-accounts.md`
* `f748edf0` the `webdav` provider in `apex-secretd`
* `6890cf8d` three end-to-end tests against a loopback WebDAV server

**The decision:** an online account is a `ServiceInfo` in the store
`apex-secretd` already owns, named `account.<provider>.<name>`. No accounts
daemon, no second store, no second write path. Removal is `Request::Remove`,
which already deletes every grant that named the service — criterion 3 was
already built and is now measured.

Five providers over three transports (nextcloud and webdav both route into
`webdav`, because P1-001 routes on an operation id's first segment and two
spellings would drift).

**Found on the way in, worth reusing:** `apex/src/cloudflare.rs` already has a
complete RFC 8628 device-code flow, and it stores the refresh token under a
SEPARATE service pinned to a different host so the endpoint pin makes it
unspendable as an API token. That is the shape P2-017's OAuth half should take.
Nothing refreshes anything, for Cloudflare either.

Also fixed: `tests/test-apex-verbs.sh` had been failing its own reverse pass
59/1 (`lid`, `permissions`, `user`, `vm` unlisted). 64/0 now.

## STATUS

P2-017 done bar the OAuth half. P2-018 next. See `## NEXT`.
