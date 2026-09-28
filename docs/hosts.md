# Trusted devices: `apex host`

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

The choice is deliberate. A real ssh alias is often more than a hostname. The
alias this feature was built against resolves over the LAN when the LAN is up,
otherwise through a VPS port, otherwise through a jump host into a reverse
tunnel: three transports behind one name, selected by a `Match exec`. An
address field in `hosts.toml` cannot express that, so it would work at home and
fail everywhere else, which is where remote compute is most useful. It also
means adding a device cannot produce a keyring or polkit prompt, because there
is no new credential to store.

Know one consequence before you debug anything: APEX runs ssh with
`BatchMode=yes`. A host that would prompt for a password, or for confirmation
of an unknown host key, **fails here instead of asking**. Connect once by hand
first.

APEX hands everything to `ssh` as arguments, never through a shell. The values
in the registry come from a file, and the person running the command may not
have written them, so APEX validates names and destinations against a narrow
character class and refuses a bad one by name instead of quoting it. A
destination starting with `-` would be an option, not a host; a name containing
`/` or `..` would be a path, because names are also filenames under the probe
cache.

## `apex host add`

Registers a device and probes it straight away.

```
apex host add katana
apex host add bigbox --ssh deploy@10.0.0.4 --port 2222 --note "the big one"
```

The name is how you refer to the machine everywhere else. It is the value
`--on` takes, as in `apex ai run --on katana`. By default it is also the ssh
destination, so a machine already in `~/.ssh/config` needs nothing but its
alias; `--ssh` overrides that for a machine that is not in there.

APEX validates the new entry as part of the whole registry, holding it to the
same standard as a hand-edited file. The write is atomic (rendered, parsed
back, compared, then renamed into place), because a bad write discovered by the
*next* command gives you no way to tell which end was wrong.

**A device that does not answer is still added.** Registering and probing are
separate outcomes: the laptop may be off the LAN, so for an unreachable machine
APEX prints what went wrong, says it saved the entry, and names the probe
command to run later. `--no-probe` skips the attempt.

## `apex host probe`

Asks a device what it can do, and caches the answer. `--all` probes each
registered device.

Two paths, tried in order:

1. **`apex host describe --json` on the far side.** An APEX peer serialises the
   same struct this side deserialises, so the probe has no parser of its own
   and the two ends cannot disagree about the format.
2. **A portable shell probe**, for a machine with no `apex`: a plain Fedora
   box, a server, somebody else's laptop. It is POSIX `sh` with no `bash` and
   no `jq`, it prints `key=value` lines, and its exit status is always 0 on
   purpose: a host that cannot report its GPU has still told you its CPU count,
   and a non-zero exit would throw that away. The parser skips a line it cannot
   read instead of failing the whole probe, which is why the format is
   `key=value` and not JSON assembled by hand in a shell.

APEX treats nothing a probe returns as well formed. A trusted host is trusted
to *run your commands*, which is not the same as trusting it to bound its own
output: APEX reads the reply up to 64 KiB, truncates each string field, and
caps the GPU and accelerator lists before any of it reaches the cache or your
terminal.

With `--all`, the exit status is non-zero only when **every** target failed, so
you can use it as a check without one switched-off machine failing the command
on a laptop.

## `apex host list`

One line per device, with what the last probe found:

```
katana  APEX 0.1.0 (gaming), 20 cpu, 62 GiB, cuda, ai,agent,build
l16     Fedora Linux 43 (Workstation Edition), 8 cpu, 15 GiB, build
```

The trailing group lists capabilities, not hardware: `ai` when the inference
service is installed, `agent` when the agent runtime is, `build` when podman
is. The probe reads these from the presence of each binary and does not ask
systemd: `apex-agentd` is opt-in and "not running" is its normal state, so
asking systemd would make a capable host look incapable.

A probe older than seven days carries the note `probe Nd old`. It is
**reported, never refused**: a stale probe is still the best information
available, and a laptop is offline most of the time.

`--json` gives the same list to scripts and to the shell. An empty registry is
not an error: `list` says so and names the command that adds one.

## `apex host show`

One device in full: destination, port, note, the capability line and the GPU
list.

`show` prints free space on `/var` **only when the probe reported it**, and
today only the shell-probe path does: `describe_self` does not report
`free_mib`, so an APEX peer, the *better* of the two probe paths, is the one
that shows no free-space line. Do not read its absence as a full disk.

It also prints, marked as such, any field a **newer** peer reported that this
build does not understand, instead of dropping it. That line is often the only
clue that the two ends run different versions of APEX.

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
does not: ssh joins its remote arguments with spaces and hands the string to
the remote login shell, so a path with a space in it becomes two arguments
without any warning. `apex host run` quotes each argument into the remote
command on its own.

The remote command's exit status becomes this process's, because
`apex host run` **execs** ssh instead of spawning it and waiting.
`apex host run k -- false` exits 1, signals reach the remote, and you can use
the verb in a script. `--tty` (`-t`) allocates a terminal on the far side,
which an editor or any full-screen program needs.

## `apex host describe`

Prints what *this* machine advertises, as a peer's probe sees it. Run it to
check what you are offering.

`describe` reads each field from the local system and defaults nothing: it
reports a capability this machine cannot demonstrate as absent. It detects
accelerators by the presence of the software that would drive them, not by a
GPU name, because a machine can have an NVIDIA card and no CUDA.

## `apex host path`

Prints the two files, which is faster than remembering which base directory
each one obeys:

| | |
|---|---|
| `~/.config/apex/hosts.toml` | the registry: yours, hand-editable, refuses unknown keys |
| `~/.local/state/apex/hosts/` | the probe cache, one JSON file per device |

They are apart on purpose. A file written only in response to an explicit
command is user-owned and treats an unrecognised key as your typo. Anything a
probe writes is a measurement: it goes stale, tolerates fields it does not
know, and costs nothing but a re-probe if you delete it.

## When the registry is from a newer APEX

`hosts.toml` lives under `$XDG_CONFIG_HOME`, which does not roll back with the
image. After a `bootc rollback` the older `apex` can meet a registry the newer
one wrote, and it refuses the file instead of guessing at a schema it does not
have:

```
hosts.toml is version 2, and this build of APEX reads version 1. It was
written by a newer APEX, which usually means a rollback: a host registry does
not roll back with the image. Boot the newer deployment again to use it.
```

Booting the other deployment gets the file back. APEX rewrites and discards
nothing in the meantime.
