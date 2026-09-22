# Multi-user, guest and shared machines

Roadmap P2-016. This is the reference for `apex user`, for what "standard" and
"administrator" mean on APEX, and for the disposable-guest and kiosk recipes
that sit next to them.

`files/system/shared-machine/README.md` is the operator's recipe for the guest
and kiosk halves, and it ships in the image at
`/usr/share/apex/shared-machine/README.md`. This document is the surface on top
of it.

## Standard vs administrator

An **administrator** on APEX is a member of `wheel`. That is not a convention
this document is inventing; it is what the running system resolves to, in two
independent places:

```
/usr/share/polkit-1/rules.d/50-default.rules
    polkit.addAdminRule(…) { return ["unix-group:wheel"]; }
/etc/sudoers
    %wheel ALL=(ALL) ALL
```

So `wheel` is what polkit's `auth_admin` asks for and what sudo asks for. Both
APEX agent polkit actions are `auth_admin` with `allow_any=no` and
`allow_inactive=no`, asserted at build time in `Containerfile.base`.

A **standard** account is everything else. It can use the desktop, hold its own
credentials, run its own agent and own its own paired remote devices. It cannot
become root, and it cannot approve an APEX privilege request.

`allow_inactive=no` is the part that matters on a shared machine: an account
whose session is switched away is *inactive*, and an inactive session cannot
answer an authentication prompt. Somebody at the keyboard has to be the one who
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

`apex user list` needs no root — "who can become root on this machine" is not a
privileged question:

```
NAME               UID     ROLE           GUEST  HOME
andre              1000    administrator  no     /var/home/andre
kiosk              1043    standard       no     /var/home/kiosk
apex-guest         1044    standard       yes    /var/home/apex-guest
```

Only accounts between `UID_MIN` and `UID_MAX` are listed. root and the system
accounts are not people.

**`apex user add` makes a STANDARD account.** `--admin` is the opt-in. That is
deliberately the opposite of what the installer does, and the reason is the
shape of the machine rather than a preference: the installer creates the
owner's account, who has to be able to administer the machine; every account
added afterwards is somebody else, and on a shared machine somebody else should
not be able to approve a root operation unless that was the intent.

No password is set. The account cannot be logged into until `passwd <name>` is
run. `--plan` prints the exact `useradd` line and changes nothing.

`apex user rm` refuses four things before it runs anything:

* uid 0;
* a system account below `UID_MIN`;
* the account you are running as;
* **the last administrator.** polkit's `auth_admin` and sudo both resolve to
  `wheel`, so an APEX with no member of `wheel` left cannot approve anything or
  become root again from inside the running system. There is no recovery path
  for that short of a rescue boot.

It also prints a path it does not remove:
`/var/lib/apex-secretd/users/<uid>/`. P0-002 moved credentials *out of the home
directory* on purpose, so `userdel -r` never sees them, and a uid can be handed
to a later account.

### What is missing, said plainly

**Nothing in APEX creates a standard account at install time.**
`installer/apex-install` puts the single account it creates in `wheel`
unconditionally and the installer GUI offers no choice. `apex user` is the
surface for every account after the first; the first is still always an
administrator, which is correct for a personal machine and is not a choice the
installer offers to make differently.

**Fast user switching is built.** `apex user switch` is the surface, and it
covers two operations that share nothing but a noun, which is why `--list` says
which one a given target would take:

- **The target is already logged in.** Their session is on another VT of this
  seat with its processes and agents alive, so the switch is `loginctl activate`
  and it is instant. This is the only path that earns the word *fast*.
- **The target is not logged in.** There is nothing to activate, so a login
  screen has to appear somewhere that is not this user's VT. greetd has no
  notion of a second seat and its VT is fixed at startup — greetd(5): "The
  specific VT is evaluated at startup, and does not change during the execution
  of greetd" — so a second greetd instance is started on another VT
  (`apex-switch-greeter@.service`, with a narrowly scoped sudoers rule).

The per-account isolation a second user needs was already real and tested —
separate agents, credentials, scratch and paired devices. This adds the switch
on top of it.

## Disposable guests

The guest half is `apex user guest`, and the thing it configures is
`apex-guest-wipe`. Read
`/usr/share/apex/shared-machine/README.md` before enabling it: the wipe is
destructive by design and there is no undo.

A home directory on tmpfs is the usual recipe for a disposable guest and it is
**not enough on APEX**, for a reason specific to this system. `apex-secretd`
keeps credentials in `/var/lib/apex-secretd/users/<uid>/`, root-owned and
`0700`, precisely so a process running as the user cannot read them. That
directory is not in the home, is not on the tmpfs, and does not go away when the
session ends — so a guest who stores a credential leaves it, plus their
per-project grants and standing approvals, for the next guest at the same uid.
The home evaporating hides that rather than fixing it.

```sh
sudo apex user add apex-guest              # standard, not in wheel
sudo passwd -d apex-guest                  # no password, if that is the intent
sudo apex user guest enable apex-guest
```

`guest enable` writes the name to `/etc/apex/guest-accounts` — the allowlist the
wipe engine reads, which **ships empty** — and enables
`apex-guest-session@<uid>.service`, by uid, because every unit logind creates
per user is named by uid.

It refuses an administrator, the account you are running as, uid 0, a system
account, and an account whose home the wipe would not clear (`/`, `/home`,
`/var/home`, `/root`).

At logout the wipe clears the home contents, the credential namespace, and the
greeter's last-user when it names the guest. A fence that holds is a **failed
unit**, not a quiet success: if the wipe refuses, the guest's credentials are
still there and that has to be visible.

## Kiosk

`/usr/share/apex/shared-machine/greetd-kiosk.toml` and `sway-kiosk.conf` are
recipes. Nothing installs them over the login path, and the shipped greeter
config carries no auto-login stanza — asserted by
`tests/test-apex-shared-machine.sh`, which also feeds the same check a config
that *has* one, because an absence assertion that never sees a presence cannot
fail.

The kiosk boundary is **absent configuration**: `sway-kiosk.conf` does not
include `/etc/sway/config.d/*`, so there is no terminal binding, no launcher and
no workspace switching to escape through. Nothing is taken away from anybody
else's desktop to achieve it, which is the criterion — a change that locked
every desktop down to make kiosk possible would have failed it.

The kiosk session belongs in greetd's `[default_session]`, not
`[initial_session]`: greetd(5) says the initial session runs only on the first
run since boot, so a kiosk built on it drops to a greeter the first time its app
exits and stays there.

No kiosk session has been booted on real hardware. Everything asserted about the
kiosk half is the shape of the shipped files.

## Per-account isolation, and what is asserted

| Thing | Where it lives | Asserted by |
|---|---|---|
| Credentials, grants, approvals | `/var/lib/apex-secretd/users/<uid>/`, root-owned `0700` | `apex-secret-core`, `tests/test-secret-at-rest.sh` |
| Agent scratch and session logs | `/tmp/apex-agent-<uid>/<id>/` | `apex-agent-core::paths` |
| Agent socket | `0600` in a `0700` per-user runtime dir | `apex-agentd` |
| Paired remote devices, identity key | `$XDG_STATE_HOME/apex-remote/` | `apex-remote-core::device` |
| Who may drive `apex-remoted` | its control socket checks the caller's uid on every verb | `apex-remoted::control::authorized` |
| Who may approve a request | polkit `auth_admin`, `allow_inactive=no` | `Containerfile.base` |

`apex-remoted`'s control socket is the one worth spelling out, because it used
to be asymmetric. Only `pair` asked who was on the other end; `status`,
`devices` and `revoke` asked nothing, on the argument that the socket lives in a
`0700` directory inside `$XDG_RUNTIME_DIR` and another account cannot reach it.
That argument is true, and it is the same argument the pairing check dismisses
one line above with "checked anyway: it costs one syscall". Measured with a real
second account — the directory and the socket mode widened by hand, so that the
filesystem was not the thing answering — the other account got the machine key
and LAN addresses out of `status`, the paired-device list out of `devices`, and
reached the owner's device store through `revoke`, which answered "no paired
device 'somebody-elses-phone'": it had looked. Every verb now checks the
account, and `pair` additionally checks for a human at the keyboard. There is no
`sudo` path to keep open, because `apex remote` finds the socket through
`$XDG_RUNTIME_DIR`.

The scratch root is per-uid because it was not: it used to be `/tmp/apex-agent`,
created by whoever logged in first and owned by them, and `/tmp` is sticky — so
the **second** account on a machine could not start an agent at all. Session ids
restart at 1 per daemon, so the two accounts also named the same directories.

Putting the uid in the name does not by itself make the root this account's:
`/tmp/apex-agent-1000` is a predictable name in a world-writable directory, so
any account can create it first. `paths.rs` claimed that case was closed —
"`ensure_private_dir` will then fail to chmod a directory it does not own … a
loud refusal and a denial of service rather than a disclosure". That was never
measured. Measured on 2026-09-19 against the shipped function, with a second
real account playing the attacker:

| what the other account pre-creates | before | after |
|---|---|---|
| the root, `0755` | `Err(EACCES)` — the claim | `Err`, naming uid and mode |
| the root, `0777` | **`Ok(())`** | `Err`, naming the owner it found |
| the session directory, `0777` | `Err(EPERM)` | `Err`, naming the owner it found |
| the session directory as a **symlink** | **`Ok(())`, and the symlink's target was chmodded `0700`** | `Err`, target untouched |

`0755` is the only mode an attacker would not choose. The reason is the one
this page already gives for the old shared root, one level down:
`ensure_private_dir` was called on the session directory, so `create_dir_all`
made the root along the way and nothing looked at it — leaving the root at the
umask (`0755`, enough to list another user's session ids) even with nobody
attacking it. `fs.protected_symlinks` stops a symlink planted directly in
sticky `/tmp`; an attacker-owned root is not sticky, so a symlink planted
*inside* it is followed, which is how row 2 buys row 4.

Two changes, and both are needed. `ensure_private_dir` stats with
`symlink_metadata`, refuses a symlink, refuses a directory this account does
not own, and creates every component `0700` rather than at the umask. And the
root is ensured **first, as a directory in its own right** — only the owner of
a `0700` directory can rename the entries in it, so hardening the session
directory while another account owns the root is a check against a moving
target. Asserted in `apex-agent-core::paths` (the refusals, and their
accepting halves) and in `tests/test-agent-inject.sh`, which pre-creates its
fixture root `0755` before the daemon starts — a root the daemon *makes* is
`0700` either way, so only a root it *finds* can tell the boundary call from
its absence.

Measured on a two-account machine: every test binary in the workspace, run by
the second account inside its own logind session, gives the same result as the
owner's — 4339 passed, and the eight that fail fail identically for both.
