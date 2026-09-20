# unit: kernel-build — APEX builds its own kernel

**Status: the decisive measurement is DONE and PUSHED. The build tier, the pin,
the gate and the core wiring are written and pushed. The end-to-end kernel
compile was RUNNING when this card was written — see §"Where the build got to".**

Repo `apex-os`, branch `task/kernel-build`, worktree
`/var/tmp/apex-work/wt-kernel-build`, from `roadmap/v2.2` (`223a255a`).
Not merged. Not landed. No PR.

Commits (push after each):

```
175459d3 feat(kernel): dwarves 1.32 fixes the kfunc BTF defect, measured on a
         real kernel
b4b4b1bb feat(kernel): build the kernel in its own tier, and refuse one with
         the defect
aa9e582b feat(core): install APEX's own kernel, and refuse one whose BTF was
         never checked
```

Evidence: `ROADMAP/evidence/kernel-build-20260920.md` (tracked, on the branch).

---

## THE HEADLINE: dwarves 1.32 fixes it. Measured.

The prior round left 1.32 as **"the candidate, not the answer"**. It is now the
answer, and the number that matters is:

| reading | `scx_bpf_*` examined | **untagged** | `bpf_kfunc` DECL_TAGs |
|---|---|---|---|
| the shipped kernel's own `.BTF` | 68 | **18** | 288 |
| pahole **1.30** re-run on that kernel's DWARF | 68 | **18** | 288 |
| pahole **1.32** re-run on the same DWARF | 68 | **0** | **308** |

All 18 fixed, none newly broken. `still broken: []`, `NEW in 1.32: []`.

**The control is the load-bearing row, not the 1.32 row.** A debuginfo vmlinux
has already been through `resolve_btfids`, which patches and sorts `.BTF_ids`,
and `readelf -S` shows this one has **no `.rela.BTF_ids`** — so re-running
pahole on it could have found no kfuncs at all and produced a meaningless zero.
It does not: 1.30 reproduces the shipped kernel's **exact 18 names** and its
DECL_TAG count to the digit. Only then does the 1.32 row mean anything.

Reproduce in ~90 s + a 1.3 GB download: `kernel/research/run-ab.sh`.

Two further things that came out of it:

* **The tag reading and the argument reading name the same 18.** That is the
  first direct confirmation of the causal chain the prior round *inferred*
  (no DECL_TAG → `btf2btf()` never strips → `libbpf` refuses).
* **Correction to the prior evidence**: `scx_bpf_cid*` / `scx_bpf_cidperf*` were
  recorded as CachyOS-only patches. They are in Fedora's stock 7.2.6 too — they
  are among the 18.

---

## What is on the branch

**`Containerfile.kernel`** — its own tier, published as its own image, consumed
by `core` by digest, exactly as `base` consumes `core`. Reasoning is in the
file header and in `docs/update-cost.md`'s new "fourth tier" section; the short
version is that **CI builds core with no layer cache**, so a kernel compile
there is paid on every core rebuild, and worse, every unrelated core rebuild
would produce a *different kernel binary*.

**`kernel/kernel.pin`** — every input, each with a sha256 and a permanent URL.
The COPR spec fetches two of its three sources from `master` branch URLs, so the
upstream build is not reproducible even to itself; those are pinned to commits.
The load-bearing line is `DWARVES_NVR=1.32-1.fc43` — upstream's
`BuildRequires: dwarves` is unversioned, which is exactly how this shipped three
times.

**`apexd/apexd-core/src/bin/apex-kernel-btf-gate.rs`** — reuses `kernelbtf.rs`
rather than adding a second reader that could disagree with the one
`apex game status` answers from. Exit 0 on `Usable` and nothing else;
`NoSchedExtKfuncs`, `Absent` and `Unreadable` all fail. 5 tests, **4 mutations,
all caught**, restored `cmp`-clean. Validated against real artefacts too: over
Fedora's real BTF it exits 1 and names the same 18.

**`Containerfile.core`** — installs those RPMs, **with no COPR fallback**
(a fallback makes the gate advisory). Two cross-tier contracts: the manifest's
`btf_scx=usable`, and its `kver` checked against what actually landed in the
rpmdb. Both copied to `/usr/share/apex-os/kernel/`.

Gates run: `cargo clippy --locked --workspace --all-targets -- -D warnings`
exit 0 · `cargo test --locked --workspace` **3481 passed, 0 failed** (was 3476)
· `check-containerfile-assertions` over base+core+kernel **195 checked, 0
failed, 0 inert**.

---

## Where the build got to

`podman build -f Containerfile.kernel` was running as transient unit
`kb-build`, log `/var/lab-scratch/kernel-build/logs/build.log`. Everything up
to and including `rpmbuild` started cleanly, and these are confirmed in that log:

* the pinned dwarves RPMs verified by sha256 (`dwarves.rpm: OK`,
  `libdwarves.rpm: OK`) and `pahole in buildroot: v1.32`;
* all three pinned sources verified by sha256;
* the gate binary compiled and copied into the builder stage.

**Read the log's tail before assuming anything about the outcome.**

> **One honesty note for whoever picks this up.** The running build was started
> *before* the last two rounds of edits to `Containerfile.kernel` (the by-name
> RPM assertion, the kver-glob assertion, the manifest fields, the `dist`
> derivation, the labels). The compile and the gate path are byte-identical
> between the two versions, so its BTF verdict is valid — but **the collection
> assertions at the end of that RUN were not exercised by it.** Validate them
> against the produced `/rpms` rather than assuming, or just rebuild.

---

## NEXT — in priority order, for a stranger

1. **Finish or re-run the kernel build and record the result.**
   `podman build -f Containerfile.kernel -t apex-kernel:local .` from the
   worktree root. Run it under
   `systemd-run --user --unit=kb-build --collect systemd-inhibit --what=sleep:idle …`
   — hypridle suspends this machine after 15 minutes idle and that is what
   actually kills long rounds here. **Do not put the build tree in `/tmp`: it is
   a tmpfs on this machine.** Use `/var/lab-scratch` (548 GB).

   If it **PASSES**, that is the positive control the gate has never had (every
   real BTF available today is a *negative* control). Then:
   * pull `/manifest/vmlinux.btf` out with `podman create` + `podman cp` and run
     the host-built gate over it independently;
   * record wall time, `podman image inspect --format '{{.Size}}'`, and the RPM
     sizes;
   * append a §4 to `ROADMAP/evidence/kernel-build-20260920.md` with those, and
     say plainly that end-to-end is now proven rather than inferred.

   If it **FAILS**, read the tail of `/build/rpmbuild.log` first (the
   Containerfile prints 120 lines of it on failure — deliberately, because
   piping rpmbuild to `tail` would have swallowed its exit status). The most
   likely break is `%autopatch` applying the BORE patch against the pinned tag.

2. **Install the RPMs into a scratch `fedora-bootc:43` and check core's
   contract without a core build.** `ls -d /usr/lib/modules/*cachyos*` must
   match, `/usr/lib/modules/<kver>/{vmlinuz,config}` must exist, and
   `rpm -q kernel-cachyos{,-core,-modules,-devel-matched}` must all resolve.
   Ten minutes, and it is what stands between this and a 50-minute core build
   failing at its `rpm -q` gate.

3. **The CI job — NOT WRITTEN, and it needs a decision from Andre first.**
   `.github/workflows/build-image.yml` needs a `kernel` job that runs before
   `core` and passes its digest as `--build-arg APEX_KERNEL_IMAGE=…@sha256:…`.
   `kernel/**` already triggers a core rebuild, which is right — core must
   reinstall — but nothing builds the kernel image yet.

   **The decision:** GitHub's hosted runners are 4 vCPU. This build used 12
   cores; on a hosted runner the honest estimate is **three to four times
   longer**, which puts the kernel tier alone in the 2–3 hour range and may
   exceed the 6-hour job limit when combined. The alternatives are a
   self-hosted runner on katana (20 cores, podman already there) or paying for
   larger runners. That is Andre's call, not an agent's.

   Traps when writing it: a workflow `run:` block is capped at **21,000
   characters** and overflowing surfaces as a jobless failed run.

4. **Relax the dwarves pin once 1.32 reaches Fedora 43 stable.** It is in
   updates-testing today, which is why `kernel.pin` uses koji NVR URLs. Once it
   is stable the pin can become "stable, ≥ 1.32" — and the *gate* is what makes
   that relaxation safe rather than a hope.

5. **Report it upstream.** Still nobody's done it, and this round makes the
   report much stronger than the last one could: it is no longer "some kfuncs
   lose their tag", it is "pahole 1.30 drops the `bpf_kfunc` DECL_TAG for 18 of
   68 `scx_bpf_*` kfuncs on Fedora's own 7.2.6 kernel, 1.32 drops none, here is
   the reproduction script". Both the COPR maintainer and the dwarves list want
   this. `kernel/research/run-ab.sh` *is* the reproducer.

6. **`digest-pin `BUILDER_BASE`.** It is `registry.fedoraproject.org/fedora:43`,
   a floating tag, so GCC can move under the pin. The build now *records* the
   compiler in `/manifest/kernel-build.txt`, which makes drift visible, but
   recording is not pinning.

7. **A drift watcher.** A scheduled workflow that diffs `kernel/kernel.pin`
   against the CachyOS tags API and Fedora's dwarves stable version, and fails
   or opens an issue. See "Security obligation" below — this is the mechanism
   that keeps the obligation from being a thing someone has to remember.

---

## For the other live units

**`sdboot-image` (they own the boot path; nothing of theirs was touched — the
diff of `Containerfile.core`/`Containerfile.apex` between `223a255a` and their
branch tip `9777a30f` is empty, checked rather than assumed).**

What the kernel exposes for a UKI, unchanged in shape from today so nothing of
theirs breaks:

* `/usr/lib/modules/<kver>/vmlinuz` — the kernel image. `CONFIG_EFI_STUB=y` is
  **asserted at build time**, so it is a valid PE and can be a UKI stub.
* `/usr/lib/modules/<kver>/initramfs.img` — still generated by the dracut RUN
  that follows the kernel install, which is **theirs**, not mine.
* `/usr/lib/modules/<kver>/config` and `System.map`.
* `/usr/lib/apex-kver` — unchanged, still the cross-tier kver contract.
* **New:** `/usr/share/apex-os/kernel/build.txt` and `kernel.pin` — source NVR,
  toolchain, `kver`, and `btf_scx=usable`. Useful in a UKI's own provenance.
* **New:** `/manifest/vmlinux.btf` in the kernel image (not in the OS image) —
  the BTF blob the gate passed, if they ever want to verify independently.

**No `vmlinux` ships and none should**: it is 537 MB and nothing on a user's
machine needs it.

**The signing question, which is the part that actually concerns them.**
Signing is *unchanged* by this work: `Containerfile.core` Stage 1b still
`sbsign`s the installed `vmlinuz` with the APEX MOK, and
`/usr/share/apex-os/secureboot/kernel-signed` stays honest with no edit —
`Containerfile.release`'s assertion of it needed no change. But **once a UKI
exists, the MOK signature belongs on the UKI, not (only) on the inner
vmlinuz**: shim verifies the PE it loads, and that PE becomes the UKI. Stage 1b
signing the inner image and nothing signing the UKI would be a machine that
will not boot under Secure Boot, with every existing assertion still green.
That ordering — build → UKI assembly → sign the UKI — is worth settling
between us before either lands.

**`luks-installer` — L-002's "initramfs keymap is always `us`".**
Flagging as asked, **not fixing** it; it is theirs.

Owning the kernel does **not** change this, and it is worth saying so plainly so
nobody waits on this unit for it. The keymap is not a kernel build option. The
fix is in the initramfs, in the dracut invocation that already exists in
`Containerfile.core` right after the kernel install: dracut's `i18n` module
ships only the host's keymap unless `i18n_install_all="yes"` is set, so a
`vconsole.keymap=` karg has nothing to select. What owning the kernel *does*
change is the other half — under systemd-boot the cmdline becomes part of the
signed UKI, so the installer writing a keymap karg has to happen at image or
UKI assembly time rather than as a mutable karg. Worth `luks-installer` and
`sdboot-image` agreeing on that together.

---

## Cost, stated plainly

* **Build time:** the pahole step itself is *not* the cost — 5.3 s and ~1 GB RSS
  over a 537 MB vmlinux, measured. The cost is the kernel compile: ~45 min at
  `-j12` on this 16-core box, and **an estimated 3–4× that on a 4 vCPU hosted
  runner**, which is item 3's decision.
* **Image size:** the kernel image is RPMs only (`FROM scratch`), so it adds
  nothing to what a user downloads. `core` installs the same kernel it installed
  before, from a different source — **the fleet's ~5 GB update cost is
  unchanged.**
* **Security updates — the obligation changed shape, and this is the part that
  needs a human decision.** Before: the COPR rebuilt and APEX ran a `force_core`
  to pick it up. Now: **when CachyOS tags a new `cachyos-7.2.*`, somebody must
  bump `KERNEL_TAG` and its sha in `kernel/kernel.pin`.** Nothing happens on its
  own, and an unbumped pin is a kernel that silently stops receiving security
  fixes while every gate stays green — the pin is a promise to keep bumping it.
  Item 7 is the mechanism that makes drift visible instead of remembered.

## Machines

**Nothing was run on katana or the L16.** No image was installed, no deployment
touched, no `apex` command run against the live system. Everything here is a
container build plus `curl`, under `/var/lab-scratch`. The L16's own
`/sys/kernel/btf/vmlinux` was **not** read this round — the substrate was
Fedora's `kernel-debuginfo` RPM from koji, downloaded fresh.
