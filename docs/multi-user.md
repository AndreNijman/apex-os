# Multi-user, guest and shared machines

Roadmap P2-016. This is the reference for `apex user`, for what "standard" and
"administrator" mean on APEX, and for the disposable-guest and kiosk recipes
that sit next to them.

`files/system/shared-machine/README.md` is the operator's recipe for the guest
and kiosk halves, and it ships in the image at
`/usr/share/apex/shared-machine/README.md`. This document is the surface on top
of it.

## Standard vs administrator

An **administrator** on APEX is a member of `wheel`. The running system resolves
it that way in two independent places:

```
/usr/share/polkit-1/rules.d/50-default.rules
    polkit.addAdminRule(…) { return ["unix-group:wheel"]; }
/etc/sudoers
    %wheel ALL=(ALL) ALL
```

`wheel` is what polkit's `auth_admin` asks for and what sudo asks for. Both
APEX agent polkit actions are `auth_admin` with `allow_any=no` and
`allow_inactive=no`, and `Containerfile.base` asserts that at build time.

A **standard** account is everything else. It can use the desktop, hold its own
credentials, run its own agent and own its own paired remote devices. It cannot
become root, and it cannot approve an APEX privilege request.

`allow_inactive=no` matters most on a shared machine: an account whose session
is switched away is *inactive*, and an inactive session cannot answer an
authentication prompt. The person at the keyboard has to be the one who
approves.

## `apex user`

```
apex user list [--json]
sudo apex user add <name> [--admin] [--comment TEXT] [--plan]
sudo apex user rm <name> [--keep-home] [--plan]
sudo apex user guest enable <name> [--plan]
sudo apex user guest disable <name> [--plan]
apex user guest status
```

`apex user list` needs no root, because "who can become root on this machine"
is not a privileged question:

```
NAME               UID     ROLE           GUEST  HOME
andre              1000    administrator  no     /var/home/andre
kiosk              1043    standard       no     /var/home/kiosk
apex-guest         1044    standard       yes    /var/home/apex-guest
```

It lists only accounts between `UID_MIN` and `UID_MAX`; root and the system
accounts are not people.

**`apex user add` makes a STANDARD account.** `--admin` is the opt-in. The
installer does the opposite on purpose: it creates the owner's account, and the
owner has to be able to administer the machine. Every account added afterwards
belongs to somebody else, and on a shared machine somebody else should not be
able to approve a root operation unless you meant them to.

`apex user add` sets no password, so nobody can log in to the account until you
run `passwd <name>`. `--plan` prints the exact `useradd` line and changes
nothing.

`apex user rm` refuses four things before it runs anything:

* uid 0;
* a system account below `UID_MIN`;
* the account you are running as;
* **the last administrator.** polkit's `auth_admin` and sudo both resolve to
  `wheel`, so an APEX with no member of `wheel` left cannot approve anything or
  become root again from inside the running system. The only way back from
  that is a rescue boot.

It also prints a path it does not remove:
`/var/lib/apex-secretd/users/<uid>/`. P0-002 moved credentials *out of the home
directory* on purpose, so `userdel -r` never sees them, and a uid can be handed
to a later account.

### Install-time accounts, and switching users

**Nothing in APEX creates a standard account at install time.**
`installer/apex-install` puts the one account it creates in `wheel`
unconditionally, and the installer GUI offers no choice. `apex user` is the
surface for every account after the first. The first is always an
administrator, which is right for a personal machine.

**Fast user switching is built.** `apex user switch` is the surface, and it
covers two operations that share nothing but a noun, which is why
`apex user switch list` says which one each account would take:

- **The target is already logged in.** Their session is on another VT of this
  seat with its processes and agents alive, so the switch is `loginctl activate`
  and it is instant. This is the only path that earns the word *fast*.
- **The target is not logged in.** There is nothing to activate, so a login
  screen has to appear somewhere that is not this user's VT. greetd has no
  notion of a second seat, and its VT is fixed at startup (greetd(5): "The
  specific VT is evaluated at startup, and does not change during the execution
  of greetd"), so APEX starts a second greetd instance on another VT
  (`apex-switch-greeter@.service`, through a narrowly scoped sudoers rule).

`apex user switch to <name>` takes whichever path applies, and
`apex user switch greeter` opens a login screen on a spare VT without naming
anyone. Either way the session you leave is locked first, unless you pass
`--no-lock`, and it keeps running.

The per-account isolation a second user needs (separate agents, credentials,
scratch and paired devices) was already in place and tested before switching
landed.

## Disposable guests

The guest half is `apex user guest`, which configures `apex-guest-wipe`. Read
`/usr/share/apex/shared-machine/README.md` before you enable it: the wipe is
destructive by design and has no undo.

A home directory on tmpfs is the usual recipe for a disposable guest, and on
APEX it is **not enough**. `apex-secretd` keeps credentials in
`/var/lib/apex-secretd/users/<uid>/`, root-owned and `0700`, so that a process
running as the user cannot read them. That directory is not in the home, is not
on the tmpfs, and does not go away when the session ends. A guest who stores a
credential therefore leaves it, with their per-project grants and standing
approvals, for the next guest at the same uid. A home that evaporates hides
that without fixing it.

```sh
sudo apex user add apex-guest              # standard, not in wheel
sudo passwd -d apex-guest                  # no password, if that is the intent
sudo apex user guest enable apex-guest
```

`guest enable` writes the name to `/etc/apex/guest-accounts` (the allowlist the
wipe engine reads, which **ships empty**) and enables
`apex-guest-session@<uid>.service`. The unit is named by uid because logind
names every per-user unit it creates by uid.

It refuses an administrator, the account you are running as, uid 0, a system
account, and an account whose home the wipe would not clear (`/`, `/home`,
`/var/home`, `/root`).

At logout the wipe clears the home contents, the credential namespace, and the
greeter's last-user when it names the guest. A fence that holds shows up as a
**failed unit**, never a quiet success: if the wipe refuses, the guest's
credentials are still there, and you need to see that.

## Kiosk

`/usr/share/apex/shared-machine/greetd-kiosk.toml` and `sway-kiosk.conf` are
recipes. Nothing installs them over the login path, and the shipped greeter
config carries no auto-login stanza. `tests/test-apex-shared-machine.sh`
asserts that, and also feeds the same check a config that *has* one, because an
absence assertion that never sees a presence cannot fail.

The kiosk boundary is **absent configuration**: `sway-kiosk.conf` does not
include `/etc/sway/config.d/*`, so there is no terminal binding, no launcher and
no workspace switching to escape through. It takes nothing away from anybody
else's desktop, and that is the criterion: a change that locked every desktop
down to make kiosk possible would have failed it.

The kiosk session belongs in greetd's `[default_session]`, not
`[initial_session]`: greetd(5) says the initial session runs only on the first
run since boot, so a kiosk built on it drops to a greeter the first time its app
exits and stays there.

Nobody has booted a kiosk session on real hardware. Every assertion about the
kiosk half is about the shape of the shipped files.

## Per-account isolation, and what is asserted

| Thing | Where it lives | Asserted by |
|---|---|---|
| Credentials, grants, approvals | `/var/lib/apex-secretd/users/<uid>/`, root-owned `0700` | `apex-secret-core`, `tests/test-secret-at-rest.sh` |
| Agent scratch and session logs | `/tmp/apex-agent-<uid>/<id>/` | `apex-agent-core::paths` |
| Agent socket | `0600` in a `0700` per-user runtime dir | `apex-agentd` |
| Paired remote devices, identity key | `$XDG_STATE_HOME/apex-remote/` | `apex-remote-core::device` |
| Who may drive `apex-remoted` | its control socket checks the caller's uid on every verb | `apex-remoted::control::authorized` |
| Who may approve a request | polkit `auth_admin`, `allow_inactive=no` | `Containerfile.base` |

`apex-remoted`'s control socket needs spelling out, because it used to be
asymmetric. Only `pair` asked who was on the other end; `status`, `devices` and
`revoke` asked nothing, on the argument that the socket lives in a `0700`
directory inside `$XDG_RUNTIME_DIR` and another account cannot reach it. That
argument is true, and the pairing check one line above dismisses the same
argument with "checked anyway: it costs one syscall". A test with a real second
account, with the directory and socket mode widened by hand so that the
filesystem was not the thing answering, showed the gap: the other account got
the machine key and LAN addresses out of `status`, the paired-device list out of
`devices`, and reached the owner's device store through `revoke`, which
answered "no paired device 'somebody-elses-phone'" because it had looked. Every
verb now checks the account, and `pair` also checks for a human at the
keyboard. There is no `sudo` path to keep open, because `apex remote` finds the
socket through `$XDG_RUNTIME_DIR`.

The scratch root is per-uid because a shared one broke: it used to be
`/tmp/apex-agent`, created and owned by whoever logged in first, and `/tmp` is
sticky, so the **second** account on a machine could not start an agent at all.
Session ids restart at 1 per daemon, so the two accounts also named the same
directories.

A uid in the name does not by itself make the root this account's:
`/tmp/apex-agent-1000` is a predictable name in a world-writable directory, so
any account can create it first. `paths.rs` claimed that case was closed:
"`ensure_private_dir` will then fail to chmod a directory it does not own … a
loud refusal and a denial of service rather than a disclosure". Nobody had
measured it. A measurement on 2026-09-19 against the shipped function, with a
second real account playing the attacker:

| what the other account pre-creates | before | after |
|---|---|---|
| the root, `0755` | `Err(EACCES)` (the claim) | `Err`, naming uid and mode |
| the root, `0777` | **`Ok(())`** | `Err`, naming the owner it found |
| the session directory, `0777` | `Err(EPERM)` | `Err`, naming the owner it found |
| the session directory as a **symlink** | **`Ok(())`, and the symlink's target was chmodded `0700`** | `Err`, target untouched |

`0755` is the only mode an attacker would not choose. The cause is the one this
page gives for the old shared root, one level down: the code called
`ensure_private_dir` on the session directory, so `create_dir_all` made the root
along the way and nothing looked at it. That left the root at the umask
(`0755`, enough to list another user's session ids) even with nobody attacking
it. `fs.protected_symlinks` stops a symlink planted directly in sticky `/tmp`;
an attacker-owned root is not sticky, so the kernel follows a symlink planted
*inside* it, which is how row 2 buys row 4.

The fix needed two changes. `ensure_private_dir` stats with `symlink_metadata`,
refuses a symlink, refuses a directory this account does not own, and creates
every component `0700` instead of at the umask. And the code ensures the root
**first, as a directory in its own right**: only the owner of a `0700`
directory can rename the entries in it, so hardening the session directory
while another account owns the root checks a moving target.
`apex-agent-core::paths` asserts this (the refusals, and their accepting
halves), and so does `tests/test-agent-inject.sh`, which pre-creates its
fixture root `0755` before the daemon starts. A root the daemon *makes* is
`0700` either way, so only a root it *finds* can tell the boundary call from
its absence.

On a two-account machine, every test binary in the workspace, run by the second
account inside its own logind session, gave the same result as the owner's:
4339 passed, and the eight that failed failed identically for both.
