# Recovery, repair and disposable execution

Roadmap §19. This is the reference for `apex recover`, `apex disposable` and
`apex doctor --json`: what exists, what each verb will and will not do, the
exact data boundary of the factory reset, and the two things §19 asks for that
APEX deliberately does not ship.

`docs/rollback.md` is the drill for the deployment layer underneath this
(`apex rollback`, `ostree admin pin`, rebuild-from-git). This document is the
surface on top of it.

## The recovery surface

`apex recover status` reports the eight components §19 names, each with a
state and the command that addresses it.

```
COMPONENT               STATE        DETAIL
Current deployment      verified     ostree 8f14e45fceea — APEX-OS 43 gaming
Previous deployment     available    2 deployments present, so there is one to go back to
Secure Boot             attention    firmware reports Secure Boot disabled
Filesystem              verified     /usr is read-only on an overlay root, ostree-booted
GPU driver              verified     1 — amd via amdgpu
APEX Shell              verified     vendored in the image, and this account is provisioned
Network                 available    a default route exists. Nothing was contacted.
Package extensions      verified     no user packages on this machine
```

Four states, and the distinction between the last two matters:

| state | means |
| --- | --- |
| `verified` | present and checked against something |
| `available` | present and usable, but nothing verified it — a rollback target exists; nobody asserted it boots |
| `attention` | present and wrong in a way a named action fixes |
| `unavailable` | could not be determined, or does not exist on this hardware. **Never** a synonym for "fine" |

`--json` emits the same rows with stable ids (`current-deployment`,
`previous-deployment`, `secure-boot`, `filesystem`, `gpu-driver`, `apex-shell`,
`network`, `package-extensions`), the four action buttons, the recovery routes
and the reset scopes. Those ids are a compatibility surface: APEX Settings keys
on them, so a rename is a broken settings page — the same rule
`org.apexos.Apexd1` members live under.

**It spawns no subprocess and contacts nothing.** Every fact is a file read:
`/proc/cmdline`, `/proc/mounts`, `/proc/modules`, `/proc/net/route`,
`/run/ostree-booted`, the efivars, the `/ostree/deploy` directory listing,
`/var/lib/apex/pkg/state.json`. Three things follow, and the suite asserts each
of them:

* it cannot raise an authentication prompt, because nothing it does needs
  authorising;
* it cannot hang, so APEX Settings can poll it;
* the states a healthy machine does not have — no rollback target, `/usr`
  mounted read-write, an extension built for the previous release, no GPU
  driver — are reachable as fixture trees instead of by reasoning.

The deliberate cost: the deployment row reports the ostree **checksum**, not
the image reference. The reference lives in `bootc status`, and parsing another
tool's JSON schema to duplicate what `apex changelog` already prints would buy
a second thing to keep in sync. The row names `apex changelog` instead.

### Rollback is `apex rollback`, and there is no second name for it

§19 lists `[Boot previous deployment]` as a button and asks to "make rollback
visible from Settings, not only the CLI". Both are satisfied by the
`previous-deployment` row: it reports whether there *is* anything to go back to
and names `sudo apex rollback` as the command a button runs. Adding an
`apex recover previous` verb would have been a second name for an operation
that already exists.

The row also carries the advice `docs/rollback.md` gives: bootc keeps only
booted+previous, so two bad updates in a row can evict the last good image, and
`sudo apex pin` before anything risky prevents that.

## Automatic repair

`apex recover repair` is a dry run unless given `--commit`.

**A step is eligible for automatic repair only if it is idempotent and removes
no data.** That invariant is what makes a single button defensible: pressing it
twice does nothing the second time, and pressing it by accident costs nothing.
`apexd-core`'s test asserts it over the whole table — no step's argv may
contain a destructive argument, and none may contain `sudo`, `pkexec`, `su`,
`run0` or `systemd-run`.

Rollback and factory reset are **not** repairs and this verb will never perform
either. §19 lists them as separate actions because they carry three different
levels of consequence.

Two steps, each offered only when the surface diagnoses it:

| step | domain | when |
| --- | --- | --- |
| `reprovision-desktop` | user | the `apex-shell` row reports attention |
| `rebuild-package-extension` | system | the `package-extensions` row reports attention |

Like `apex apply`, repair converges the privilege domain it is already running
in and *reports* the other. Nothing here calls `sudo` itself, so it cannot
raise an authentication prompt; `sudo apex recover repair --commit` runs the
system half.

`ostree admin pin 0` is deliberately **not** a repair step. It would pass every
invariant — it is idempotent and deletes nothing — but APEX cannot tell whether
it is *needed* without running `ostree admin status`, and `apex recover status`
reads files rather than spawning subprocesses. A repair with no diagnosis
behind it gets proposed on every healthy machine, and a button that always has
something to say is one people learn to ignore. It is advice on the
previous-deployment row instead.

## The factory reset, and exactly what it deletes

`apex recover reset` is the most destructive verb in the product, so it is
built to be refused.

**Dry run is the default.** Performing it needs `--commit` **and**
`--confirm <token>`, where the token is derived from the scope *and the exact
set of paths the plan found*:

```
$ apex recover reset --scope user
Factory reset — DRY RUN. Nothing has been changed.
…
To perform it, run exactly:
  apex recover reset --scope user --commit --confirm user:9:3f2a1c9b
```

A caller cannot construct that token from the scope alone — it has to run the
plan, which is the step that prints the loss list. If the machine changes
between plan and commit the token differs and the commit is refused with
nothing touched.

### Two scopes, and no third

| | `--scope desktop` | `--scope user` |
| --- | --- | --- |
| APEX Shell settings, keybinds, caches | removed | removed |
| the generated Hyprland input/monitor overrides | **emptied**, not removed | **emptied**, not removed |
| your blueprint (`~/.config/apex/blueprint.toml`) | preserved | **removed** |
| per-game profiles, trusted devices, local-model settings | preserved | **removed** |
| `~/.local/state/apex` (applied-blueprint record, probe cache, recorded agent sessions) | preserved | **removed** |

Preserved by **both**, and printed in full by the dry run:

* every document, project, checkout and credential in your home directory;
* `~/.ssh`, `~/.gnupg`, `~/.aws` and every browser profile;
* your Hyprland, niri and labwc configuration, including the lock/idle config;
* APEX Shell plugins in `~/.config/apex-shell/plugins`;
* your capsules and their records;
* installed packages, Flatpaks and downloaded models;
* the booted deployment and its rollback target.

### Why those boundaries, and not wider

* **Nothing under `/etc`.** It holds `passwd`, `shadow`, `fstab`, `crypttab`
  and the NetworkManager connections. ostree three-way-merges it against the
  deployment and there is no runtime verb that restores it to image state
  without deploying. A reset that emptied it would produce a machine that does
  not boot or cannot be logged into, and the thing that would undo it is what
  it broke.
* **Nothing under `/var/lib/apex`.** The package extension, the model store and
  the boot-health records each have a program that owns them (`apex-pkg`,
  `apex ai`, `apex-boot-health`). Deleting `pkg/state.json` under a *merged*
  system extension leaves `/usr` carrying packages APEX can no longer name —
  a worse state than the one being left. The machine-level operations are the
  verbs that already exist, and `apex recover status` names them.
* **Not the capsule records under `~/.local/share/apex/env`.** Each one names a
  real rootless container. Deleting the record orphans the container:
  `apex env list` would show nothing while `podman ps -a` still shows them, and
  APEX would have lost the name it needs to remove them. `apex env rm` is the
  verb for that.
* **Nothing under `~/.config/hypr` is ever *deleted*.** `hyprland.conf`
  `source=`s `apex-input.conf` and `apex-display.conf`, and Hyprland treats a
  `source=` with no match as a **fatal** config error — which is why
  `apex-shell-firstrun` pre-creates both as empty files. So those two are
  **truncated** to the empty state the provisioner itself seeds, and everything
  else in that directory is preserved. A one-line edit to a live compositor
  config has already cost this project a desktop once; `AGENTS.md`'s
  "Editing a live machine's configuration" rules exist because of it.

### The other protections

* **It refuses to run as root.** Root's home is not the user's, so
  `sudo apex recover reset` would reset root's desktop and leave the user's
  exactly as it is — while reporting success.
* **`$HOME` is validated before anything is resolved under it**: absolute, an
  existing directory, at least two components deep, and never `/`, `/home`,
  `/var/home`, `/usr`, `/etc` or `/var`.
* **Every target is re-resolved before it is touched.** The final component
  must not be a symlink (`realpath` on a symlink returns its *target*, so a
  prefix check alone would resolve `~/.cache/apex-shell` → `$HOME` and conclude
  `$HOME` is inside itself), the parent is canonicalised, and the rebuilt path
  must be inside the home and must not be the home. A single refusal aborts the
  **whole** reset — all-or-nothing, because a reset that removed four paths and
  then refused the fifth leaves a state nobody planned.
* **Everything removed is copied aside first**, to
  `~/apex-reset-backup-<timestamp>`, except caches. `AGENTS.md`'s first rule
  about touching a live config is to back it up, and that rule is what turned
  an earlier outage into a two-minute restore.
* **Preserved landmarks are re-checked afterwards.** Every landmark that
  existed before the commit must still exist after it. Grepping for what you
  deleted cannot detect what you deleted *as well*; this is what can.
* **It refuses to start if the provisioner is missing.** Removing the files
  APEX Shell needs with no way to put them back is worse than the state being
  left. `--no-reprovision` takes the deletion alone, deliberately.

### What a full factory reset is, and why it is not this verb

A machine indistinguishable from a fresh install — user accounts gone, `/etc`
pristine, disks repartitioned — is **a reinstall**, and the installer is what
does it. Nothing on a running system can recreate `/etc` from the image without
deploying, and deleting user accounts from a running system is not a CLI verb.

`apex recover reset --scope user` is the largest reset APEX will perform, and
it is scoped to one account's APEX-owned state. Saying so is better than a
`[Factory reset]` button that stops somewhere the user cannot predict.

## Recovery routes

`apex recover status` reports which ways back into a working system exist on
*this* machine. They are not uniform, and that is the point.

| route | when it exists |
| --- | --- |
| previous deployment | two or more deployments under `/ostree/deploy` |
| `rescue.target` | the kernel command line is editable at the boot menu |
| boot counting | `LoaderBootCountPath` is set — the opt-in systemd-boot path |
| disposable environment | `/usr/libexec/apex-disposable` is installed |
| recovery boot entry | **never** — see below |
| installer media | reported as *unknown*, because a running system cannot tell |

The rescue route is conditional on the **UKI**, not on the bootloader's name.
GRUB lets you edit the command line at the menu; a Unified Kernel Image does
not, because its command line is inside the signed image — which is what makes
the signature worth having. So on the opt-in systemd-boot+UKI path that route
does not exist, on exactly the machines that are hardest to get into. Reporting
it uniformly would be a false claim there.

### APEX ships no recovery boot entry, and that is deliberate

§19 asks for "a recovery boot entry **or** environment". APEX ships the
environment and documents the entry as an operator procedure. The reason is
`AGENTS.md`'s boot-path rules, which are binding:

> There is no rollback for an ESP you overwrote or an EFI variable you
> replaced, because the thing that would perform the rollback is what you
> broke.

Creating a genuine recovery *entry* means one of `bootctl install`,
`efibootmgr -c`, a file under `/boot/loader/entries`, or `grub2-mkconfig`
against a live `grub.cfg`. Rule 1 forbids every one of those on a real APEX
host, and `tests/test-boot-v2.sh` scans every shipped unit and helper for those
commands on executable lines and **fails the build** if one appears. Scripting
it would have broken an existing CI gate as well as the contract.

So `apex recover status` reports this route as `available: false` with the
reason, rather than omitting it. The operator procedure, for someone who wants
one:

```bash
# On a machine you can put into firmware setup, with a live USB you have
# actually booted once. Read docs/boot-v2.md's enrolment section first.
#
# GRUB is already installed and already in the firmware's boot order. The
# simplest recovery "entry" on a GRUB machine costs nothing and needs no
# script: at the menu, press `e`, append systemd.unit=rescue.target to the
# linux line, and press Ctrl-X.
#
# For a persistent menu entry, /etc/grub.d/40_custom is the supported place,
# and running grub2-mkconfig is YOUR decision on YOUR machine:
sudo $EDITOR /etc/grub.d/40_custom      # add a menuentry with the rescue cmdline
sudo cp /boot/grub2/grub.cfg /boot/grub2/grub.cfg.pre-recovery-entry
sudo grub2-mkconfig -o /boot/grub2/grub.cfg
```

Nothing in this repository runs those commands for you, and that is checked
rather than promised.

## Disposable environments

`apex disposable` is a **mode of APEX Capsules**, not a second environment
mechanism. Every disposable environment is an ordinary capsule created through
`/usr/libexec/apex-env`, so `apex env list` sees it and `podman ps` sees it.
What it adds is three things a capsule does not have: a home that is not
yours, an explicit copy boundary, and a teardown.

```bash
apex disposable plan --copy-in ~/src/thing --copy-out ~/results
apex disposable run  --copy-in ~/src/thing --copy-out ~/results -- make test
apex disposable run --git https://github.com/someone/unknown-project
apex disposable list       # normally empty: a run deletes its own
apex disposable purge      # removes ones a lost-power run left behind
```

### It is not a security boundary, and `plan` says so

distrobox mounts the host's root filesystem at `/run/host` inside **every**
capsule — that is how `distrobox-export` reaches back out to write a `.desktop`
file into the host's home, which `apex-env` documents and depends on. No
distrobox flag removes it. A program in a disposable capsule can read and write
your files through `/run/host`, as your own uid.

So "disposable" here means the **environment** is disposable — its packages,
its home, its state — and nothing more. §19's "run untrusted GitHub projects in
disposable capsules" is satisfied on the axis of throwing the environment away;
it is **not** satisfied on the axis of containing hostile code.

For confinement — `$HOME` masked, `~/.ssh` unreachable, the system bus
unreachable, the environment cleared and rebuilt from an allowlist — the
mechanism that exists and fails closed is the agent sandbox (`apex agent`,
`apexd/apex-agent-core/src/sandbox.rs`). The two compose: an agent session
confined by that sandbox can be started from inside a disposable capsule, and
then the environment is thrown away as well.

`apex disposable plan` prints all of this before anything is created, and the
image build asserts that `--help` still contains "not a security boundary".

### The copy boundary

Default-deny at both ends.

* **In**: nothing, unless `--copy-in` names it. It is a **copy**, not a bind:
  whatever happens inside cannot change the host original, which is stronger
  than a read-only mount and far easier to prove. Copies land at `~/in/<name>`.
* **Out**: nothing, unless `--copy-out` names a destination. Only `~/out`
  leaves. A destination inside the disposable root is refused, because teardown
  would delete it. An existing file at the destination is never overwritten —
  a collision refuses the **whole** copy-out, and `--force` is the explicit
  opt-in.
* **Devices**: none, ever. A capsule holding a device open is one that stops the
  machine suspending, and nothing disposable needs one.
* `--git` clones **inside** the environment, so your git configuration and
  credential helpers are never used for it. `https://` only: an ssh remote would
  need your key, and a disposable environment that can reach your ssh agent is
  one that can push with your identity.

### Teardown

The environment is deleted when it closes — on a clean exit, on a failure
inside, on a failed creation, and on Ctrl-C. Teardown is a recursive removal
driven by a name, which makes it the most dangerous line in the file, so it is
fenced four ways and **any** failure is a hard exit with nothing removed:

1. the name must match `disp-[a-z0-9]{1,24}`;
2. the final component must not be a symlink — `realpath` on a symlink returns
   its *target*, so a prefix check alone would resolve `<root>/disp-x` → `$HOME`
   and conclude `$HOME` is inside itself;
3. the path is resolved with `realpath -e`;
4. the resolved path must equal **exactly** `<resolved root>/<name>` — string
   equality, because a prefix test passes for `<root>-elsewhere/x`.

`purge` re-validates every name on the way out and leaves a directory that is
not a disposable environment alone, saying so rather than skipping silently.

Both guards are mutation-tested. Removing the symlink check fails one assertion
and the canary still survives, because the equality check catches it
independently; removing both fails six, including "THE CANARY IS GONE".

The disposable root is `$XDG_STATE_HOME/apex/disposable`, **not** `/tmp`. This
repository has already had `/tmp` wiped mid-session
(`docs/p1-progress.md`), and a sweep between `run` and its teardown would take
the one directory the user asked to keep.

## `apex doctor --json`

§19's "expose `apex doctor` results graphically", from the OS side. The same
checks the text form prints, in the shape a UI renders:

```json
{ "checks": [ {"ok": true, "check": "apexd running (owns org.apexos.Apexd1)"} ],
  "passed": 14, "warned": 3, "total": 17 }
```

It is the same list rendered twice, not a second set of checks: two diagnostic
implementations disagree, and the one the user reads would be the one wired to
nothing. The suite asserts that the JSON and the text report the same count.

**No severity field.** `apex doctor`'s own comment says a WARN is information
rather than a fault — a laptop with no ACPI `platform_profile` is not broken.
Adding a severity would mean inventing a judgement the checks do not make, and
a UI painting an invented judgement red is worse than one showing two states.

## What §19 asks for and this does not do

* **A recovery boot entry: not shipped, deliberately.** See above. The
  constraint is `AGENTS.md` boot-path rule 1 and the existing
  `tests/test-boot-v2.sh` gate, and the honest deliverable is §19's own
  alternative — a recovery *environment* — plus the entry as a documented
  operator procedure.
* **A full factory reset: not shipped, deliberately.** Accounts, `/etc` and
  partitions belong to the installer. The largest reset here is scoped to one
  account's APEX-owned state, and `apex recover reset --scope user` prints
  exactly where it stops.
* **"Open normally / Open isolated / Open disposable" is two modes, not
  three.** "Open normally" is the host's default handler and "Open disposable"
  is `apex disposable run`. There is no generic *confined-open* verb: the
  bubblewrap sandbox in `apex-agent-core` confines **agent sessions**, and
  exposing it as a general "run this one program confined" command is a real
  feature that does not exist yet. Offering a three-item menu where one item
  does nothing different would be worse than reporting the gap.
* **No GUI.** §19's action list is a UI, and the OS side of it is
  `apex recover status --json` plus `apex doctor --json`. APEX Shell renders
  them; nothing in this repository writes QML for it.
