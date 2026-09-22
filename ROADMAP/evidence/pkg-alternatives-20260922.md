# pkg-alternatives — extraction never runs a scriptlet, so every package that publishes through `alternatives` installs broken

Round 40, 2026-09-22. Unit `pkg-alternatives`, item P0-001 (the package
engine), feeding P1-038. Branch `task/pkg-alternatives`. All measurements on the
L16 in `registry.fedoraproject.org/fedora:43` under rootless podman; nothing was
run on katana, whose root filesystem is 96% full.

## 0. What was already known, and what this unit adds

`95c0a798` diagnosed it from katana: `apex install wine` produced
`/usr/bin/wine32`, `wine64`, `wineserver32` and `wineserver64` as real files,
`/usr/bin/wine` and `/usr/bin/wineserver` **absent**, and twelve `/usr/bin`
entries as symlinks to `wine` that all dangled. The install reported success.
`command -v` answered, `wine64 --version` printed `wine-11.0 (Staging)`, and
only actually running a program failed.

That diagnosis is confirmed here from the package rather than from the machine,
and the twelve are reproduced in a container without touching katana. What this
unit adds is the mechanism, a fix, and a gate that is red before it and green
after.

## 1. The three checks that disagreed, and the one that settled it

Kept because it cost two sessions and it is the general lesson.

| check | answer | what it was actually asking |
|---|---|---|
| `ls /usr/bin \| grep ^wine` | present | does a directory entry exist — `ls` does not follow symlinks |
| `test -e /usr/bin/wineboot` | missing | does the TARGET exist — `test -e` does follow |
| `stat -c %s /usr/bin/wineboot` | **4** | how long is the link — and 4 is `strlen("wine")` |

All three were right about different questions. The 4-byte answer is what gave
it away, and the suite prints it for all twelve names for that reason. Earlier
the same night the mirror-image error was made from `command -v wine`, when the
shipped binary is `wine64`. **When a file's existence is the question, say which
existence you mean.**

## 2. The cause, read out of the packages

`extract_rpms` runs `rpm -Uvh --noscripts`. `wine-core` creates
`/usr/bin/wine` and `/usr/bin/wineserver` in **`%posttrans`** — not `%post`,
which matters for anything that goes looking:

```
/usr/bin/alternatives --install /usr/bin/wine       wine       /usr/bin/wine64       20
/usr/bin/alternatives --install /usr/bin/wineserver wineserver /usr/bin/wineserver64 20
```

Both paths are `%ghost` in the rpm's own file list, so rpm never extracts them
either. Twelve more `--install` calls in the same scriptlet publish
`/usr/lib64/wine-wow64/wine/{x86_64,i386}-windows/*.dll`, two of them with
`--slave` followers. **Wine is the example, not the bug**: `java`, the
`iptables`/`ebtables`/`arptables` nft wrappers, `nc`, editors and compilers all
publish the same way, and every one of them installs "successfully" and does
not work.

### A correction worth keeping

I first reported that APEX's `alternatives` was already half-dead — no admin
database — because `rpm -ql alternatives` lists `/etc/alternatives.admindir`
(a **dot**) and `/var/lib/alternatives`, and neither exists on the booted image.
Wrong. `strace` shows the binary reads **`/etc/alternatives-admindir`** (a
**hyphen**), which exists and is populated; `alternatives --display java` and
`--list` both work and `find /etc/alternatives -xtype l` is 0. The rpm's file
list and the binary disagree about the spelling. Read the syscall, not the file
list — the same family as "permission denied is not absence".

## 3. The mechanism, and why the other two were rejected

Rejected on measurements, not on taste.

**Parse the scriptlet text.** Dies on the real corpus. `iptables-nft` builds all
three of its invocations out of shell variables:

```
pfx=/usr/bin/iptables ; pfx6=/usr/bin/ip6tables
update-alternatives --install \
        $pfx iptables $pfx-nft 10 \
        --follower $pfx6 ip6tables $pfx6-nft \
        …
```

The tokens a parser reads are `$pfx`, not paths. A parser that resolved them
would be a shell.

**Run the real `alternatives` against the payload.** It cannot be relocated.
Measured:

```
# alternatives --altdir /r/etc/alternatives --admindir /r/etc/admin \
      --install /r/usr/bin/wine wine /usr/bin/wine64 20
/r/usr/bin/wine        -> /r/etc/alternatives/wine     # build root in a runtime symlink
/r/etc/alternatives/wine -> /usr/bin/wine64
/r/etc/admin/wine: "auto / /r/usr/bin/wine / … "        # and in the admin file too
```

**Reproduce the full alternatives triangle by hand.** `/etc/alternatives-admindir/<name>`
is a candidate *list* the image may already have an entry in, in an undocumented
format, and it would have to reach the real `/etc` through `install_etc` — the
pass that once deleted 26 image-owned `/etc` files. The cost of not doing it is
stated rather than hidden: `alternatives --display` and `--config` do not know
about extension-provided links, and the user cannot switch them. That is not
worse than a link that does not exist, and the extension is rebuilt wholesale on
every `apex install`/`remove` anyway.

**What was built instead:** the scriptlet's own shell is executed under a
recording `alternatives`, and the only thing that comes back out of it is argv.

## 4. What the engine is willing to run

`alternatives_record()` runs `%post` and `%posttrans` with `/bin/sh -c` and
nothing else:

* **as `nobody`, never as root** — `setpriv --reuid --regid --clear-groups
  --no-new-privs --inh-caps=-all --bounding-set=-all`, so a setuid binary
  reached by absolute path cannot climb back. If privileges cannot be dropped
  the scriptlet is **not run at all** and the pass says so. There is no path
  through this code that executes package code with privileges.
* **PATH holds one directory containing one program**, the recorder. `rm`,
  `systemctl`, `useradd`, `ldconfig` are not found.
* environment emptied (`env -i`), stdin on `/dev/null`, a 30 s timeout, and
  **no `-e`** — rpm does not use errexit either, and the first `rm: not found`
  must not abort before the alternatives line.

Interception needs both halves, measured: PATH alone catches `iptables-nft`'s
bare `update-alternatives` and misses `wine-core`, `nmap-ncat` and
`java-latest-openjdk-headless`, which all call `/usr/bin/alternatives` by
absolute path. The body is therefore also rewritten,
`(/usr)?/s?bin/(update-)?alternatives` → the recorder's own absolute path — not
a bare word, so a scriptlet guarding itself with `[ -x /usr/bin/alternatives ]`
still finds something executable.

The recorded argv is walked by each option's real arity (`--slave`/`--follower`
take three, `--initscript`/`--family`/`--altdir`/`--admindir` take one, the rest
none) and `--test` drops the whole invocation. Three refusals, each one a defect
this engine has already shipped in some other form:

* a link the **image** owns is never created, asked of `rpm -qf` and not of the
  filesystem — after this fix ships `/usr/bin/wine` exists on a live machine
  *because the extension put it there*, and a filesystem test would refuse to
  build it again on the next rebuild;
* a target in neither the payload nor the image is not linked to, which is what
  stops a conditional branch creating a **new** dangling link;
* nothing outside `/usr` and `/opt` is touched.

Higher priority wins, which falls out of applying candidates in descending
priority order and never replacing a link that already resolves.

## 5. The gate, red and green in one run

`tests/test-apex-pkg-alternatives.sh`. Four packages for four shapes of the same
declaration: `wine-core` (the measured one, 221 MB), `wine-common` (103 KB,
ships eleven of the twelve dangling symlinks and declares nothing),
`iptables-nft` (shell-variable arguments, bare `update-alternatives`) and
`nmap-ncat` (smallest complete example, and the refusal control).

**The property is not "no dangling symlinks."** `gcc`'s extraction leaves eight
— `/usr/lib/gcc/x86_64-redhat-linux/15/32/libasan.a -> ../../../i686-redhat-linux/15/libasan.a`
and friends, links into subpackages the set did not carry, measured here. A
suite demanding zero could never be made green. What is asserted is that every
link the set **declares** and the engine is **allowed to create** exists and
resolves; the katana symptom is a separate leg that attributes each dangling
link to the missing target it leads to.

No mutant is needed to show it red: a payload on which the pass has not yet run
IS what the old engine shipped.

### Measured on the L16, 2026-09-22, 56 s end to end

```
4 rpm(s) downloaded; extract_rpms rc=0; 86 symlink(s) in the payload
33 declaration(s) recorded from the scriptlets, 33 distinct link(s)
engine: alternatives: 33 of 33 declared link(s) created, 0 skipped
RED   placeable=33 absent=33  dangling-because-of-it=12  dangling-total=12
GREEN placeable=33 absent=0   dangling-because-of-it=0   dangling-total=0  refused=0
```

**Twelve**, independently of the katana count, and by name:

```
msidb msiexec notepad regedit regsvr32 wineboot winecfg wineconsole
winedbg winefile winemine winepath      each 4 bytes, each -> wine
/usr/bin/wine       -> /usr/bin/wine64        (created by the pass)
/usr/bin/wineserver -> /usr/bin/wineserver64  (created by the pass)
```

The other 21 declarations are the ones the symptom cannot see — the twelve
`.dll` links nothing points at, and iptables' and ncat's wrappers and man pages.
That is why the property is the upstream one.

### The assertions that exist so the others cannot pass over nothing

| leg | what it would catch |
|---|---|
| call site | every other leg calls `apply_alternatives` directly. Delete the line in `rebuild_extension` and all fourteen still pass, so the call order is read out of the shipped engine — and this one runs even where podman is absent |
| ≥ 10 declarations | a recorder that silently ran nothing |
| ≥ 10 **placeable** | "absent 0" reached by refusing everything |
| negative control | one link the pass just created is repointed at a path that does not exist; the checker's count must move `0 -> 1`. Without it, a `dangling_links()` that always answered "fine" would make every GREEN leg pass |
| refusal control | `nmap-ncat` is installed for real, so `rpm -qf` owns `/usr/bin/nc`; the pass must refuse to shadow it and say why. Measured: REFUSED, owner `nmap-ncat` |
| confinement canary | a purpose-built rpm whose `%post` writes `id -u` to a file and tries `rm -f /usr/bin/apex-alt-canary` by bare name **and** by absolute path. Measured: uid **65534**, canary **INTACT** |

`15 passed, 0 failed`. The checker the suite measures with is the engine's own
`dangling_links()`/`payload_link_resolves()`, so the suite cannot pass against a
checker production does not use — and that checker is itself on trial in the
negative control.

`payload_link_resolves()` exists because neither `find -xtype l` nor
`readlink -e` can answer this question: a system extension's `/usr` is overlaid
on the image's, so an absolute target may be satisfied by the payload **or** by
the image, and during the build the payload is mounted nowhere. `find -xtype l`
resolves against the live root only and calls every correct link in a
container-built payload dangling.

## 6. What this does NOT prove

* That wine then launches a Windows program. That needs the machine and it is
  P1-038's row, not this suite's. **The Wine row stays blocked until an image
  carrying this reaches katana.**
* That `alternatives --display` knows about extension links. It does not, by
  design — §3.
* Anything about packages whose scriptlet needs a command to compute its
  arguments. The PATH holds only the recorder, so such an invocation records
  short and is skipped with a warning. None of the 21 packages sampled here
  does that; it is a known limit, not a measured one.

## 7. Files

```
files/system/libexec/apex-pkg          alternatives_record, payload_link_resolves,
                                       dangling_links, alternatives_replay,
                                       alternatives_candidates, alternatives_place,
                                       apply_alternatives; called from
                                       rebuild_extension between extract_rpms and
                                       fix_caches
tests/test-apex-pkg-alternatives.sh    the gate, 15 assertions
.github/workflows/pr-validation.yml    `engine` job, floor 15
```
