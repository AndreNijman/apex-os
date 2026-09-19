# pkg-share
items: P0-001 (the two remaining apex-pkg defects from the 2026-09-19 katana qualification)
repo: apex-os
worktree: /var/tmp/apex-work/wt-pkg-share
branch: task/pkg-share
base: roadmap/v2.2 @ f666f5c1
commit: c55f218e — pushed to origin/task/pkg-share, **NOT landed**

## NEXT

**The code work is finished. Nothing is in progress. Do not re-dispatch an agent
for it.** Two things remain and only one of them is anybody's job right now.

### 1. Land it. Nothing else here needs a machine.

`task/pkg-share` is one commit on top of `roadmap/v2.2` @ `f666f5c1`, touching
five files, all of which this unit owned:

```
files/system/libexec/apex-pkg        +188/-…    the fix
tests/test-apex-multilib-extract.sh  6 -> 11 assertions
tests/test-apex-pkg.sh               2 stale assertions replaced by 6
tests/test-apex-resolve.sh           the fake dnf5 made as strict as the real one
.github/workflows/pr-validation.yml  extract-step floor 6 -> 11
```

Landings are merges, not rebases (round 10 settled that). Nothing else in the
tree touches `apex-pkg`; `apex gaming` status code is **not** in it — checked,
`grep -n 'gaming\|gamescope' files/system/libexec/apex-pkg` prints nothing — so
there is no overlap with the `apex-gaming-session` unit.

`roadmap/v2.2` moved to `b0e34371` while this ran. Checked rather than assumed:
21 files landed since `f666f5c1` and **none of them is one of the five above**,
and `git merge-tree --write-tree origin/roadmap/v2.2 task/pkg-share` writes a
tree, so the merge is clean as of that tip.

### 2. One thing a real machine still has to confirm, and only one

Everything below was measured; this was not. **No package was installed on any
machine.** The container proves the ICD manifests reach the payload tree; it
cannot prove that a booted APEX machine then finds them. After this lands in an
image build, on katana:

```
sudo apex install steam 2>&1 | tee /tmp/install.log
ls /usr/share/vulkan/icd.d/ | grep -c i686            # was 0; must be >0
ls /usr/share/vulkan/implicit_layer.d/ | grep -c i686 # same class, same fix
grep -c 'VK_ERROR_INCOMPATIBLE_DRIVER' ~/.local/share/Steam/logs/console-linux.txt
```

Three readings out of that same log are worth more than the counts, because
each one is a thing only a real machine can say:

```
grep 'multilib: carrying' /tmp/install.log
```
prints `carrying N of M file(s) … (X already owned by the image, Y already
placed by this set's native packages)`. **X should dominate Y on katana.** That
is the whole point of control 2 in the suite: in a container the native-pass
clause does most of the work, and on a real machine the rpmdb clause does. If X
comes back small, the rule is not doing there what it does here and something
about the image's rpmdb needs looking at before anything else is believed.

```
grep -c "omitting '" /tmp/install.log     # was structurally 0
grep -c "refusing '"  /tmp/install.log
```
That split is the **only** visible effect of the `newer_satisfied_by_image`
fix, and it is invisible until a real set hits it. Before the fix the function
could not return 0, so `omitting` was unreachable and every newer-than-image
package became a `refusing` — compare against §2 of the 2026-09-19 evidence,
which recorded the run that produced the 219-package set.

and the §3 measurement alongside it, which this change must not regress:

```
sudo mount -o ro,loop /var/lib/extensions/apex-user.raw /mnt/apexext
sudo find /mnt/apexext/usr -mindepth 1 \( -type f -o -type l \) -printf "/usr/%P\n" \
  | LC_ALL=C sort > /tmp/ext.txt
rpm -qal | LC_ALL=C sort -u > /tmp/image.txt
LC_ALL=C comm -12 /tmp/ext.txt /tmp/image.txt | wc -l     # was 177, expect ~0
rm -rf ~/.cache/gstreamer-1.0; gst-inspect-1.0 | tail -1  # was "2 features", expect 1344
```

`LC_ALL=C` on **every** sort and comm. A first pass in the qualification report
reported "0 shadows" and was wrong because two sorted files had been produced
under different collations and `comm` silently under-reported.

**Do not hotfix katana to get this.** It needs an image build, and the defects
were deliberately left live there.

---

## What was wrong, and what replaced it

### Defect 1 (§6.5) — `apex install steam` produced a Steam with no 32-bit Vulkan

`extract_rpms`' 32-bit pass carried `--excludepath /usr/share`, which threw away
every `*_icd.i686.json` while keeping the `.so` files those manifests name.
`/usr/share/vulkan/icd.d` held 13 x86_64 ICDs and zero i686; Steam's client is
`ubuntu12_32/steam`, so it died with `VK_ERROR_INCOMPATIBLE_DRIVER`. It is also
why the 2026-09-17 live recovery had to hand-write `nvidia_icd.i686.json`.

The same line had already been wrong in the opposite direction (§3, fixed last
round): the list named four directories, and i686 packages also ship helper
executables under `/usr/libexec`.

**The list was the defect, so the list is gone.** The foreign set is unpacked
into a scratch root of its own (`${root}.multilib`, inside `$WORK`, so the
existing RETURN trap cleans it on every `die` path) and `merge_multilib` carries
a path only when **neither the running image nor this transaction's own native
pass already provides it**. `/boot` is the one thing still excluded outright,
because a sysext merges `/usr` and `/opt` and a bootloader file cannot reach the
system from there even in principle.

The image half asks `rpm -qal` — the same rpmdb question `install_etc`'s removal
pass asks before it deletes anything. It **fails closed in the opposite
direction to `image_owns()`**, and that inversion is the whole reason it is a
separate function: there, "cannot tell who owns it" must mean KEEP the file;
here it must mean REFUSE THE MERGE, because an empty owner list reads as "the
image owns nothing" and would carry a 32-bit copy of every image path into the
extension. `tests/test-apex-pkg.sh` has an assertion for exactly that, driven by
a stub `rpm` whose `-qal` answers nothing.

**Measured, not asserted** — fedora:43, `gstreamer1 + at-spi2-core +
mesa-vulkan-drivers`, 91 rpms / 57 i686, new rule vs the old one on the same set:

```
files 4235 -> 4258   bytes 756065407 -> 756110047   newly carried 23, lost 0
  12  /usr/share/vulkan/icd.d/*.i686.json        <- the defect
   6  /usr/libexec/getconf/*ILP32*               <- glibc.i686's 32-bit getconf variants
   1  /usr/bin/gio-querymodules-32               <- arch-suffixed by design
   4  licence / gdb-autoload text
```

Nothing under `/usr/lib/{systemd,udev,tmpfiles.d,sysusers.d}` came back and no
new `/etc` path did: the image owns those, so the rule drops them without ever
being told their names.

**"0 lost" counted files and symlinks, not directories** — deliberately, because
that is what the katana measurement counted and what an overlay can actually
hide. Directories were measured separately and they do move, in both directions:
547 -> 548, 12 out and 13 in, every one of them empty. The 12 are i686 `%dir`
entries the empty-directory prune now drops (`/usr/lib/gio/modules`,
`/usr/lib/dri`, `/usr/lib/pkcs11`, six `/usr/lib/llvm21/*`, `/run/setrans`,
`/var/cache/ldconfig`). Benign — GIO and mesa both tolerate a missing module
directory, and nothing was placing files there. The 13 arriving are the
mirror-image curiosity: `/usr/lib64/dri`, five `/usr/lib64/llvm21/*` and the
getconf/gdb directories, which the OLD scheme had *erased*, because unpacking
the i686 twin into the same rpmdb as its x86_64 sibling let rpm treat it as an
upgrade and remove the sibling's empty directories. Two roots cannot do that.

**Consequence for a future reader:** `/usr/bin/gio-querymodules-32` and the six
getconf helpers are 32-bit ELF files outside a library directory, and that is now
CORRECT. The old suite's rule "every 32-bit ELF must live in a library dir" would
fail on them, so it was replaced by the true rule, which is §3.5's own wording:
no 32-bit ELF may sit on a path the image or a 64-bit package in the set also
ships.

### Defect 2 (§11.1) — `apex resolve chromium` predicted the wrong source

Not a ranking bug. `probe_rpm` passed `--` to `dnf5 repoquery`, and dnf5 rejects
it outright:

```
$ dnf5 repoquery --quiet --queryformat '%{name}\n' -- chromium; echo $?
Unknown argument "--" for command "repoquery". …     (stderr)
2                                                     (and no stdout)
```

stderr was discarded, so `probe_rpm` returned nothing **for every package in the
world** and `apex resolve` silently lost its entire rpm leg. Chromium was just
where somebody noticed.

Verified on the L16 with the shipped engine as a positive control, same machine,
same minute:

```
$ APEX_PKG_NO_CAPSULE=1 bash /usr/libexec/apex-pkg resolve chromium
  flatpak  org.chromium.Chromium …        APEX would use: flatpak
$ APEX_PKG_NO_CAPSULE=1 bash files/system/libexec/apex-pkg resolve chromium
  rpm      chromium-152.0.7977.82-1.fc43  APEX would use: rpm
```

`newer_satisfied_by_image` carried the same `--` (line 481 of the old file), so
`guard_rpms` answered "not satisfied" for every versioned requirement it was
ever asked about and refused packages it was written to omit. Fixed with it —
same root cause, same file, found with it.

Also: a package carried by two repositories at one EVR was printed as two
candidates (`--latest-limit 1` is per repository). Deduped on name+EVR.

## Why 50+ existing assertions passed over defect 2

`tests/test-apex-resolve.sh` fakes `dnf5`, and the fake read only `"${@: -1}"` —
so `-- chromium` and `chromium` looked identical to it. A fake that accepts
arguments the real tool rejects is a gate that inspects nothing. **The fake now
refuses `--` with the real error text and rc 2**, and with that one change a
re-introduced `--` fails 11 of the suite's assertions instead of none. There is
also an assertion pinning the fake's own refusal, so the guard cannot rot.

`tests/test-apex-pkg.sh` was worse than blind: two of its assertions DEMANDED
`--excludepath /usr/share` in the 32-bit pass argv — the suite pinned the defect
in place. Replaced with assertions about what the new rule may see: the two
passes use different `--root`, the 32-bit pass hand-excludes nothing but
`/boot` (read as the whole set of `--excludepath` values, not as the absence of
the two directories anyone happened to think of), and a mute rpmdb stops the
merge.

## The instrument

`tests/test-apex-multilib-extract.sh`: 6 -> 11 assertions, and **six of the
eleven exist only to stop it passing for free**. Nothing in it names a directory
or a filename the engine also names; the packages and the container's rpmdb are
asked what they ship and own, and the tree is judged against the answers.

Two controls, and the second is the one that keeps it honest:

* **Control 1** re-extracts the set with the pre-fix exclude list and is read
  **twice**: it must still produce a `/usr/libexec` shadow (so §3 is visible on
  today's packages) and it must contain **none** of the Vulkan manifests (so
  §6.5 is visible too, and "they all arrived" is not a free pass).
* **Control 2** re-runs the merge with the **rpmdb clause removed** and must let
  an image-owned 32-bit file through. Without it this suite proves the wrong leg
  of the rule: in fedora:43 the x86_64 siblings of `gstreamer1` and
  `at-spi2-core` are *downloaded*, so the "already placed by the native pass"
  clause hides most of the defect — while on a real APEX machine every 64-bit
  sibling is in the **image** and the rpmdb clause is the only thing working.
  `glib2.x86_64` is installed in fedora:43 and therefore never downloaded, which
  is what makes `/usr/libexec/gio-launch-desktop` exercise that clause alone.
  Measured: control 2 lets 16 image-owned 32-bit files through.

The suite also calls `probe_rpm` against the **real** dnf5 the container has —
the only place in the tree with one.

Every new assertion was mutation-tested. Engine mutations: re-add
`--excludepath /usr/share` (ICD assertion fails), drop the rpmdb clause (16 ELF
shadows + 141 path shadows), drop the native-pass clause (11 ELF shadows), skip
the 32-bit pass (anti-blindness fires), re-add `--` (resolve leg fails), scratch
root == payload root, drop the mute-rpmdb refusal, remove the dedupe. Suite
mutations: blind control 1 both ways, blind control 2, blind both anti-blindness
derivations, disable the fake's `--` refusal. Every one was caught by the
intended assertion and only it; every restore was `cp` + `cmp`, byte-identical.

`newer_satisfied_by_image`'s `--` had **no** coverage anywhere: the resolve
fake never reaches `guard_rpms`, and the container suite that does run
`guard_rpms` against a real repository cannot construct a newer-than-installed
set on demand. Two things now cover it. A unit case drives the function
directly with both tools stubbed, in **both** directions so the fix cannot be
"always return 0", and its stub dnf5 refuses `--` exactly as the real one does.
And a class-wide static guard requires that **no** non-comment line in the
engine passes `--` to dnf5 — the defect is a property of dnf5, not of the two
call sites anyone happened to notice, and a future third one is covered without
edits.

Final: extract 11/0, resolve 91/0, pkg 82/0, multilib 3/0.
`shellcheck -S warning -x` clean on all four scripts (0.11.0, run in the
container — shellcheck is not installed on the L16).

## Two traps this unit paid for; do not re-pay them

* **`tests/test-apex-resolve.sh` spuriously FATALs with "dnf5 resolves to
  '/bin/dnf5'" when it is run in the SAME Bash-tool invocation that also writes
  files.** Measured, and stated as measured: 9 FATALs, every one of them in a
  call that had just written a file, and 0 FATALs in 12 runs of the same file
  made in calls of their own. The stock `roadmap/v2.2` version was run 9 times
  and never FATAL'd — but it was **never run under the write-in-same-call
  condition**, so this is a correlation with the harness, not a demonstration
  that the stock file is immune. When it fires, the fake exists, is
  `-rwxr-xr-x`, `find -perm -u+x` finds it and executing it directly works, but
  `env -i PATH="$BIN:…" bash -c 'command -v dnf5'` does not see it — which is
  not a thing a shell script can do to itself. **Run the suite in a call of its
  own** and it passes. Nothing was changed to work around it, because CI does
  not run suites that way. If someone wants the mechanism, the discriminator to
  try is whether `env -i` is what loses the file: run the same `command -v`
  without it while the condition holds.
* `--` is not a universal end-of-options separator. `dnf5` rejects it on every
  subcommand; `flatpak` accepts it (GOption), checked rather than assumed, so
  `probe_flatpak`'s `flatpak search -- "$1"` is left alone.

## Left for later, deliberately

`newer_satisfied_by_image` only inspects requirements whose capability **is the
package name**. A soname requirement (`libfoo.so.2()(64bit)`) that only the newer
build provides is invisible to it, so a package that genuinely needs the newer
build could be omitted rather than refused. Pre-existing, reachable today, and
out of this unit's scope — it wants its own measurement, not a guess.
