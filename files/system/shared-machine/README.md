# Shared-machine recipes: guest and kiosk (roadmap P2-016)

`docs/multi-user.md` is the surface above this: `apex user`, what
"standard" and "administrator" mean on APEX, and what is not built.

Everything in this directory is **inert on a stock APEX**. The image creates no
guest account, installs no kiosk config and enables no unit. That is the
criterion:

> Kiosk/shared-device policy is possible without weakening ordinary desktop
> use.

A change that locked every desktop down to make kiosk possible would have
failed that. The ordinary login path is therefore untouched, and
`tests/test-apex-shared-machine.sh` asserts it: the shipped
`greetd-config.toml` has no auto-login stanza, and the suite fails if one
appears.

## What ships, and where it lands

| File | Installs to | Active? |
|---|---|---|
| `guest-accounts` | `/etc/apex/guest-accounts` | Yes, and **empty** |
| `greetd-kiosk.toml` | `/usr/share/apex/shared-machine/greetd-kiosk.toml` | No, a recipe |
| `sway-kiosk.conf` | `/usr/share/apex/shared-machine/sway-kiosk.conf` | No, a recipe |
| `../libexec/apex-guest-wipe` | `/usr/libexec/apex-guest-wipe` | Installed, refuses everything |
| `../units/apex-guest-wipe@.service` | `/usr/lib/systemd/system/` | Installed, not enabled |
| `../units/apex-guest-session@.service` | `/usr/lib/systemd/system/` | Installed, not enabled |

## Guest sessions, and why a tmpfs home is not enough

The usual recipe for a disposable guest is a home directory on tmpfs. On APEX
that is **not enough**:

P0-002 moved credentials *out of the home directory* on purpose.
`apex-secretd` keeps them in `/var/lib/apex-secretd/users/<uid>/`, root-owned
and `0700`, so that a process running as the user cannot read them. That
directory is not in the home, is not on the tmpfs, and does not go away when
the session ends.

A guest who stores a credential therefore leaves it for the next guest, who
logs in to the same uid and inherits the whole namespace: the credentials, the
per-project grants and the standing approvals. A home that evaporates hides
that without fixing it.

`apex-guest-wipe` closes that gap. It clears three things, and the middle one
is the reason it exists:

1. the contents of the home directory (not the directory, which is a mount
   point or a `tmpfiles.d` line);
2. `/var/lib/apex-secretd/users/<uid>/`, the protected credential namespace;
3. `/var/lib/apex-greet/last-user`, but only when it names the guest.

It is **not** a boundary against the guest while the session is running: the
guest is a real account with a real uid, and the ordinary confinement applies.
It guarantees that those three things are gone afterwards.

### Turning it on

Three steps, and the image performs none of them:

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
`/etc/apex/guest-accounts` and enables `apex-guest-session@<uid>.service`. The
unit is named by uid because logind names every per-user unit it creates by
uid; the wipe engine resolves the uid back to the name. It refuses an
administrator, the account you are running as, uid 0, a system account, and a
home the wipe would not clear. `sudo apex user guest disable apex-guest` undoes
it; `apex user guest status` says who is configured. To do it by hand instead:

```sh
echo apex-guest | sudo tee -a /etc/apex/guest-accounts
sudo systemctl enable apex-guest-session@"$(id -u apex-guest)".service
```

Step 2 makes a guest disposable in the ordinary case, and step 3 is the
backstop. They are separate units because a refusal means opposite things in
the two places (see below).

The engine refuses every account that is not in that file. It also refuses an
account that does not exist, uid 0, a home of `/`, `/home`, `/var/home`,
`/root` or empty, and an account that still has a logind session. It checks
these four fences in that order before it removes anything. They are modelled
on the four `apex-disposable` puts on its recursive removal, for the same
reason: a wrong argument here is unrecoverable.

### Session end: what fires it, and the version that looks right and does not

Round 1 shipped the engine with no trigger: `apex-guest-wipe@.service` is
`After=user-%i.slice`, which *orders* it and never *fires* it.
`apex-guest-session@.service` is the trigger, and a real second account
verified it in both directions:

| allowlist | after the guest logs out |
|---|---|
| names the guest | home cleared, `/var/lib/apex-secretd/users/<uid>` gone, unit deactivated successfully |
| emptied | nothing removed, unit **failed**, journal: `refusing: … is not listed in /etc/apex/guest-accounts` |

The second row had to be checked, because the two units disagree about what a
refusal means:

* `apex-guest-wipe@.service` runs at boot on any machine, where "no guest is
  configured" is the normal answer. It carries `SuccessExitStatus=0 2`, so a
  refusal is quiet.
* `apex-guest-session@.service` is only ever enabled for an account that *is* a
  configured guest, at the one moment the wipe is supposed to happen. A refusal
  there means the guest's credentials are still waiting for the next guest. It
  has **no** `SuccessExitStatus` on purpose: with the boot unit's line copied
  across, that exact refusal produced a green `systemctl status` over an
  untouched guest home.

**The obvious wiring fails, and fails silently.** Binding the hook to the user
slice (`BindsTo=user-%i.slice`, `WantedBy=user-%i.slice`) reads like the right
hook and never fires:

* logind does not stop `user-<uid>.slice`. At logout it stops
  `user@<uid>.service` and `user-runtime-dir@<uid>.service`; the slice then
  goes away by garbage collection, once it is empty and nothing refers to it.
* a unit that `BindsTo=` the slice **is** something that refers to it. With the
  hook installed, `user-1042.slice` stayed `active` indefinitely after the
  guest logged out: sessions removed, `user@1042.service` inactive, slice still
  up. With the hook removed, systemd collected the same slice within seconds.

The hook prevented the event it was waiting for. logind stops
`user-runtime-dir@<uid>` itself, so that unit has no such loop, and ordering the
hook `Before=` it on the way up puts the wipe *after* it on the way down: the
wipe sees a torn-down session, not a live one.

`tests/test-apex-shared-machine.sh` exercises the engine end to end, fences
included. It also asserts the unit's shape (what it binds to, and that it has
no `SuccessExitStatus`), and `Containerfile.base` asserts the same at build
time.

**The third live cycle found a failure mode, and the loud unit showed it.** A
logind session record stuck in `State=closing` (a session whose leader process
is gone but which logind never reaped) still counts as a session, so fence 4
refuses and the wipe does not run. The guest's credentials stay where they are.
That conservative answer is correct: from inside the engine, "the record is
stuck" and "somebody is still sitting at that desktop" look the same. The unit
fails instead of reporting success, so the journal says `refusing: … still has
an active login session` and `systemctl --failed` shows it. The boot-time sweep
clears it at the next boot, which is the case that unit is the backstop for.
`loginctl list-sessions` shows the stuck record.

The boot sweep is no smarter about this: both units run the same engine and the
same fence 4. It succeeds because logind keeps its session records in
`/run/systemd/sessions`, which does not survive a reboot, so the stuck record is
gone by the time the sweep runs, along with every other session.

## Kiosk

`greetd-kiosk.toml` and `sway-kiosk.conf`. The first carries the design note a
reader is most likely to get wrong, repeated here:

**greetd's `[initial_session]` auto-login runs once per boot, not once per
logout.** greetd(5), verified against the shipped greetd 0.10.3, says it "will
only be executed during the first run of greetd since boot ... checked through
the presence of the runfile". A kiosk built on it drops to the greeter the
first time its app exits and stays there. The kiosk session therefore goes in
`[default_session]`, which greetd restarts "whenever no session is running".

The kiosk boundary is made of **absent configuration**: `sway-kiosk.conf` does
not `include /etc/sway/config.d/*`, so there is no terminal binding, no
launcher, no exec binding and no workspace switching. It takes nothing away
from anybody else's desktop.

The kiosk path has no authentication: whoever can see the screen is the kiosk
user. That is what a kiosk is, and it is why the account must be purpose-made,
not in `wheel`, and not the machine owner's.

## Not enabled on the development laptop

Nothing here is enabled on the L16. The session-end measurements above used a
**temporary** fixture account (`apex-fx-guest`, uid 1042, home under
`/var/tmp`, not in `wheel`, password locked), created for the purpose and
deleted afterwards, with the hook installed under `/etc/systemd/system` and
removed with it. `tests/test-apex-shared-machine.sh` asserts everything against
fixture trees and shipped files, not against a live account, so the suite gives
the same answer on a one-account machine as on a twenty-account one.

Nobody has booted the kiosk half. It is still shape assertions on shipped
files: no kiosk session has ever run, and nobody touched a greetd config on any
machine to try.
