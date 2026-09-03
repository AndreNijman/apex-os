# Installing software on APEX-OS

```bash
sudo apex install android-tools           # a package from the repositories
sudo apex install ~/Downloads/vendor.rpm  # an .rpm file you downloaded
sudo apex remove android-tools
apex search wireshark
apex pkg list
```

That is the whole interface. It works for ordinary Fedora packages — CLI tools,
libraries, development toolchains, GUI applications, fonts, services — and for a
local `.rpm` file, and it does **not** stop the OS from updating.

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
| `apex remove PKG…` | remove packages (a local one by its package name) |
| `apex search TERM…` | search the repositories |
| `apex repo list` | list enabled and disabled RPM repositories |
| `apex repo enable-copr OWNER/PROJECT` | opt into a Fedora COPR for search/install/upgrade |
| `apex repo disable-copr OWNER/PROJECT` | disable an opted-in COPR |
| `apex pkg list` | requested packages and dependency count |
| `apex pkg status` | extension state, what it was built for, whether merged |
| `apex pkg upgrade` | re-resolve everything against the repositories |
| `apex pkg rebuild [--if-needed]` | rebuild for the running OS version |
| `apex pkg rollback` | restore the previous extension |
| `apex pkg verify` | check the extension against its recorded checksum |
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
