# unit: kernel-build — APEX builds its own kernel

**Status: the decisive measurement was done last round. This round found why the
first end-to-end build died, fixed it, closed the last reproducibility hole, and
built the mechanism that carries the security obligation. The rebuilt kernel
tier was IN FLIGHT when this line was written — see §"Where the build got to".**

Repo `apex-os`, branch `task/kernel-build`, worktree
`/var/tmp/apex-work/wt-kernel-build`.
Merged `origin/roadmap/v2.2` (`4c2478fc`) in cleanly at `6150e25c` — no
conflicts, and the v2.2 delta touched none of this unit's files.
Not landed. No PR.

Commits this round (all pushed):

```
204035e5 feat(kernel): notice when the pin stops being current, because
         nothing else will
<docs>   docs(update-cost): the kernel's recurring price, and the CI question
         it cannot answer
<ev>     docs(evidence): what the first two end-to-end runs of the kernel tier
         found
a5ce7437 build(kernel): pin the builder base by digest, because the tag moved
         mid-unit
0891c6fc fix(kernel): rpmbuild deletes the tree the gate has to read
cb262685 test(kernel): derive the pin checker's locals instead of listing them
         by hand
6150e25c Merge origin/roadmap/v2.2 into task/kernel-build
```

Evidence: `ROADMAP/evidence/kernel-build-20260920.md` (tracked, on the branch),
§4 added this round.

---

## THE HEADLINE IS UNCHANGED AND STILL HOLDS: dwarves 1.32 fixes it

| reading | `scx_bpf_*` examined | **untagged** | `bpf_kfunc` DECL_TAGs |
|---|---|---|---|
| the shipped kernel's own `.BTF` | 68 | **18** | 288 |
| pahole **1.30** re-run on that DWARF | 68 | **18** | 288 |
| pahole **1.32** re-run on the same DWARF | 68 | **0** | **308** |

Reproduced this round from an independent implementation (`kernel/btf-xcheck.sh`,
sh + bpftool) to the digit. Reproduce in ~36 s + a 1.3 GB download:
`kernel/research/run-ab.sh`.

---

## What this round found

### 1. The first build's failure was the harness, not the kernel

The 2026-09-20 08:21 run **compiled the kernel successfully in 49m14s** and died
one second later. rpm ≥ 4.20 runs an `Executing(rmbuild)` phase after a
*successful* build that deletes `%{buildsubdir}` — the whole source tree,
`vmlinux` included — so the BTF gate had nothing to read.

Measured on `fedora:43` / rpm 6.0.2 with a throwaway spec: default leaves **0**
vmlinux under `BUILD`, `--noclean` leaves **1**. The spec defines no `%clean`,
so the flag has no other effect. **Do not "tidy up" that flag.**

The prior card's prediction — "the most likely break is `%autopatch` applying
the BORE patch" — was **wrong**. Every source verified, the patch applied, the
kernel linked.

### 2. The builder base moved during this unit's own work

`fedora:43` was the one input in `kernel.pin` still named by a floating tag, and
it moved the same day: the 08:21 build pulled base blob `ce241f1b…`, the evening
rerun pulled `a71bf9b8…` from an image the registry rebuilt at 05:47:58Z. Both
`FROM` lines and `BUILDER_BASE` now name `sha256:84325c67…`.

**What it does not pin, so nobody reads more into it:** the digest freezes the
base *layer*, not the `dnf5 -y install gcc …` on top of it, which still resolves
against live Fedora repos. GCC can still move. That is tolerable because the
compiler can change the kernel *binary* but cannot change whether the kfunc BTF
is correct — that is pahole's, pinned by NVR **and** sha256 and asserted twice.

### 3. An assumption the prior unit flagged is now a measurement

`kernel-btf-scx`'s evidence recorded a limit on its own reader: *"the `_impl`
skip ASSUMES `_impl` means pre-strip twin."* Counting the three A/B blobs:

| blob | stage | `scx_bpf_*` FUNCs | of which `*_impl` |
|---|---|---|---|
| pahole 1.30 output | **pre**-`resolve_btfids` | 68 | **0** |
| pahole 1.32 output | **pre**-`resolve_btfids` | 68 | **0** |
| the shipped `.BTF` | **post**-`resolve_btfids` | 97 | **29** |

pahole emits no `*_impl` name at all. All 29 appear only after
`resolve_btfids`, which creates them — one per kfunc whose implicit argument it
stripped. So the exclusion is right for the right reason, and **any reader run
over a vmlinux's `.BTF` must make it**, because a vmlinux is always read
post-strip.

### 4. Two readers now, because the gate had never passed on anything

`apex-kernel-btf-gate` has **never returned 0** on any BTF in existence here —
checked this round against four inputs, all exit 1. Every test of it was a
negative one. So `kernel/btf-xcheck.sh` (sh + bpftool, no shared code) runs
**first** and prints, asking the root-cause question (did pahole emit the tag?)
where the gate asks the symptom question (is the implicit arg still there?). If
the gate passes and xcheck does not, **the build fails on the disagreement**.

Validated against every BTF on hand, failing both ways — including a valid BTF
containing no `scx_bpf_*` at all, which it refuses. Zero untagged out of zero
examined is not a passing kernel.

> **A trap for whoever reads the gate's output next.** Running
> `apex-kernel-btf-gate` over `ab/out/btf-132.bin` says *47 of 68 carry the
> implicit arg* and exits 1. That is **not** a contradiction of the table above
> and not a bug: `btf-132.bin` is pahole output, **pre**-`resolve_btfids`, so
> nothing has been stripped yet. The gate is only meaningful on a real
> vmlinux's `.BTF` section. Both readers in the build run on exactly that.

### 5. The security obligation now has a mechanism, and it is verified in CI

`tests/check-kernel-drift.sh` + `.github/workflows/kernel-drift.yml`.
**It ran for real in GitHub Actions** (run `35512436299`, 15 s, success, on the
branch's own push trigger) — not merely syntax-checked. Three outcomes: `0` no
drift, `1` drift, `2` a lookup could not be performed (**not** a pass).
Mutation-tested with five mutants, all caught with the right code and message.

Both of its lookups were **wrong when first written**, and both were caught by
checking them against a second source rather than by reading them:

* GitHub's `/tags` API returns tags in its own order — page one of
  `CachyOS/linux` is 100 `v5.18-rc*` tags with no `cachyos-*` among them. Written
  the obvious way that reads as "no newer kernel". Now `git ls-remote --tags`:
  all 148 in 1.1 s, no token, no rate limit.
* **A live false positive:** Fedora's mdapi answers `version: 1.32` for dwarves
  and, three fields later, `repo: updates-testing`. The first version of the
  script reported *"1.32 has REACHED stable, you can relax the pin"*. Bodhi says
  `status=testing`, `date_stable=None`. It now asks bodhi.

**Current real state, which answers two standing questions:**
`cachyos-7.2.6-1` **is** the newest stable tag in the 7.2 series (of 101), and
`dwarves-1.32-1.fc43` is **still in updates-testing** (stable is 1.30-1.fc43),
so **the koji NVR pin cannot be relaxed yet.**

---

## Things checked this round rather than inherited

* **Signing is untouched.** The diff of `Containerfile.core` on this branch
  against `roadmap/v2.2`, grepped for `sbsign|kernel-signed|secureboot|
  sign-file|MODULE_SIG`, is **empty**. `/usr/share/apex-os/secureboot/
  kernel-signed` stays honest with no edit, and `Containerfile.release`'s
  assertion of it needs no change.
* **The four config assertions will pass.** Read out of the pinned config
  (`/var/lab-scratch/kernel-build/src/config`): `CONFIG_SCHED_CLASS_EXT=y`,
  `CONFIG_DEBUG_INFO_BTF=y`, `CONFIG_EFI_STUB=y`, and
  `# CONFIG_MODULE_ALLOW_BTF_MISMATCH is not set`. No `scripts/config` line in
  the spec enables it. **So the dispatch's "do not enable
  `MODULE_ALLOW_BTF_MISMATCH`" needs no exception — no module required it.**
* **`CONFIG_PAHOLE_VERSION=131` ships in CachyOS's config and that is fine.**
  The symbol has no prompt, so `make olddefconfig` discards the stored value and
  recomputes it from the buildroot's pahole. The build's `=132` assertion is
  precisely what proves the pinned pahole was the one used.
* **`_build_minimal 0`** in the spec — the full module set. That is where the
  ~100 GB tree comes from.
* **`check-containerfile-assertions.sh`** over base+core+kernel: **197 checked,
  0 failed, 0 inert.**
* **The "syntax error at `Containerfile.core:1572`" in the prior round's
  `runs.txt` is not real.** It was the ad-hoc checker joining `\`-continuations
  with a newline; podman joins them into one line. Modelled correctly, all three
  Containerfiles are clean (kernel 6 blocks, core 46, base 56, 0 errors).

---

## Where the build got to

`podman build -f Containerfile.kernel -t apex-kernel:local .` as transient unit
**`kb-build2`**, log `/var/lab-scratch/kernel-build/logs/build2.log`, started
2026-09-20 ~20:56 AWST with `rpmbuild` entering at 12:59:03Z.

**IN FLIGHT when this card was written. Read the tail of that log before
assuming anything about the outcome.** Confirmed in it already: the pinned
dwarves RPMs verified by sha256, `pahole in buildroot: v1.32`, all three pinned
sources verified, the gate binary compiled into the builder stage, and
`rpmbuild` running with `--noclean`.

Tagged `apex-kernel:local` deliberately — that is `Containerfile.core`'s default
`APEX_KERNEL_IMAGE`, so a passing image feeds a core build with no `--build-arg`.

**Provenance if it passes:** neither `kernel/kernel.pin` nor
`Containerfile.kernel` has changed since `a5ce7437`, so that is the commit the
image was built from. `Containerfile.core` `cmp`s the build context's
`kernel.pin` against `/manifest/kernel.pin`, so **any edit to either file means
the image must be rebuilt before it can feed core.**

---

## NEXT — in priority order, for a stranger

1. **Finish reading the build out, then run the contract test.** If the log ends
   in `EXIT=0`, the gate returned 0 on a real kernel for the first time ever.
   Then, in this order:
   ```sh
   cid=$(podman create localhost/apex-kernel:local /x)
   podman cp "$cid:/rpms" /var/lab-scratch/kernel-rpms
   podman cp "$cid:/manifest" /var/lab-scratch/kernel-manifest && podman rm "$cid"
   RPMS=/var/lab-scratch/kernel-rpms ./tests/check-kernel-contract.sh
   ```
   (`FROM scratch` has no shell — `podman create` + `podman cp`, never
   `podman run`.) Ten minutes, and it is what stands between this and a
   50-minute core build failing at its `rpm -q` gate. Then run BOTH readers over
   the extracted `/manifest/vmlinux.btf` from OUTSIDE the container
   (`apexd/target/release/apex-kernel-btf-gate` is already built on the host,
   and `kernel/btf-xcheck.sh` needs a fedora container for bpftool), and append
   the timings, `podman image inspect --format '{{.Size}}'` and the RPM sizes to
   evidence §4.5.

   If it **FAILS**: do not `podman build` again — the layer is discarded and you
   lose the compile. Re-run the final RUN's body in a *named* container off the
   cached sources layer (`podman run --name kb-persist <layer-id> bash -c '…'`)
   so the tree survives for inspection. xcheck's printed names tell you which
   story it is: names listed = the kernel genuinely has the defect (and that is
   the loud result Andre needs immediately, not something to engineer past);
   xcheck clean but the gate red = the readers disagree and one is wrong.

2. **THE CI DECISION, AND IT IS ANDRE'S, NOT AN AGENT'S.** Nothing builds the
   kernel image in CI. `build-image.yml` needs a `kernel` job before `core`
   passing `--build-arg APEX_KERNEL_IMAGE=…@sha256:…`. The decision is sharper
   than "slower": **the build tree is ~100 GB and a hosted GitHub runner has
   14 GB**, so it cannot run on one at all. The two options and their real costs
   are written up as a table in `docs/update-cost.md` ("The CI question this tier
   cannot answer for itself") — a self-hosted runner on katana (check its `/var`
   has ≥120 GB free first; a self-hosted runner executing PR code is its own
   security decision), or `_build_minimal 1` + a `modprobed.db`, which **changes
   what hardware APEX boots on** and is a product decision. Until one is chosen,
   a release depends on somebody's laptop. Trap when writing the job: a workflow
   `run:` block is capped at **21,000 characters**.

3. **Relax the dwarves pin when 1.32 reaches stable — and only then.** It is
   **still `testing`** as of 2026-09-20 (checked against bodhi, not mdapi). The
   drift workflow will say when it moves; that is literally one of the things it
   watches. The *gate* is what makes the relaxation safe rather than a hope.

4. **A wart in the two-reader wiring, small and worth fixing.** `btf-xcheck`
   exits **2** for "unreadable" and **1** for "defect found", but
   `Containerfile.kernel` collapses both into `XCHECK=fail`, so an unreadable
   BTF would be reported as "the two readers disagree". Misleading, not unsafe —
   it still fails the build. Distinguish the two exits.

5. **Report it upstream. Still nobody has.** This is now a strong report: pahole
   1.30 drops the `bpf_kfunc` DECL_TAG for 18 of 68 `scx_bpf_*` kfuncs on
   Fedora's own 7.2.6 kernel, 1.32 drops none, with a reproduction script
   (`kernel/research/run-ab.sh`) and a second independent reader agreeing.
   Both the COPR maintainer and the dwarves list want this.

6. **The residual reproducibility hole, named so it is not rediscovered.** The
   builder base is digest-pinned, but the `dnf5 install gcc …` transaction on top
   of it is not — GCC can move between two builds of an unchanged pin. Closing it
   needs a pinned repository snapshot. `cc=` in `/manifest/kernel-build.txt`
   makes the drift visible meanwhile. Same applies to `cargo rust` in the
   gate-builder stage.

---

## For the other live units

**`sdboot-image` (they own the boot path).** Unchanged in shape from the prior
card, and nothing of theirs was touched. What the kernel exposes for a UKI:

* `/usr/lib/modules/<kver>/vmlinuz` — `CONFIG_EFI_STUB=y` is **asserted at build
  time** (and confirmed present in the pinned config), so it is a valid PE and
  can be a UKI stub.
* `/usr/lib/modules/<kver>/initramfs.img` — still from the dracut RUN that
  follows the kernel install, which is **theirs**.
* `/usr/lib/modules/<kver>/config` and `System.map`.
* `/usr/lib/apex-kver` — unchanged, still the cross-tier kver contract.
* `/usr/share/apex-os/kernel/build.txt` and `kernel.pin` — source NVR, toolchain,
  `kver`, `cc`, and `btf_scx=usable`. Useful in a UKI's own provenance.
* `/manifest/vmlinux.btf` in the kernel image (not the OS image) — the BTF blob
  the gate passed, to verify independently.

No `vmlinux` ships and none should: 537 MB, and nothing on a user's machine
needs it.

**The signing question, which is the part that concerns them.** Signing is
*unchanged* by this work and that is now checked, not assumed (empty diff, see
above). But **once a UKI exists, the MOK signature belongs on the UKI, not only
on the inner vmlinuz**: shim verifies the PE it loads, and that PE becomes the
UKI. Stage 1b signing the inner image with nothing signing the UKI is a machine
that will not boot under Secure Boot, with every existing assertion still green.
That ordering — build → UKI assembly → sign the UKI — is worth settling between
us before either lands.

**`luks-installer` — L-002's "initramfs keymap is always `us`".** Flagged, not
fixed; it is theirs. Owning the kernel does **not** change it and nobody should
wait on this unit: the keymap is not a kernel build option. The fix is in the
dracut invocation already in `Containerfile.core` (dracut's `i18n` module ships
only the host's keymap unless `i18n_install_all="yes"`). What owning the kernel
*does* change is the other half — under systemd-boot the cmdline becomes part of
the signed UKI, so writing a keymap karg has to happen at image or UKI assembly
time rather than as a mutable karg.

---

## Cost, stated plainly

* **Build time:** ~45–49 min at `-j12` on this 16-core box. The pahole step is
  **not** the cost — 5.3 s and ~1 GB RSS over a 537 MB vmlinux, measured.
* **Disk:** a **~100 GB** build tree, measured (`/var` 552 → 453 GB free). That
  figure, not the CPU, is what decides the CI question.
* **Image size:** the kernel image is RPMs only (`FROM scratch`), so it adds
  nothing to a user's download. `core` installs the same kernel from a different
  source — **the fleet's ~5 GB update cost is unchanged.**
* **Security updates — the obligation, and it is permanent.** When CachyOS tags
  a new `cachyos-7.2.*`, somebody must bump `KERNEL_TAG` and its sha256. Nothing
  happens on its own, and **an unbumped pin is a kernel that silently stops
  receiving security fixes while every gate stays green** — the sha256 verifies
  against the old tarball, the BTF gate is happy with the old BTF. Each bump
  costs a ~45-minute kernel rebuild plus a core rebuild (~5 GB to the fleet),
  per security update. `kernel-drift.yml` is the mechanism that replaces
  remembering; it is running and verified.

## Machines

**Nothing was run on katana or the L16's system state.** No image installed, no
deployment touched, no `apex` command run against the live system, no boot path,
ESP, NVRAM or bootloader touched, and **no `bootc install` of any kind**. All
work is container builds plus `curl`/`git ls-remote`, under `/var/lab-scratch`
(never `/tmp`, which is a 15 GB tmpfs on this machine). The build ran under
`systemd-inhibit --what=sleep:idle` because hypridle suspends this box after 15
minutes idle.
