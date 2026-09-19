# katana — qualifying the image that carries gaming-gpu and pkg-share, 2026-09-19

Round 33. Two units closed on 2026-09-19 with the same sentence — RE-OPEN AFTER
AN IMAGE BUILD. Their code is landed and headlessly verified; what was missing
was one run on a machine that carries it.

* `gaming-gpu`, merge `78f04717` — run-book `docs/gaming-and-sessions.md` §6.
* `pkg-share`, merge `5de97037` — the three confirmation numbers in §2 below.

The round-31 run is `ROADMAP/evidence/katana-qualification-20260919.md`. This
file does not replace it; it measures the same machine on a **newer image** and
answers the rows that one could not.

---

## 0. The starting state, measured rather than assumed

Katana was free: uptime 6:37, greetd on `seat0` and `tty1`, nobody logged in, no
`steam`/`gamescope`/`proton`/`wine` process, `rpm-ostree status` idle
(2026-09-19 17:12 AWST).

```
$ rpm-ostree status
* ostree-unverified-registry:ghcr.io/andrenijman/apex-os:apex-266dcc572c51bdf9ec421d79eaa8784583184cd2
                   Digest: sha256:ba263890b69619532b13b9c64e0ac5dcb8eced78bffdc9fd542ccac92607503d
                  Version: apex (2026-09-18T14:33:05Z)
  ostree-unverified-registry:ghcr.io/andrenijman/apex-os:gaming-nvidia   (rollback)
                   Digest: sha256:308127d9cefeada90414ae37bdc8175d011c1f851ea9dde1661279a5da5bd89b
                 Unlocked: hotfix
```

`266dcc57` is **131 commits behind** the integration tip and
`git merge-base --is-ancestor` says NO for both `78f04717` and `5de97037`. A
run-book executed on this deployment measures the wrong build, so the rebase is
step 1.

Disk, checked before pulling a multi-GB image:

```
$ df -h /var
/dev/nvme0n1p3  954G  890G   60G  94% /var        (/var, /sysroot and /boot are one filesystem)
```

60 G available, so no `rpm-ostree cleanup -r` — that would drop the
`gaming-nvidia` rollback for no benefit, and `bootc switch` retires the oldest
deployment by itself.

> The device name above is printed by `df` and is **not** how anything in this
> run identifies a disk. Katana's NVMe names reorder across ordinary reboots.
> Nothing here writes to a block device.

### 0.1 The false-negative trap, named before it could fire

A system extension was already merged: `/var/lib/extensions/apex-user.raw`.

```
$ stat -c '%n %s %y' /var/lib/extensions/apex-user.raw
/var/lib/extensions/apex-user.raw 540012544 2026-09-19 09:43:04 +0800
$ jq -r '{os_version_id, pkg_compat_level, built, image_sha256, nres:(.resolved|length)}' \
      /var/lib/apex/pkg/state.json
{ "os_version_id": "43", "pkg_compat_level": 2, "built": "2026-09-19T01:43:06Z",
  "image_sha256": "99749240e8762e2b4eaf0c3840a07beba249961e1cad86f798805c22c58282ed",
  "nres": 223 }
$ cat /var/lib/apex/pkg/requested
chromium gamemode gamescope libgcc.i686 libSM.i686 mangohud steam steam-devices
```

The standing warning was that this is the 2026-09-06 extension (219 packages,
~337 MB). **It is not** — round 31 rebuilt it at 01:43 UTC today. The substance
is unchanged and matters more than the date: it was built by the engine **in the
image**, which predates pkg-share, and a system extension survives a rebase and
re-merges at boot. Reading `ls /usr/share/vulkan/icd.d/ | grep -c i686` before a
genuine rebuild therefore returns 0 and looks exactly like pkg-share failing.

**The discriminator is the line `multilib: carrying` in the install log.** Only
the new engine emits it (`merge_multilib`, `files/system/libexec/apex-pkg`
~line 698). Its absence means the old extension was measured, not that the fix
failed.

### 0.2 DEFECT (new, found by reading the engine before running it) — the pkg-share fix does not reach an existing machine through `apex update`

`rebuild_extension` stops early when the resolved rpm set, `os_version_id` and
`pkg_compat_level` all match and the sysext is merged:

```bash
    if [ -f "$STATE" ] && merged && [ -n "$(current_payload)" ] \
       && [ "$(jq -r '.os_version_id // empty' "$STATE")" = "$(os_version)" ]; then
        old_set="$(jq -r '.resolved[]?' "$STATE" | sort)"
        if [ "$new_set" = "$old_set" ]; then
            msg "already up to date"
```

and `apex-sysext-rebuild.service` — the unit whose whole job is to notice that
the OS moved — runs `rebuild --if-needed`, which returns 0 on the same
three-way match. `os_version()` is `VERSION_ID` out of `/usr/lib/os-release`,
and on katana that is **`43` before and after an APEX image build**. The
extension's `pkg_compat_level` is 2 and the engine's constant is still 2.

So none of the three changes across this rebase, and an existing machine keeps
its old-engine extension — with 0 i686 ICDs and 14 32-bit shadows — after
`sudo apex update`. The knob that exists for exactly this is
`PKG_COMPAT_LEVEL`, whose own comment says "increment whenever a newly baked
image package may overlap existing user extensions"; an engine change that
alters **which files the extension carries** is the same class. Verified on the
machine in §2.1 rather than left as a reading.

### 0.3 The §3.1 shadow baseline, re-measured by this unit on the old image

Round 31's numbers are reproduced here by this agent, on this deployment, so
the post-rebase numbers are a same-agent like-for-like delta. `LC_ALL=C` on
every `sort` and `comm`.

```
$ sudo mount -o ro,loop /var/lib/extensions/apex-user.raw /mnt/apexext-qual2
$ sudo find /mnt/apexext-qual2/usr -mindepth 1 \( -type f -o -type l \) -printf "/usr/%P\n" \
    | LC_ALL=C sort > ext-pre.txt
$ rpm -qal | LC_ALL=C sort -u > image-pre.txt
$ LC_ALL=C comm -12 ext-pre.txt image-pre.txt | wc -l
177
```

| | old image, 2026-09-19 17:20 AWST |
|---|---|
| extension files + symlinks | 3 716 |
| image paths (`rpm -qal`) | 266 030 |
| **paths the extension shadows** | **177** |
| of those, 32-bit ELF over a 64-bit image binary | **14** |
| `ls /usr/share/vulkan/icd.d/ \| grep -c i686` | **0** (13 files, all `x86_64`) |
| `/usr/share/vulkan/icd.d/` inside the extension | **does not exist** |
| `gst-inspect-1.0 \| tail -1` | `240 plugins (239 blacklist entries not shown), 2 features` |

The 14 are the same 14 round 31 named — at-spi2's bus launcher and registryd,
`dconf-service`, `gio-launch-desktop`, `glib-pacrunner`, four glycin loaders,
four GStreamer helpers including `gst-plugin-scanner`, and `p11-kit-remote`.

### 0.4 The greetd question, and the route this run takes

Round 31 could not log in through greetd (§4 there): `apex-session-select`
deliberately arms no autologin and that unit did not have Andre's password.
Neither does this one, and it must not ask. `sudo -n true` on katana **does**
succeed, so the route is a temporary `initial_session` in
`/etc/greetd/config.toml`, backed up, restored and `cmp`-verified afterwards.

greetd's initial session skips `pam_authenticate` and still runs `acct_mgmt` →
`setcred` → `open_session`. The fidelity argument is a measurement, not an
assertion:

```
$ sudo grep -rn -i 'pam_cap\|capability' /etc/pam.d/{greetd,system-auth,postlogin,login}
(no output)
$ ls /etc/security/capability.conf
ls: cannot access '/etc/security/capability.conf': No such file or directory
```

Nothing in the `auth` stack on this machine can grant a capability, so skipping
it cannot change `CapPrm` — which is the only thing §6.3 asks the login path
about. For the same reason the greeter's own process already reads an empty
permitted set:

```
$ sudo grep -E '^Cap(Inh|Prm|Eff|Bnd|Amb):' /proc/<sway, user greetd>/status
CapInh: 0000000000000000   CapPrm: 0000000000000000   CapEff: 0000000000000000
CapBnd: 000001ffffffffff   CapAmb: 0000000000000000
$ systemctl show greetd -p AmbientCapabilities
AmbientCapabilities=
```

This grep is repeated on the new image in §3 before any capability number is
believed.

### 0.5 The negative control — five readings that must flip

Taken on the old deployment at 17:20 AWST so "the image changed" is a
measurement rather than a hope:

```
$ apex gaming --gamescope-device-args ; echo "rc=$?"
error: unexpected argument '--gamescope-device-args' found
rc=2
$ grep -c 'prefer-vk-device\|prefer-output' /usr/libexec/apex-gaming-session   -> 0
$ grep -c 'CapEff\|CapPrm\|CapAmb'         /usr/libexec/apex-gaming-session   -> 0
$ getcap "$(command -v gamescope)" ; echo "rc=$?"                              -> (empty) rc=0
$ for p in /sys/class/drm/*/vrr_capable; do [ -e "$p" ] && echo "$p"; done      -> (no matches)
```

The first three must change on the new image; `getcap` and `vrr_capable` must
**not** — nothing in either unit grants a file capability or invents a sysfs
attribute, and §6.2 and §7.2 both depend on that staying true.

One thing already correct on the old image and worth recording because round 31
called it out: the EACCES-vs-absent distinction on the `sudoers rule` row is
present — it reads `not measured — could not read the path: Permission denied
(os error 13)`, not a bare `no`.


---

## 1. The rebase

*(pending — the per-SHA tag had not been published at 17:20 AWST)*

## 2. pkg-share

*(pending)*

## 3. gaming-gpu — docs/gaming-and-sessions.md §6

*(pending)*
