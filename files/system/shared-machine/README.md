# Shared-machine recipes — guest and kiosk (roadmap P2-016)

`docs/multi-user.md` is the surface above this: `apex user`, what
"standard" and "administrator" mean on APEX, and what is not built.

Everything in this directory is **inert on a stock APEX**. No guest account is
created, no kiosk config is installed, no unit is enabled. That is deliberate
and it is the criterion:

> Kiosk/shared-device policy is possible without weakening ordinary desktop
> use.

A change that locked every desktop down in order to make kiosk possible would
have failed that, not met it. So the ordinary login path is untouched, and
`tests/test-apex-shared-machine.sh` asserts the untouched-ness directly — the
shipped `greetd-config.toml` has no auto-login stanza, and the suite fails if
one appears.

## What ships, and where it lands

| File | Installs to | Active? |
|---|---|---|
| `guest-accounts` | `/etc/apex/guest-accounts` | Yes, and **empty** |
| `greetd-kiosk.toml` | `/usr/share/apex/shared-machine/greetd-kiosk.toml` | No — a recipe |
| `sway-kiosk.conf` | `/usr/share/apex/shared-machine/sway-kiosk.conf` | No — a recipe |
| `../libexec/apex-guest-wipe` | `/usr/libexec/apex-guest-wipe` | Installed, refuses everything |
| `../units/apex-guest-wipe@.service` | `/usr/lib/systemd/system/` | Installed, not enabled |
| `../units/apex-guest-session@.service` | `/usr/lib/systemd/system/` | Installed, not enabled |

## Guest sessions, and the thing everybody gets wrong

The usual recipe for a disposable guest is a home directory on tmpfs. On APEX
that is **not enough**, and the reason is specific to this system:

P0-002 moved credentials *out of the home directory* on purpose.
`apex-secretd` keeps them in `/var/lib/apex-secretd/users/<uid>/`, root-owned
and `0700`, so that a process running as the user cannot read them. That
directory is not in the home, is not on the tmpfs, and does not go away when
the session ends.

So a guest who stores a credential leaves it for the next guest, who logs in to
the same uid and inherits the whole namespace: the credentials, the per-project
grants and the standing approvals. The home evaporating hides that rather than
fixing it.

`apex-guest-wipe` is the part that makes the claim true. It clears three
things, and the middle one is the reason it exists:

1. the contents of the home directory (not the directory — that is a mount
   point or a `tmpfiles.d` line);
2. `/var/lib/apex-secretd/users/<uid>/`, the protected credential namespace;
3. `/var/lib/apex-greet/last-user`, but only when it names the guest.

It is **not** a boundary against the guest while the session is running. The
guest is a real account with a real uid and the ordinary confinement applies.
It is the guarantee that nothing of theirs is still here afterwards.

### Turning it on

Three steps, none of which this repo performs:

```sh
# 1. the account. `apex user add` makes a STANDARD one -- not in wheel --
#    which is what a guest has to be.
sudo apex user add apex-guest
sudo passwd -d apex-guest          # no password, if that is the intent

# 2. the allowlist and the session hook, together
sudo apex user guest enable apex-guest

# 3. optional: also sweep at boot, for what a power cut left behind. This one
#    is by NAME, and it is a different unit -- see below.
sudo systemctl enable apex-guest-wipe@apex-guest.service
```

`apex user guest enable` is the whole of step 2: it writes the name to
`/etc/apex/guest-accounts` and enables `apex-guest-session@<uid>.service` — by
uid, because every unit logind creates per user is named by uid, and the wipe
engine resolves it back to the name. It refuses an administrator, the account
you are running as, uid 0, a system account, and a home the wipe would not
clear. `sudo apex user guest disable apex-guest` undoes it; `apex user guest
status` says who is configured. By hand, if you would rather:

```sh
echo apex-guest | sudo tee -a /etc/apex/guest-accounts
sudo systemctl enable apex-guest-session@"$(id -u apex-guest)".service
```

Step 2 is what makes a guest disposable in the ordinary case and step 3 is the
backstop. They are separate units because a refusal means opposite things in
the two places — see below.

The engine refuses every account that is not in that file. It also refuses uid
0, a home of `/`, `/home`, `/var/home`, `/root` or empty, an account that does
not exist, and an account that still has a logind session. Four fences, checked
in that order, before anything is removed — modelled on the four
`apex-disposable` puts on its recursive removal, for the same reason: a wrong
argument here is unrecoverable.

### Session end: what fires it, and the version that looks right and does not

Round 1 shipped the engine with no trigger — `apex-guest-wipe@.service` is
`After=user-%i.slice`, which *orders* it and never *fires* it.
`apex-guest-session@.service` is the trigger, and it was verified on a real
second account rather than reasoned about. Both directions:

| allowlist | after the guest logs out |
|---|---|
| names the guest | home cleared, `/var/lib/apex-secretd/users/<uid>` gone, unit deactivated successfully |
| emptied | nothing removed, unit **failed**, journal: `refusing: … is not listed in /etc/apex/guest-accounts` |

The second row is the one that had to be checked. The two units disagree about
what a refusal means, and it is not a detail:

* `apex-guest-wipe@.service` runs at boot on any machine, where "no guest is
  configured" is the normal answer. It carries `SuccessExitStatus=0 2`, so a
  refusal is quiet.
* `apex-guest-session@.service` is only ever enabled for an account that *is* a
  configured guest, at the one moment the wipe is supposed to happen. A refusal
  there means the guest's credentials are still sitting there for the next
  guest. It has **no** `SuccessExitStatus` on purpose. With the boot unit's
  copied across, that exact refusal produced a green `systemctl status` over an
  untouched guest home.

**The obvious wiring does not work and fails silently.** Binding the hook to
the user slice — `BindsTo=user-%i.slice`, `WantedBy=user-%i.slice` — reads
like the right hook and never fires:

* logind does not stop `user-<uid>.slice`. At logout it stops
  `user@<uid>.service` and `user-runtime-dir@<uid>.service`; the slice then
  goes away by garbage collection, once it is empty and nothing refers to it.
* a unit that `BindsTo=` the slice **is** something that refers to it. With the
  hook installed, `user-1042.slice` stayed `active` indefinitely after the
  guest logged out — sessions removed, `user@1042.service` inactive, slice
  still up. Remove the hook and the same slice was collected within seconds.

So the hook prevented the event it was waiting for. `user-runtime-dir@<uid>`
is explicitly stopped by logind and has no such loop, and `Before=` it on the
way up is what puts the wipe *after* it on the way down — the wipe sees a
torn-down session, not a live one.

The engine is exercised end to end by `tests/test-apex-shared-machine.sh`,
fences included; the unit's shape — what it binds to, and that it has no
`SuccessExitStatus` — is asserted there and at build time in
`Containerfile.base`.

**One failure mode the loudness earned, on the third live cycle.** A logind
session record stuck in `State=closing` — a session whose leader process is
gone but which logind never reaped — still counts as a session, so fence 4
refuses and the wipe does not run. The guest's credentials stay where they are.
That is the conservative answer and it is correct: from inside the engine, "the
record is stuck" and "somebody is still sitting at that desktop" look the same.
What makes it survivable is that the unit fails rather than reporting success,
so the journal says `refusing: … still has an active login session` and
`systemctl --failed` shows it. The boot-time sweep clears it at the next boot,
which is what that unit is the backstop for. `loginctl list-sessions` shows the
stuck record.

The boot sweep is not smarter about this, and it is worth saying so or the next
reader will go looking for the difference: both units run the same engine and
the same fence 4. It succeeds because logind keeps its session records in
`/run/systemd/sessions`, which does not survive a reboot — the stuck record is
simply gone by the time the sweep runs, along with every other session.

## Kiosk

`greetd-kiosk.toml` + `sway-kiosk.conf`. The important design note is in the
first of those and is worth repeating here because it is the thing a reader
will get wrong:

**greetd's `[initial_session]` auto-login runs once per boot, not once per
logout.** greetd(5), verified against the shipped greetd 0.10.3, says it "will
only be executed during the first run of greetd since boot ... checked through
the presence of the runfile". A kiosk built on it drops to the greeter the
first time its app exits and stays there. The kiosk session therefore goes in
`[default_session]`, which greetd restarts "whenever no session is running".

The kiosk boundary is made of **absent configuration**: `sway-kiosk.conf` does
not `include /etc/sway/config.d/*`, so there is no terminal binding, no
launcher, no exec binding and no workspace switching. Nothing is taken away
from anybody else's desktop to achieve it.

There is no authentication on the kiosk path — whoever can see the screen is
the kiosk user. That is what a kiosk is, and it is why the account must be
purpose-made, not in `wheel`, and not the machine owner's.

## Not enabled on the development laptop

Nothing here is enabled on the L16. The session-end measurements above were
made with a **temporary** fixture account (`apex-fx-guest`, uid 1042, home
under `/var/tmp`, not in `wheel`, password locked) created for the purpose and
deleted afterwards, with the hook installed under `/etc/systemd/system` and
removed with it. Everything in `tests/test-apex-shared-machine.sh` is asserted
against fixture trees and shipped files, not against a live account, so the
suite is as true on a one-account machine as on a twenty-account one.

The kiosk half was **not** booted. It is still shape assertions on shipped
files: no kiosk session has ever run, and no greetd config on any machine was
touched to try.
