# Trusted devices — `apex host`

The registry §20 dispatches from: which other machines this one may hand work
to, and what each of them turned out to be able to do.

Eight verbs, none of them privileged. The registry is your file, not the
machine's, so nothing here needs `sudo` and nothing here writes outside your
home.

## The transport is your own `~/.ssh/config`

A host entry names an **ssh destination**. It does not store an address, a
port, a key path or a known-hosts entry, and APEX generates no key and holds no
passphrase: authentication, host identity and transport are whatever
`ssh <destination>` already does on this account.

That is a deliberate choice and it is not about saving code. A real ssh alias
is frequently not "a hostname" — the alias this feature was built against
resolves over the LAN when the LAN is up, otherwise through a VPS port,
otherwise through a jump host into a reverse tunnel, three transports behind
one name selected by a `Match exec`. An address field in `hosts.toml` cannot
express that, so it would work at home and fail everywhere else, which is
exactly when remote compute is worth having. It also means adding a device
cannot produce a keyring or polkit prompt, because there is no new credential
to store.

One consequence worth knowing before you debug anything: APEX runs ssh with
`BatchMode=yes`. A host that would prompt for a password, or for confirmation
of an unknown host key, **fails here instead of asking**. Connect once by hand
first.

Everything reaches `ssh` as an argument rather than through a shell, and the
values in the registry come from a file rather than necessarily from the person
running the command, so names and destinations are validated against a narrow
character class and refused by name rather than quoted. A destination starting
with `-` would be an option, not a host; a name containing `/` or `..` would be
a path, because names are also filenames under the probe cache.

## `apex host add`

Registers a device and immediately probes it.

```
apex host add katana
apex host add bigbox --ssh deploy@10.0.0.4 --port 2222 --note "the big one"
```

The name is how you refer to the machine everywhere else — it is the value
`--on` takes, as in `apex ai run --on katana`. By default it is also the ssh
destination, so a machine already in `~/.ssh/config` needs nothing but its
alias; `--ssh` overrides that for a machine that is not in there.

The new entry is validated as part of the whole registry rather than on its
own, so it is held to exactly the standard a hand-edited file is held to. The
write is atomic — rendered, parsed back, compared, then renamed into place —
because a bad write discovered by the *next* command gives you no way to tell
which end was wrong.

**A device that does not answer is still added.** Registering and probing are
separate outcomes: the laptop may simply be off the LAN, so an unreachable
machine prints what went wrong, says the entry is saved, and names the probe
command to run later. `--no-probe` skips the attempt entirely.

## `apex host probe`

Asks a device what it can do, and caches the answer. `--all` does every
registered device.

Two paths, tried in order:

1. **`apex host describe --json` on the far side.** An APEX peer serialises the
   same struct this side deserialises, so the probe has no parser of its own
   and the two ends cannot disagree about the format.
2. **A portable shell probe**, for a machine with no `apex` — a plain Fedora
   box, a server, somebody else's laptop. POSIX `sh`, no `bash`, no `jq`,
   printing `key=value` lines, and its exit status is always 0 on purpose: a
   host that cannot report its GPU has still told you its CPU count, and a
   non-zero exit would throw that away. Unparseable lines are skipped rather
   than failing the probe as a whole, which is why it is `key=value` and not
   JSON assembled by hand in a shell.

Nothing a probe returns is trusted to be well formed. A trusted host is trusted
to *run your commands*, which is not the same as trusted to bound its own
output: the reply is read up to 64 KiB, every string field is truncated, and
the GPU and accelerator lists are capped, before any of it reaches the cache or
your terminal.

With `--all`, the exit status is non-zero only when **every** target failed —
so it is usable as a check, without one machine being switched off failing the
command on a laptop.

## `apex host list`

Every device, one line each, with what the last probe found:

```
katana  APEX 0.1.0 (gaming), 20 cpu, 62 GiB, cuda, ai,agent,build
l16     Fedora Linux 43 (Workstation Edition), 8 cpu, 15 GiB, build
```

The trailing group is capabilities rather than hardware: `ai` when the
inference service is installed, `agent` when the agent runtime is, `build` when
podman is. Those are reported by the presence of the binary and not by asking
systemd, because `apex-agentd` is opt-in and "not running" is its normal state
— asking systemd would make a capable host look incapable.

A probe older than seven days is annotated `probe Nd old`. It is **reported,
never refused**: a stale probe is still the best information available, and a
laptop is offline most of the time.

`--json` for scripts and for the shell. An empty registry is not an error; it
says so and names the command that adds one.

## `apex host show`

One device in full — destination, port, note, the capability line, the GPU
list, free space on `/var`.

It also prints any field a **newer** peer reported that this build does not
understand, marked as such, rather than dropping it. That line is usually the
only clue that the two ends are running different versions of APEX.

## `apex host remove`

Forgets a device: the registry entry goes, and so does its cached probe. A
cache file left behind for a removed host is noise, but failing the removal
over it would leave the registry and the cache disagreeing about whether the
machine exists, so removing the cache is best-effort and the entry goes either
way.

## `apex host run`

The escape hatch §24 asks APEX to keep: run one command on a device.

```
apex host run katana -- make -j20
apex host run katana --tty -- nvim /etc/hosts
```

Everything after `--` keeps its argument boundaries. Plain `ssh host cmd a b`
does not do that — ssh joins its remote arguments with spaces and hands the
string to the remote login shell, so a path with a space in it silently becomes
two arguments. Here each one is quoted into the remote command individually.

The remote command's exit status becomes this process's, because `apex host
run` **execs** ssh rather than spawning it and waiting. So `apex host run k --
false` exits 1, signals reach the remote, and the verb can be used in a script.
`--tty` (`-t`) allocates a terminal on the far side, which is what an editor or
any full-screen program needs.

## `apex host describe`

What *this* machine advertises, as a peer's probe sees it. Running it directly
is how you check what you are offering.

Everything is read from the local system and nothing is defaulted: a capability
this machine cannot demonstrate is reported absent. Accelerators are detected
by the presence of the thing that would be used rather than by a GPU name, on
the grounds that a machine can have an NVIDIA card and no CUDA.

## `apex host path`

Prints the two files, which is faster than remembering which base directory
each one obeys:

| | |
|---|---|
| `~/.config/apex/hosts.toml` | the registry — yours, hand-editable, refuses unknown keys |
| `~/.local/state/apex/hosts/` | the probe cache, one JSON file per device |

They are apart on purpose. A file written only in response to an explicit
command is user-owned and treats an unrecognised key as your typo; anything a
probe writes is a measurement, goes stale, tolerates fields it does not know,
and costs nothing but a re-probe if you delete it.

## When the registry is from a newer APEX

`hosts.toml` lives under `$XDG_CONFIG_HOME`, which does not roll back with the
image. So after a `bootc rollback` the older `apex` can meet a registry written
by the newer one, and it refuses it rather than guessing at a schema it does
not have:

```
hosts.toml is version 2, and this build of APEX reads version 1. It was
written by a newer APEX, which usually means a rollback: a host registry does
not roll back with the image. Boot the newer deployment again to use it.
```

Booting the other deployment gets the file back. Nothing is rewritten or
discarded in the meantime.
