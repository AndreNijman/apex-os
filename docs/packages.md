# Installing software on APEX-OS

```bash
sudo apex install android-tools           # a package from the repositories
sudo apex install ~/Downloads/vendor.rpm  # an .rpm file you downloaded
sudo apex install --allow-unsigned ~/Downloads/app.deb   # a Debian package
sudo apex remove android-tools
apex search wireshark
apex pkg list
```

That is the whole interface. It works for ordinary Fedora packages — CLI tools,
libraries, development toolchains, GUI applications, fonts, services — for a
local `.rpm` file, and for a local `.deb` file (the format some vendors, Claude
Desktop among them, publish for Linux and nothing else), and it does **not**
stop the OS from updating.

## Why this is not `rpm-ostree install`

APEX is a bootc image. Layering a package with `rpm-ostree` marks the deployment
as locally modified, and from that point on:

```text
error: Upgrading: Deployment contains local rpm-ostree modifications;
cannot upgrade via bootc.
```

One CLI tool and the machine silently stops receiving OS updates. Packages
applied with `--apply-live` were worse: they also disappeared on the next reboot,
so the user lost both the software and the update path.

`apex install` builds a **systemd system extension** instead: a squashfs image in
`/var/lib/extensions` that systemd overlays onto `/usr` at boot.

* the bootc deployment is never modified, so `bootc upgrade` keeps working
* programs land in the real `/usr/bin` — no wrappers, no PATH edits
* `.desktop` files, icons, man pages, shell completions, systemd units and udev
  rules work, because they sit exactly where the OS already looks
* removing a package is deleting a file; nothing rots in `/usr`
* `apex rollback` (the OS) and `apex pkg rollback` (packages) are independent

Fedora, RPM Fusion, and explicitly enabled COPRs are the repository sources, and
a path to an `.rpm` file installs that file. There is no APEX package registry to
host, sign or keep online, and every RPM is checked against a trusted RPM keyring
before a single file is extracted.

## What actually happens

1. `dnf5 download --resolve` resolves against the **installed image**, so only
   dependencies APEX does not already ship are downloaded. A local `.rpm` file is
   copied in from its cache at this point instead (see below).
2. Every RPM's signature is verified with `rpmkeys`.
3. The packages are extracted into a staging tree (`--noscripts`; the scriptlets
   that matter are emulated below).
4. Caches that are a single file describing a whole directory — GSettings
   schemas, the desktop database, the MIME database, GIO modules — are rebuilt
   from the **union** of the image and the new packages, so the extension can
   never hide the OS's own applications. Caches whose consumers re-scan safely
   (icon caches, fontconfig) are dropped instead.
5. The tree is labelled for SELinux with `setfiles`, so binaries are executable
   under enforcing.
6. It becomes one squashfs image, replaces the old one atomically, and systemd
   re-merges `/usr`.
7. `/etc` files ship to the real `/etc`; your edits are never overwritten (a new
   version lands beside yours as `*.apexnew`).

Everything the user requested lives in **one** extension, rebuilt from the
requested list on every change. Separate per-package images would fight over
shared dependencies and removal could delete files another package still needs.

## Installing a local `.rpm` file

Some software is only published as an RPM on a website — vendor browsers,
conferencing clients, editors. Point `apex install` at the file:

```bash
sudo apex install ~/Downloads/some-app.rpm
```

An argument is treated as a file when it ends in `.rpm`, when it contains a `/`,
or when it is an existing file that really starts with an RPM header. That test
runs **before** the Flatpak rule, because `org.foo.Bar.rpm` matches both.

It goes through the same pipeline as a repository package, so it produces the
same result: programs in the real `/usr/bin`, a `.desktop` entry in the app
launcher, icons, MIME associations, systemd units, udev rules and SELinux labels.
Its dependencies are still resolved from the repositories — the file's own
`Requires` are compared against what the image already provides, and only the
remainder is downloaded.

### The file is copied, and the copy is what gets rebuilt

The extension is rebuilt from scratch whenever it has to change: on `apex
update`, and on the first boot after an OS version change. If a rebuild needed
the path you typed, it would fail the moment the USB stick was unplugged or the
download was cleaned up.

So the file is copied into `/var/lib/apex/pkg/local/<NAME>.rpm` at install time,
and **every later rebuild reads that copy**. The requested list records
`local:<NAME>`, not a path.

Consequences worth knowing:

* Reinstalling from a newer file of the same package replaces the cached copy —
  that is how you update it.
* `apex update` re-resolves the file's **dependencies** against the repositories,
  but it cannot update the file itself; there is no repository to check. A local
  package stays at the version you installed until you install a newer file.
* `sudo apex remove NAME` uses the package name, not the path. The cached copy is
  retired at the same time (kept for one generation, so `apex pkg rollback` still
  works).

### Signatures: refused by default, opt-in per file

Vendor RPMs are signed by keys APEX has no reason to trust, and some are not
signed at all. APEX refuses them:

```text
apex-pkg: error: cannot verify /home/you/Downloads/some-app.rpm
apex-pkg: error: rpmkeys says: some-app.rpm: DIGESTS SIGNATURES NOT OK
...
apex-pkg: error: If the vendor's own site is where it came from and you accept that:
apex-pkg: error:   sudo apex install --allow-unsigned /home/you/Downloads/some-app.rpm
```

`--allow-unsigned` applies **only** to the files named on that command line.
Repository packages are never affected by it, and it is not a mode the engine
remembers — what it remembers is that one decision, recorded against that file's
exact checksum. Replace the cached file with different content and the decision
no longer applies.

Because the decision is recorded, the system keeps telling the truth about it:

```text
$ apex pkg list
packages (system extension):
  htop
  some-app  [local file, signature not verified]
```

`apex pkg verify` names them too. If the software is also published in a COPR,
enable that instead — signature checking then stays on.

### What a local RPM does not get

`%post` and friends are **not executed** (see below). For most packages that
changes nothing, but some vendor RPMs create their `/usr/bin` launcher symlink or
register a repository in `%post`, and those steps simply do not happen: the
program is installed under `/opt` with a working `.desktop` entry, but the short
command name may be missing from `PATH`. Check with `apex pkg info` what was
installed and call the real path, or use the Flatpak if the vendor ships one.

## Installing a local `.deb` file

```bash
sudo apex install --allow-unsigned ~/Downloads/claude-desktop_1.17282.0_amd64.deb
apex pkg list
sudo apex remove claude-desktop
```

Some software ships for Linux as a Debian package and as nothing else. Claude
Desktop is the one that forced this: Anthropic publishes an apt repository and
no RPM at all — five `rpm`/`yum` prefixes under `downloads.claude.ai` answer
404 — so before this existed the only way to have it on APEX was to unpack the
`.deb` into `/usr/local` by hand and run a per-application update timer beside
the OS's own. Electron applications are packaged this way constantly.

A `.deb` therefore goes through the **same** pipeline as everything else: the
same system extension, the same cache under `/var/lib/apex/pkg`, the same
requested-package list, the same `apex pkg rollback`. `apex update` rebuilds it
with everything else, and if a later APEX image starts shipping the same
application the extension copy is dropped rather than left shadowing it.

### APEX does not talk to apt

There is no apt client here. APEX **fetches no `.deb`**, resolves no Debian
dependency graph, tracks no Debian suite and knows nothing about
`sources.list`. `apex install ./thing.deb` installs a file you already have, and
that is the whole feature. Adding a repository client would mean maintaining a
second package database, a second signing-trust store and a second release
cadence, and none of those has a rollback story that composes with bootc.

### Its maintainer scripts are never run

dpkg executes `preinst`, `postinst`, `prerm` and `postrm` as root. APEX runs
none of them, and the install says so, naming the ones it skipped. They assume
dpkg, apt and a Debian filesystem — Claude Desktop's own `postinst` writes an
apt source and an AppArmor profile, and that apt source would be exactly the
per-application update channel APEX's design forbids.

That has a cost, and it is refused rather than hidden. A package whose program
exists only because its `postinst` creates it is **not installed at all**:

```text
apex-pkg: error: refusing 'PacketTracer': it ships no program APEX can start —
no executable in /usr/bin and no desktop entry whose Exec is a path the package
ships. Its entry point is created by a maintainer script (preinst postinst
prerm postrm), and APEX never runs those
```

A half-installed package that reports success is worse. `apex install wine`
once did precisely that through the RPM path — twelve `/usr/bin` entries
shipped as dangling symlinks, `/usr/bin/wine` absent, and the install printed
"done".

A package with no program **and no maintainer script** is a different thing
and installs normally: a font, an icon theme or a set of headers has nothing
to start, and nothing that was ever going to create one. The rule is "useless
without its `postinst`", so a package with no `postinst` cannot trip it.

### Its dependencies are reported, never resolved

`libgtk-3-0` is not a Fedora package name and no mapping between the two is
honest, so APEX does not invent one. The `Depends:` line is printed before you
accept the package, printed again as a warning when it installs, and recorded
in `apex pkg info`:

```text
apex-pkg: warning: 'claude-desktop': APEX resolved none of its Debian
dependencies (libgtk-3-0, libnotify4, libnss3, xdg-utils, …). Debian package
names do not exist on Fedora; anything APEX-OS does not already provide under
another name is yours to install
```

In practice a bundled Electron application needs nothing that a desktop APEX
install does not already have. A package that genuinely needs a library is
yours to install with `apex install` first.

### Signatures: there are none, and saying otherwise would be a lie

Every `.deb` needs `--allow-unsigned`. That is not laxity; it is the honest
reading of Debian's trust model, which signs the apt **index** a package is
downloaded through and not the package file. Detach the file from that chain —
download it from a website, copy it off a USB stick — and nothing is left to
check. Some vendors embed a `debsigs` `_gpgorigin` member; APEX carries no deb
keyring and no policy saying which key may sign what, so it does not treat one
as verification either.

The acceptance is recorded against that file's exact bytes under
`/var/lib/apex/pkg/deb`, exactly as it is for an RPM, so `apex pkg list` and
`apex pkg verify` keep saying which packages APEX never vouched for.

(The image build does verify Claude Desktop, by reconstructing the whole apt
chain with the signing-key fingerprint pinned in this repository. That takes a
network fetch and twenty lines of `Containerfile.core`; it is not something a
file on your disk can be put back into.)

### Where the payload may land

A system extension merges `/usr` and `/opt`, so those are the only two
hierarchies a `.deb` may write. Everything else is refused by name:

| Refused | Reason |
|---|---|
| Anything outside `/usr` and `/opt` (`/etc`, `/var`, …) | a system extension merges nothing else, and a Debian conffile's whole lifecycle is dpkg's |
| `/usr/local` | it is a symlink into `/var` on an ostree system, so a payload directory lands on the symlink rather than inside the merged tree |
| A shared library in `/usr/lib`, `/usr/lib64`, `/lib`, `/lib64` | a Debian build of a library in front of the image's own is unrecoverable without a rollback |
| Anything in a Debian multiarch directory (`/usr/lib/x86_64-linux-gnu`) | Fedora's linker never looks there, and moving it to `/usr/lib64` is the row above |
| Kernel modules and firmware | they need an initramfs and a real deployment |
| A symlink pointing out of `/usr` and `/opt` | it cannot resolve once merged, and it is how an archive escapes its own tree |
| A path APEX-OS already provides | an extension may not shadow the image — that is an OS update |
| An `i386` package on `x86_64` | half a Debian 32-bit userspace is worse than a refusal |

A library under the package's **own** directory is fine and is what most
`.deb`s actually ship: `/usr/lib/claude-desktop/libEGL.so` is found by that
application's RPATH and by nothing else.

### Ownership, for a format the rpmdb cannot see

`apex-pkg` normally decides whether a path belongs to the OS by asking the
rpmdb. That question has no answer for a `.deb`, and it has no answer for
Claude Desktop **as the image ships it** either, because the image installs it
with `cp -a` rather than from an RPM. So the `.deb` route asks the booted
ostree deployment instead — the pristine image tree, files and all — and not
the running `/usr`, which is an overlay carrying the extension being rebuilt.
What each `.deb` contributed is written to `/var/lib/apex/pkg/deb/NAME.files`,
which is the `rpm -qf` equivalent for those paths.
## Installing an AppImage

```bash
sudo apex install --allow-unsigned ./Thing.AppImage
```

An AppImage is one executable file with a whole filesystem glued to its back.
APEX unpacks it once, at install time, and installs the application inside it —
launcher entry, icon and command. **The AppImage is never run**, not at install
time and not afterwards.

### Why it is never run, and why that is the point

The classic AppImage runtime mounts its own payload with FUSE, and on APEX that
cannot work. Measured on the image: `fusermount3` is present, but
**`libfuse.so.2` is not, `fusermount` (the libfuse2 helper) is not, and
`squashfuse` is not.** Double-click a type-2 AppImage on a stock APEX machine
and you get the error everyone knows:

```
dlopen(): error loading libfuse.so.2
```

There were three ways to answer that, and the one APEX took costs the least:

| | What it means | Why not |
|---|---|---|
| Ship a fuse2 compatibility package | `libfuse.so.2` in the image | A deprecated ABI on every machine in the fleet, whether or not it ever sees an AppImage, so that a format APEX does not control can mount itself |
| Run each launch with `--appimage-extract-and-run` | Unpack on every start | Hundreds of megabytes of I/O before an Electron app's splash screen — and it does it by **executing the vendor's binary**, which this engine refuses to do for a `.deb`'s maintainer scripts and sandboxes for an RPM's `%post` |
| **Unpack once at install time** | `unsquashfs` into `/usr/local` | **Chosen.** No FUSE at install or at run time, no kernel mount, and nothing from the download is ever executed as root |

The payload's offset inside the file is `e_shoff + e_shentsize × e_shnum`, read
straight out of the ELF header with `od` — the same number `--appimage-offset`
prints, computed without asking the file about itself. `unsquashfs -o` does the
rest.

That also fits the OS better than the alternative would. RPM packages have to
become a systemd system extension because `/usr` is read-only composefs; an
AppImage does not, because it is self-contained and `/var` is writable. **An
installed AppImage is not part of `apex-user.raw` and appears in no requested
list**, so it survives an OS upgrade, a `bootc rollback`, an extension rebuild
and `apex remove` of every RPM on the machine.

### Where it goes

| Path | Holds |
|---|---|
| `/usr/local/lib/apex-appimage/NAME/` | the unpacked payload (`AppRun` and everything under it) |
| `/usr/local/bin/NAME` | a generated launcher |
| `/usr/local/share/applications/ID.desktop` | the launcher entry, rewritten to point at it |
| `/usr/local/share/icons/hicolor/**/apps/` | the icon named by the entry's `Icon=` key, and only that one |
| `/var/lib/apex/appimage/NAME.AppImage` | the accepted bytes, kept |
| `/var/lib/apex/appimage/NAME.{trust,files,json}` | the checksum you accepted, what was installed where, and the record |

`/usr/local` and not `/var` directly: on APEX `/usr/local` is a symlink to
`../var/usrlocal`, so it is writable; `/usr/local/bin` is already on `PATH` and
`/usr/local/share` is already in `XDG_DATA_DIRS`, so nothing needs a wrapper or
an `environment.d` drop-in; and SELinux's `/var/usrlocal → /usr/local`
substitution labels the tree `bin_t`/`lib_t` rather than `var_lib_t`, which is
what lets the desktop session execute it.

The launcher reconstructs the four variables the AppImage runtime would have
set — `APPDIR`, `APPIMAGE`, `ARGV0` and `OWD` — because `AppRun` scripts read
them.

### `apex update` does nothing to an AppImage. It is pinned.

This is the honest cost of the format, and it is written down here rather than
left for you to discover. An installed AppImage stays at the version you
installed. `sudo apex update` does not move it — and it tells you so, by name,
on every run. (`apex update`'s package pass used to return early on a machine
with no system extension, which is exactly a machine whose only user software
is an AppImage; it now also runs when `/var/lib/apex/appimage` holds a record,
so the line below is one you actually see rather than one this page claims.)

```
apex-pkg: AppImages are pinned and not updated by this command: obsidian
apex-pkg: to move one, run: sudo apex install --allow-unsigned /path/to/the/newer.AppImage
```

**APEX does not write an updater for AppImages.** This document says two
sections down that Zen Browser is a Flatpak because a tarball or an AppImage
"would need APEX to write and maintain its own updater to keep 'always the
latest stable' true". That is still true, and this feature does not pay that
cost — it declines it. A vendor's zsync channel (`X-AppImage-UpdateInformation`,
what AppImageUpdate follows) is **reported at install time and never followed**:

```
apex-pkg: it advertises the update channel 'zsync|https://…'; APEX does not follow it — this AppImage is pinned
```

So: if the software has an RPM, a COPR or a Flatpak, use that instead — those
track upstream through the update path that already exists. Reach for an
AppImage when there is nothing else, and expect to update it by hand.

Self-updating is not merely forbidden, it is **impossible**. The application
runs out of a root-owned `0755` tree and `$APPIMAGE` points at a root-owned
`0644` file, so an AppImage that tries to rewrite itself gets `EACCES` rather
than becoming a second update channel beside `apex update`.

### Signatures: the same rule as an RPM, not a weaker one

**Every AppImage needs `--allow-unsigned`.** Some embed a signature in a
`.sha256_sig` ELF section with the signing key in `.sig_key` — a key taken from
the file it signs proves nothing, so APEX does not accept it as verification,
the same conclusion the `.deb` route reached about `debsigs` and the same reason
the image *pins* both AI vendors' key fingerprints. Your acceptance is recorded
against that file's exact bytes under `/var/lib/apex/appimage`, so `apex pkg
list` and `apex pkg verify` keep telling the truth about where the software came
from, and swapping the file for different content revokes the decision instead
of inheriting it.

### What it refuses

| Refused | Why |
|---|---|
| A **type-1** AppImage (ISO 9660 payload) | Superseded in 2016. APEX unpacks only type 2 (squashfs) |
| A foreign architecture, or a 32-bit runtime | Read from the ELF header. It would never run |
| A payload with **no** `.desktop` file at its root, or more than one | The format allows exactly one. Zero means nothing says what the application is; several means APEX would be choosing on the vendor's behalf |
| A payload with no `AppRun` | That is the entry point every AppImage is required to provide |
| A name that would **shadow** something the OS provides | `/usr/local/bin` comes before `/usr/bin` on `PATH` and `/usr/local/share` before `/usr/share` in `XDG_DATA_DIRS`, so `./firefox.AppImage` would take over the browser for every user on the machine. `apex-pkg` decides image ownership by asking the rpmdb, which has no answer for an AppImage — so the question is asked about the path the install would *hide* |
| Overwriting any file APEX did not itself install | Under `/usr/local` as much as anywhere else |
| A `.desktop` or icon that resolves **outside** the payload | `.DirIcon` is conventionally a symlink, which is the obvious way to make a root process copy `/etc/shadow` somewhere world-readable |

Two things are taken away from every payload as it is unpacked: **setuid and
setgid bits**, and **ownership**. A FUSE-mounted AppImage is mounted `nosuid`,
so preserving a `4755` helper out of a download would grant strictly *more* than
running the AppImage normally ever does; and a squashfs built on the packager's
laptop records uid 1000, which is the desktop user on nearly every APEX machine.
Everything lands `root:root` with no group or other write.

`apex pkg verify` re-checks all of this later, including the one question only
time can answer: whether an RPM installed since has put the same command in
`/usr/bin`, where the AppImage's launcher now sits in front of it.

### Removing one

```bash
sudo apex remove NAME                    # the command name it installed
sudo apex remove ./Thing.AppImage        # or the file it came from
```

Either works: the file is matched by checksum, so the same download in a
different directory still resolves. Removal deletes exactly what the manifest
records and nothing outside `/usr/local`.

### The cost, stated rather than buried

An installed AppImage occupies roughly **two to three times** what the file
does: the unpacked tree (the payload uncompressed, so larger than the file it
came in) plus the original, which is kept because the trust marker is a checksum
*of those bytes* and because `$APPIMAGE` has to point at a file that exists. A
1 GB AppImage is therefore 2–3 GB of `/var`. It is all machine-local — none of
it touches the image, so it costs the fleet nothing.

## OS upgrades

An extension records the OS version it was built for, and systemd refuses to
merge a mismatched one. That refusal is the safety property that makes user
packages compatible with atomic updates — a Fedora 43 build is never overlaid
onto Fedora 44.

`apex-sysext-rebuild.service` completes the story: on the first boot after an OS
version change it rebuilds the extension against the new OS. It does nothing on a
normal boot, and if the machine is offline it says so and leaves the packages to
be rebuilt later rather than failing the boot.

APEX also records a package compatibility level. When an image starts baking a
package that users may already have in their extension, the level changes and
triggers one rebuild even if the Fedora version is unchanged. Requested packages
now provided by the image are removed automatically, so an older extension copy
cannot shadow the OS package.

That level is the only thing that notices an APEX image build at all: `VERSION_ID`
is the Fedora release and does not move when APEX rebuilds, and the resolved
package set comes from Fedora's repositories, which know nothing about what APEX
baked. To check on a machine that a rebuild really happened rather than being
skipped, read the level the extension was built at and the unit's own log:

```bash
jq -r .pkg_compat_level /var/lib/apex/pkg/state.json
journalctl -u apex-sysext-rebuild -b
```

A boot that rebuilt logs `extension compatibility changed … — rebuilding`, and
the unit takes minutes rather than finishing in the same second it started. A
state file still holding the older level means the rebuild has not run yet — an
offline boot leaves it for the next one, or for the next `apex update`.

`apex update` also re-resolves user packages, so they receive Fedora security
fixes instead of staying pinned at whatever was current on install day. If
nothing changed it stops early and does not re-merge `/usr`.

## Coming from a layered system

If a machine already has rpm-ostree layered packages, `apex update` will say so
and point at:

```bash
sudo apex pkg adopt
```

which rebuilds those same packages as an extension, then runs `rpm-ostree reset`
so the OS can update again. Reboot afterwards to drop the layered deployment.

## What it refuses, and why

| Refused | Reason |
|---|---|
| Kernels, `kmod-*`, `akmod-*` | need an initramfs and a real deployment — they belong in the image |
| `glibc`, `systemd`, `rpm`, `dnf`, `bootc`, `filesystem`, … | overlaying a second copy of the running userspace ABI is unrecoverable without a rollback |
| A **newer** version of something the image ships | that is an OS update, not a package install |
| Anything already in the image | already provided; nothing to do |
| An `.rpm` built for another architecture | it cannot run here |
| A `.deb` whose entry point only its `postinst` would create | APEX never runs maintainer scripts, so the program would not exist |
| A `.deb` shipping outside `/usr` and `/opt`, or a library into a linker path | see the `.deb` section above |
| An `.rpm` no trusted key covers | unless you pass `--allow-unsigned` for that file |
| A file that is not an RPM, is unreadable, or is a directory | refused by name, before anything is copied |

Packages with custom scriptlets install their files correctly, but APEX does not
execute arbitrary `%post` scripts against a live system: extraction runs with
`--noscripts --notriggers`. The scriptlets that matter in practice are emulated
against the union of image and extension — `ldconfig`, `systemd-sysusers`,
`systemd-tmpfiles`, the GSettings/desktop/MIME/GIO caches, `udevadm` — so
libraries resolve, users and directories exist, and applications appear in the
launcher. What does not happen is anything a package invents for itself: creating
symlinks, registering an external repository, generating keys, running a
first-time setup. If a package needs one of those to be useful, it belongs in the
image — open an issue.

## Commands

| Command | Does |
|---|---|
| `apex install PKG…` | add packages (`--no-weak-deps`, `--enable-repo=REPO`) |
| `apex install FILE.rpm` | add a local RPM file (`--allow-unsigned` if no trusted key covers it) |
| `apex install FILE.deb` | add a local Debian package (`--allow-unsigned` always; see above) |
| `apex remove PKG…` | remove packages (a local one by its package name) |
| `apex install FILE.AppImage` | unpack an AppImage into `/usr/local` (`--allow-unsigned` always). Pinned: `apex update` never moves it |
| `apex remove PKG…` | remove packages (a local one by its package name, an AppImage by its command name or its file) |
| `apex search TERM…` | search the repositories |
| `apex repo list` | list enabled and disabled RPM repositories |
| `apex repo enable-copr OWNER/PROJECT` | opt into a Fedora COPR for search/install/upgrade |
| `apex repo disable-copr OWNER/PROJECT` | disable an opted-in COPR |
| `apex pkg list` | requested packages and dependency count |
| `apex pkg status` | extension state, what it was built for, whether merged |
| `apex pkg upgrade` | re-resolve everything against the repositories |
| `apex pkg rebuild [--if-needed]` | rebuild for the running OS version |
| `apex pkg rollback` | restore the previous extension |
| `apex pkg verify` | check the extension against its recorded checksum, and each AppImage against the bytes you accepted |
| `apex pkg adopt` | convert rpm-ostree layers into APEX packages |

Read-only verbs work as an ordinary user; anything that writes needs `sudo`.

RPM Fusion Free and Nonfree are enabled in every APEX image, so their packages
work with `apex search` and `apex install` without extra setup. For software from
a Fedora COPR, enable the project once and then use the normal commands:

```bash
sudo apex repo enable-copr OWNER/PROJECT
apex search PACKAGE
sudo apex install PACKAGE
sudo apex repo disable-copr OWNER/PROJECT
```

COPRs are third-party repositories, not Fedora or APEX. Enabling one trusts its
owner to publish RPMs for that repository until it is disabled. Enabling stores
that COPR's signing key in APEX's writable keyring under `/var/lib/apex/pkg`
(the OS keyring is immutable); APEX still verifies every downloaded RPM against
a trusted key and still refuses kernel/core-system replacements in an extension.
Disabling the COPR also removes its key from the APEX keyring.

## Flatpak

`apex install` also speaks Flatpak, chosen by the name you give it:

```bash
sudo apex install org.gimp.GIMP     # reverse-DNS id -> Flatpak (Flathub)
sudo apex install gimp              # plain name     -> RPM (system extension)
sudo apex install ./gimp.rpm        # a path         -> that RPM file
```

The rule is unambiguous rather than clever: Flathub ids are three or more
dot-separated segments each starting with a letter, and no RPM is named that way
(`python3.12` has two segments, `java-1.8.0-openjdk` has segments starting with
digits). The file test runs first, so `org.foo.Bar.rpm` is a file and not a
Flathub id. `apex remove` follows the same rules, and `apex pkg list` shows both.

A Flatpak-only install never rebuilds the extension, so it costs nothing.

`apex update` now updates Flatpak apps too — system-wide and for the invoking
user — because otherwise a machine could report itself fully up to date while
every graphical application on it was months stale. Skip it with
`--skip-flatpak`; a Flathub outage can never fail an OS update.

## Notes

* Flatpak is still the better choice for sandboxed desktop applications, and
  Bazaar is still the graphical store. The RPM side of `apex install` is for
  what Flatpak is a poor fit for: CLI tools, libraries, headers, drivers'
  userspace — anything that must exist in `/usr`.
* Set `APEX_PKG_FORMAT=tree` to build an uncompressed directory extension
  instead of squashfs. The engine falls back to this automatically if
  `mksquashfs` is unavailable.
* State lives in `/var/lib/apex/pkg` (`requested`, `state.json`, `local/` with
  the cached local RPM files and their trust markers, and a one-generation
  rollback copy of all of it). The extension itself is
  `/var/lib/extensions/apex-user.raw`.
* `state.json` records `local_files` and `unsigned_accepted` so provenance
  survives a reboot and is not something only the person who typed the command
  knows.

## Browsers — two shipped, one default

Both editions ship **Firefox** (RPM, in `core`) and **Zen Browser** (Flatpak,
installed on first boot).

**Firefox is the default and stays the default.** Zen is a Firefox fork with an
opinionated interface — vertical tabs, workspaces, a compact chrome — and someone
who dislikes it should not have to undo a choice the image made for them. So Zen
is installed and discoverable in the launcher, and `files/desktop/xdg/mimeapps.list`
keeps `x-scheme-handler/http` and `https` pointed at `firefox.desktop`.

The build asserts that, because it is not self-maintaining: a Flatpak's exported
`.desktop` can win the handler race depending on XDG data directory ordering, so
without the check an image could silently change every user's default browser.

### Why Zen is a Flatpak

Zen is not in Fedora's repositories and ships no Fedora RPM. The alternatives
are a tarball in `/opt` or an AppImage, and both would need APEX to write and
maintain its own updater to keep "always the latest stable" true.

`apex install ./Thing.AppImage` exists now, and it does **not** change that
answer. It declines the updater rather than writing one: an installed AppImage
is pinned to the bytes that were installed, which is exactly the property Zen
must not have. Zen stays a Flatpak. See *Installing an AppImage* above.

As a Flatpak it needs none: `apex update` already runs
`flatpak update --system` (`cmd_flatpak_upgrade` in `apex-pkg`), so Zen tracks
latest stable through the update path that already exists.

It installs at **first boot**, not at build time, for the same reason the Flathub
remote does — `flatpak install` needs a running system, and bootc seeds `/var`
once and never updates it. `apex-flatpak-preinstall.service` runs after
`apex-flathub-setup.service`, is idempotent, and stamps only on success so a
first boot without network retries on the next one.

### Making Zen your default, per machine

A per-user choice, never an image one:

```sh
xdg-settings set default-web-browser app.zen_browser.zen.desktop
```

### Moving a Firefox profile into Zen

Zen reads a Firefox profile directly — same Gecko, same layout — but there is one
trap. Zen's **application** version is its own (`1.21.16b`), not the Gecko
version it is built on (`154.0.1`). Gecko's downgrade protection compares the
*application* version in `compatibility.ini`, so a profile last used by Firefox
153 looks like a downgrade to Zen 1.21 no matter how new its Gecko is, and Zen
opens with *"You've launched an older version of Zen Browser"*.

Copy the profile, then **delete `compatibility.ini` from the copy.** Zen
regenerates it and runs its normal profile-upgrade path. Do not delete the
databases, and do not do any of this while the source browser is running.

## Desktop AI apps — shipped with the system

**ChatGPT** and **Claude Desktop** are part of APEX-OS, not add-ons. Both are in
the image (stage `5a-aiapps` in `Containerfile.core`), both are on a fresh
install, and both arrive on an existing machine through a normal
`sudo apex update`. There is no separate install step and nothing to download by
hand.

| | source | how it is installed | where it lands |
|---|---|---|---|
| ChatGPT | OpenAI's rpm-md repo (`persistent.oaistatic.com`) | `dnf5` from the vendor rpm | `/usr/lib/chatgpt`, `/usr/bin/chatgpt` |
| Claude Desktop | Anthropic's apt repo (`downloads.claude.ai`) | deb unpacked into `/usr` | `/usr/lib/claude-desktop`, `/usr/bin/claude-desktop` |

Anthropic publishes no rpm, which is why the deb is unpacked rather than
installed — and unpacking is also what keeps its maintainer script from running.
ChatGPT goes through `dnf` on purpose: `apex-pkg` decides whether something is
image-owned by asking the system rpmdb, so an rpm-installed ChatGPT makes
`apex install chatgpt` refuse to shadow it. Unpacking it would have left that
guard blind to 442 MB of application.

### They do not update themselves

**A version bump is an image rebuild.** Both vendors package for mutable
distributions, where installing the app also subscribes the machine to the
vendor's repository — OpenAI's rpm ships `/etc/yum.repos.d/chatgpt.repo` with
`enabled=1`, and Anthropic's `postinst` writes an apt source and an
unattended-upgrades snippet. The build removes the first and never runs the
second, and asserts both.

That is not tidiness. `/usr` is read-only, so neither updater could ever
succeed; but `apex-pkg` builds user system extensions with `dnf` against the
**host's** repo set, so an enabled vendor repo would turn `apex install chatgpt`
into a newer build layered into an extension that shadows the image's own
`/usr`. A self-update through a side channel, which is exactly what shipping
these apps in the image is meant to prevent.

If you find `/etc/yum.repos.d/chatgpt.repo` on a machine, it was put there by a
hand-install of the vendor rpm, not by an APEX image.

### Scheme handlers, and the one surprise

`claude://` opens Claude Desktop and `codex://` opens ChatGPT — note that
ChatGPT's scheme is `codex`, not `chatgpt`. Both are asserted at build time by
reading them back out of `mimeinfo.cache`, because an entry on disk that never
reached that cache is not a registered handler.

ChatGPT's desktop entry also registers `x-scheme-handler/http` and `https` for
itself, so it appears in the "Open With" list for any web link. It does **not**
become the default browser: `files/desktop/xdg/mimeapps.list` keeps http and
https pointed at `firefox.desktop`, and the build asserts that.

Both apps are Electron, and Electron defaults to X11. `/etc/environment` sets
`ELECTRON_OZONE_PLATFORM_HINT=auto` so they run as native Wayland clients under
Hyprland instead of going through XWayland.
