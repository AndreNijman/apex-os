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

## Round 1 — P2-018 landed on the branch

* `45f5c0ef` APEX Safe Graphics: the session, the terminal helper, the baked
  config directory, the session entry, `tests/test-apex-safe-graphics.sh`
* `33e88ab4` the Containerfile stage, the two greet-session counts, and the
  `docs/recovery.md` section

The load-bearing thing is `labwc -C /usr/share/apex/safe-graphics`. A config
DIRECTORY, so the compositor never opens `~/.config/labwc` — the file
`apex-input-apply` splices into and `apex-labwc-keybinds` generates into, and
the file labwc falls back from SILENTLY when it will not parse. `-C` and not
`XDG_CONFIG_HOME`, because the clients must keep their own config.

`WLR_BACKENDS` is the variable deliberately NOT set: unset it is DRM for a
person and `headless` for a test, which is the only way this gets exercised
without taking Andre's display away.

Two routes shipped (greeter entry, virtual console) and the third named as
refused: no recovery BOOT entry, because a shipped helper may not write the ESP
and `tests/test-boot-v2.sh` scans for exactly that.

## Round 1 — P2-019 landed on the branch

* `9779e443` `docs/fleet.md`. A design, marked built / partly built / not built
  per section, with a "must never be built" list and a "does not settle" list.

## Round 1 — P2-019 gaps that matter

The design does not settle the transport and has no server-side design at all,
which is why it is recorded `partial` and not `done`.

## NEXT

Ordered by what a second round would gain most from.

1. **P2-018's automatic entry is not built, and it is the criterion's verb.**
   "A graphics/compositor/shell failure CAN ENTER a conservative recovery
   desktop" — today a person enters it; nothing detects a loop. Measured facts
   for whoever does it: greetd is a stock Fedora unit with NO `Restart=`, no
   `StartLimitBurst`, no `OnFailure=` and no drop-in anywhere in the repo; the
   only wiring points are `apex-greet-session` (boot-critical) and
   `apex-shell-autostart`; and `files/system/units/apex-boot-health.*` is INERT
   on every published image because both units carry
   `ConditionPathExists=…LoaderBootCountPath…` and every APEX image boots GRUB.
   Build it as a pure counter script with a state-dir override, tested in
   isolation with a mutation, and make the greet hook two lines that FAIL OPEN
   to the normal session. A counter that can strand somebody at a login screen
   is worse than no counter.
2. **`last-session` stickiness.** `GreetContext.qml:185` writes the chosen
   session on every launch, so picking Safe Graphics once makes it the
   preselected default forever. That is an apex-shell QML change and could not
   be run from here.
3. **P2-017's OAuth half.** `apex/src/cloudflare.rs` already has a complete RFC
   8628 device-code implementation — `Endpoints`, `connect_by_device_code`,
   `poll_interval` honouring `slow_down`. Wire Google and Microsoft to it,
   keeping its best idea: the refresh token goes under a SEPARATE service
   pinned to a different host, so the framework's endpoint pin makes it
   unspendable as an API token. Nothing refreshes anything today, Cloudflare
   included — that is one refresher for three providers.
4. **S3/R2 is a name and not a signer.** `broker::perform_webdav` sends headers
   it is given; SigV4 needs a request signer. P1-011's `temporary.rs` header
   records hitting the same wall with R2's `temp-access-credentials`.
5. **No file-manager integration.** gvfs with `gvfsd-dav` ships and `apex
   devices share` reports it, but nothing mounts an account. Doing it honestly
   needs a short-lived credential minted per mount (`Provider::mint` exists),
   not a copy of the stored one in the session.
6. **P2-019's server side and transport.** `relay/` is an undeployed Noise_IK
   rendezvous; whether a fleet client polls, holds a connection or is pushed to
   changes the threat model, and the operator side is unwritten.
7. **Not asserted at build time:** `thunar` and `nmtui` are named on the safe
   graphics menu and are present on the L16, but no Containerfile refusal
   checks them, because this branch could not run an image build and therefore
   could not confirm which layer installs them. `tests/test-apex-safe-graphics.sh`
   checks them and SKIPs where they are absent.
