# kernel-build — dwarves 1.32 fixes the kfunc defect, measured

2026-09-20, unit `kernel-build`. Follows `ROADMAP/evidence/kernel-btf-scx-20260920.md`,
which established that **no sched-ext scheduler can load on any APEX image** because
some `scx_bpf_*` kfuncs carry no `bpf_kfunc` DECL_TAG in the kernel's BTF, and left
`dwarves-1.32` recorded as **"the candidate, not the answer"**.

**It is now the answer. Measured, not read off a changelog.**

> **And as of 2026-09-20 it is no longer only an A/B over someone else's
> kernel.** APEX built its own with the pinned pahole 1.32, and both readers
> call the result clean — **0 of 68 `scx_bpf_*` kfuncs untagged**, the shipped
> reader's first `verdict: ok` on any kernel in existence. §4.5 has the run.

---

## 1. The headline

| reading | `scx_bpf_*` examined | **untagged** | `bpf_kfunc` DECL_TAGs |
|---|---|---|---|
| the shipped kernel's own `.BTF` | 68 | **18** | 288 |
| pahole **1.30** re-run on that kernel's DWARF | 68 | **18** | 288 |
| pahole **1.32** re-run on the same DWARF | 68 | **0** | **308** |

**All 18 fixed. None newly broken. `still broken: []`, `NEW in 1.32: []`.**

The 18 names 1.32 fixes:

```text
scx_bpf_cid_curr            scx_bpf_cid_override        scx_bpf_cidperf_cur
scx_bpf_cidperf_set         scx_bpf_cpu_to_cid          scx_bpf_cpuperf_cap
scx_bpf_destroy_dsq         scx_bpf_dispatch_cancel     scx_bpf_dsq_insert___v2
scx_bpf_dsq_nr_queued       scx_bpf_error_bstr          scx_bpf_get_idle_cpumask
scx_bpf_get_idle_smtmask_node   scx_bpf_pick_any_cpu    scx_bpf_pick_any_cpu_node
scx_bpf_reenqueue_local     scx_bpf_task_set_slice      scx_bpf_test_and_clear_cpu_idle
```

`scx_bpf_get_idle_cpumask`, `scx_bpf_destroy_dsq` and `scx_bpf_dispatch_cancel` are
the three the prior evidence found broken on **all three** kernels it read. They are
in that list.

---

## 2. What was measured, and why this substrate

### 2.1 The substrate

`kernel-debuginfo-7.2.6-100.fc43.x86_64.rpm`, from koji:

```text
https://kojipkgs.fedoraproject.org/packages/kernel/7.2.6/100.fc43/x86_64/
    kernel-debuginfo-7.2.6-100.fc43.x86_64.rpm
sha256  4d70775bee15950e41e2dcc193d2d77f386470cc2aa4454708aed402ef5f009f
→ /usr/lib/debug/lib/modules/7.2.6-100.fc43.x86_64/vmlinux   (537 MB, DWARF intact)
```

**Why Fedora's kernel and not CachyOS's**: the CachyOS COPR spec sets
`%define debug_package %{nil}` (line 6 of `kernel-cachyos.spec`, build 10997378), so
**the COPR publishes no debuginfo at all** — there is no CachyOS vmlinux carrying
DWARF to re-run pahole against. Fedora's 7.2.6 kernel is the same upstream version,
the same buildroot toolchain (GCC 15.3.1, `CONFIG_PAHOLE_VERSION=130`), and the prior
evidence already measured it at **18 of 68** — which makes it a substrate with a
*known expected control value* rather than an unknown.

### 2.2 The reading is deliberately NOT the shipped reader's reading

`apexd/apexd-core/src/kernelbtf.rs` asks the **post-`resolve_btfids`** question: does
the published FUNC prototype still carry the implicit `struct bpf_prog_aux *`? That is
the user-visible symptom, and it is the right question for the shipped probe and for
the build gate (§4).

This measurement asks the **pre-`resolve_btfids`** question: did pahole emit a
`bpf_kfunc` DECL_TAG for the function at all? That is the root cause — with no tag,
`collect_kfuncs()` never sees the function and `btf2btf()` never strips the argument.

A freshly-`pahole`'d blob has not been through `resolve_btfids`, so there is nothing
stripped yet and the shipped reader **cannot** be used on it. These are two stages of
one pipeline, not two implementations of one check. The script is
`kernel/research/kfunc-tags.py` and says so in its docstring.

### 2.3 The control that makes R2 mean something

The final `vmlinux` in a debuginfo package has already been through `resolve_btfids`,
which patches and sorts `.BTF_ids` — and `readelf -S` confirms this one carries
`.BTF`, `.BTF_ids` and `.debug_info` but **no `.rela.BTF_ids`**. pahole's kfunc
collection reads that section, so it was entirely possible that re-running pahole on a
*linked* vmlinux would find no kfuncs at all and produce a meaningless zero.

So R1 was run before believing anything about R2, and it is the load-bearing row:

```text
1.30 vs shipped: same=18  only-in-1.30=[]  only-in-shipped=[]
```

pahole 1.30 on this DWARF reproduces the shipped kernel's **exact 18 names**, and its
total `bpf_kfunc` DECL_TAG count (288) to the digit. The substrate is not confounded,
and **the same 18 that the argument-presence reading found are the 18 the
tag-presence reading finds** — two independent routes to the same set, which is also
the first direct confirmation of the causal chain the prior evidence inferred.

Only then does R2 mean what it appears to mean.

### 2.4 The flags are each version's own flags

`scripts/Makefile.btf` keys pahole's flags on `CONFIG_PAHOLE_VERSION`, so 1.30 and
1.32 are **not** invoked identically by a real kernel build — 1.32 additionally gets
`--btf_features=layout`. Transcribed from that file at `CachyOS/linux @
cachyos-7.2.6-1` (saved as `kernel/research/Makefile.btf.cachyos`):

```text
1.30:  -J -j12 --btf_features=encode_force,var,float,enum64,decl_tag,type_tag,\
              optimized_func,consistent_func,decl_tag_kfuncs \
              --lang_exclude=rust --btf_features=attributes
1.32:  … the same, plus --btf_features=layout
```

Using one flag set for both would have been neither build. `--lang_exclude=rust` is
included because the shipped kernel reports `CONFIG_PAHOLE_HAS_LANG_EXCLUDE=y`.

### 2.5 Reproducing it

```sh
kernel/research/run-ab.sh      # runs inside registry.fedoraproject.org/fedora:43
```

**36 seconds** of container time (08:14:11 → 08:14:47, from the unit's own
journal), plus the one-off 1.3 GB debuginfo download. pahole itself is
**7.1 s (1.30)** and **5.3 s (1.32)** over the 537 MB vmlinux, peak RSS 1.05 GB
and 0.99 GB — so the BTF step is *not* a meaningful part of a kernel build's
cost, and the whole experiment is cheap enough that there was never a good
reason to leave 1.32 unverified.

Both dwarves builds are pinned to permanent koji NVR URLs, not to "whatever
updates-testing has today":

```text
dwarves-1.30-2.fc43  libdwarves1-1.30-2.fc43   ← what COPR build 10997378 used
dwarves-1.32-1.fc43  libdwarves1-1.32-1.fc43   ← the fix
https://kojipkgs.fedoraproject.org/packages/dwarves/<ver>/<rel>/x86_64/
```

---

## 3. What this does NOT say

* **It does not prove a full APEX kernel built with 1.32 loads a scheduler.** It
  proves the root-cause step — tag emission — is fixed, on a real 7.2.6 kernel's real
  DWARF, for every one of the 68 kfuncs. The end-to-end proof is a built kernel whose
  post-`resolve_btfids` BTF the shipped reader calls `Usable`, which is what the build
  gate in §4 exists to assert on every build.
* **It was measured on Fedora's tree, not CachyOS's.** CachyOS's tree carries extra
  sched_ext patches. The prior evidence expected `scx_bpf_cid*`/`scx_bpf_cidperf*` to
  be CachyOS-only; **they are present in Fedora's stock 7.2.6 too** (they are in the
  18 above), so that caveat was wider than it needed to be. Noted as a correction.
  The defect is a pahole defect and the fix is in pahole, so a different tree changes
  *which* kfuncs fall through, not whether 1.32 emits tags for them — but the tree is
  a variable this reading did not hold constant, and the gate is what closes it.
* **`dwarves-1.32-1.fc43` is in Fedora 43 updates-testing, not stable.** Pinning it by
  koji NVR URL is what makes it usable now; that pin is a build input, not a runtime
  dependency, and nothing ships dwarves to a user.

---

## 4. The build tier, and what the first end-to-end run found

§1–§3 were measured by re-running pahole over an existing kernel's DWARF. That
answers the root-cause question and nothing else. The tier in
`Containerfile.kernel` is what turns it into a kernel APEX ships, and its first
two runs each found a defect that no amount of re-reading the A/B would have.

### 4.1 `rpmbuild` deletes the tree the gate has to read

The 2026-09-20 08:21 AWST run **compiled the kernel successfully** and then died
one second later:

```text
Wrote: /build/rpm/RPMS/x86_64/kernel-cachyos-modules-7.2.6-cachyos1.apex1.fc43.x86_64.rpm
Executing(rmbuild): /bin/sh -e /var/tmp/rpm-tmp.PKBW0H
+ rm -rf /build/rpm/BUILD/kernel-cachyos-7.2.6-build
…
+ VMLINUX=
FATAL: no vmlinux under /build/rpm/BUILD — the build tree is not where this expects
```

rpm ≥ 4.20 runs an `Executing(rmbuild)` phase after a **successful** build that
deletes `%{buildsubdir}`. The whole source tree goes, `vmlinux` with it, so the
gate had nothing to read. 49m14s of wall clock (08:21:51 → 09:11:05), and the
podman layer is discarded on a failed `RUN`, so the artefacts went with it.

Measured rather than assumed, on `fedora:43` / rpm 6.0.2 with a throwaway spec
that plants a file called `vmlinux` in its build dir:

| rpmbuild invocation | `find BUILD -name vmlinux` |
|---|---|
| default | **0 found** |
| `--noclean` | **1 found** |

The spec defines no `%clean`, so the flag has no other effect. `--noclean` is
now passed and carries a comment saying why, because it reads like tidy-up
someone can safely remove.

**The prior round's prediction was wrong, and that is worth recording.** Its
card said "the most likely break is `%autopatch` applying the BORE patch
against the pinned tag". `%autopatch` was fine; every source verified by
sha256, the BORE patch applied, the kernel linked. The failure was in the
harness, one line after the compile finished.

Two further changes to the same step, both aimed at the same 50-minute feedback
loop: the vmlinux search now asserts **exactly one** match rather than taking
`head -1` (a gate must read *the* artefact the build produced), and its failure
message names `--noclean` as the first thing to check. The `rpmbuild` wall time
is printed, because the rpmbuild log dies with the layer.

### 4.2 The builder base moved *during this unit*

`kernel.pin` content-addresses every input — kernel tree, config, BORE patch,
both dwarves RPMs — except one: `BUILDER_BASE` named `fedora:43`, a floating
tag. It moved the same day:

* build at 08:21 AWST pulled base blob `ce241f1b…`
* build at 20:5x AWST pulled `a71bf9b8…`
* `skopeo inspect docker://registry.fedoraproject.org/fedora:43` →
  `sha256:84325c67…`, **`Created: 2026-09-20T05:47:58Z`**

The immediate cost was the layer cache missing at step 1, re-downloading the
toolchain and the 266 MB kernel tarball. Both `FROM` lines and `BUILDER_BASE`
now name the digest.

**What that does not pin, stated so nobody reads more into it:** the digest
freezes the base *layer*, not the `dnf5 -y install gcc …` transaction on top of
it, which still resolves against Fedora's live repositories. GCC can still move
between two builds of an unchanged pin. That is tolerable for one reason and it
is not "it's probably fine": what the compiler can change is the kernel
**binary**, and what it cannot change is whether the kfunc BTF is correct —
that is pahole's, pinned by NVR **and** sha256 and asserted twice against the
built kernel. `cc=` in `/manifest/kernel-build.txt` records the compiler each
build actually used, so the residual drift is visible after the fact.

### 4.3 The `*_impl` twins are created by `resolve_btfids` — measured, not assumed

`ROADMAP/evidence/kernel-btf-scx-20260920.md` recorded a limitation on its own
reader: *"the `_impl` skip ASSUMES `_impl` means pre-strip twin, which holds
across all three kernels read but would silently skip a public kfunc genuinely
named `scx_bpf_*_impl`."*

That assumption is now a measurement. Counting `scx_bpf_*` FUNCs in the three
blobs the A/B already produced:

| blob | stage | `scx_bpf_*` FUNCs | of which `*_impl` |
|---|---|---|---|
| pahole 1.30 output | **pre**-`resolve_btfids` | 68 | **0** |
| pahole 1.32 output | **pre**-`resolve_btfids` | 68 | **0** |
| the kernel's shipped `.BTF` | **post**-`resolve_btfids` | 97 | **29** |

pahole emits no `*_impl` name at all, under either version. All 29 appear only
after `resolve_btfids` has run, and 97 = the same 68 plus those 29. They are
created by the strip step, one per kfunc whose implicit `struct bpf_prog_aux *`
was actually removed — not a naming convention, a by-product.

Two consequences:

* The exclusion is **correct for the right reason**. A public kfunc genuinely
  named `scx_bpf_something_impl` would appear in pahole's output, where no
  `*_impl` name exists; the named risk is bounded by a reading rather than by
  hope.
* **Any reader run against a vmlinux's `.BTF` section must exclude them**,
  because a vmlinux is always read after `resolve_btfids`. A reader that did not
  would report a perfectly good kernel as having ~29 untagged kfuncs.

### 4.4 Two readers, because this gate had never passed

`apex-kernel-btf-gate` had never once returned 0 on a real kernel. Every BTF
available to test it against carried the defect, so every test of it was a
negative one. If it failed on a kernel we built, "this kernel is genuinely
broken" and "the reader is wrong about this BTF" would look identical, and
telling them apart costs another 45 minutes.

`kernel/btf-xcheck.sh` is a second, independent reading: `sh` + `bpftool`, no
code shared with the Rust reader, asking the other half of the question — the
gate asks whether the published prototype still carries the implicit
`struct bpf_prog_aux *` (the symptom), this asks whether pahole emitted a
`bpf_kfunc` DECL_TAG at all (the root cause). It runs **first** and prints
whatever it finds, before the reader that can abort the build. If the gate
passes and this one does not, the build fails on the disagreement.

Validated against every BTF on hand, and it fails both ways:

| input | examined | twins excluded | untagged | exit |
|---|---|---|---|---|
| the kernel's shipped `.BTF` | 68 | 29 | **18** | 1 |
| pahole 1.30 on that DWARF | 68 | 0 | **18** | 1 |
| pahole 1.32 on that DWARF | 68 | 0 | **0** | **0** |
| the ELF `vmlinux` directly | 68 | 29 | **18** | 1 |
| a valid BTF with no `scx_bpf_*` at all | 0 | 0 | 0 | **1, refused** |

The first three reproduce §1's table to the digit from an independent
implementation. The last row is the one that matters most: zero untagged out of
zero examined is **not** a passing kernel, and a gate that would call it one is
this repository's dominant defect family. It was produced by compiling a
two-line C file with `-g` and running `pahole --btf_encode_detached` over it —
a real BTF blob that simply contains no sched-ext.

### 4.5 The end-to-end run: it passes, and the gate returned 0 for the first time

`podman build -f Containerfile.kernel -t apex-kernel:local .`, 2026-09-20, unit
`kb-build2`, from commit `a5ce7437` (neither `kernel/kernel.pin` nor
`Containerfile.kernel` moved after it). **`EXIT=0`.**

```text
START                 2026-09-20T12:56:12Z
rpmbuild starting at  2026-09-20T12:59:03Z
rpmbuild took         73m42s at -j12
END                   2026-09-20T14:13:07Z          (76m55s total wall)
vmlinux               498,397,712 bytes
CONFIG_PAHOLE_VERSION=132 (as pinned)
CONFIG_SCHED_CLASS_EXT=y · CONFIG_DEBUG_INFO_BTF=y · CONFIG_EFI_STUB=y
CONFIG_MODULE_ALLOW_BTF_MISMATCH is not set (deliberate)
```

**`CONFIG_PAHOLE_VERSION=132` is the line that proves the pin did something.**
CachyOS's config ships `=131`; the symbol has no prompt, so `make olddefconfig`
discards the stored value and recomputes it from the buildroot's pahole. 132 is
the pinned toolchain, asserted rather than hoped for.

#### Both readers, inside the build

```text
--- reading 1 of 2: bpftool, the root cause (did pahole emit the tag?)
btf-xcheck: 68 scx_bpf_* names examined (47 *_impl resolve_btfids twins excluded)
btf-xcheck: 308 bpf_kfunc DECL_TAGs in this BTF
btf-xcheck: 0 scx_bpf_* names carry NO bpf_kfunc DECL_TAG
btf-xcheck: PASS — every scx_bpf_* name carries the tag
--- reading 2 of 2: apexd kernelbtf.rs, the symptom — THIS ONE IS THE GATE
kernel BTF gate: /build/vmlinux.btf (6,562,087 bytes)
  verdict : ok
  reading : kernel BTF: sched-ext kfunc prototypes are the shape BPF schedulers expect
PASS: this kernel can load a sched-ext scheduler.
both readers agree: this kernel's scx_bpf_* kfuncs are intact
```

**This is the first time `apex-kernel-btf-gate` has ever returned 0.** Every BTF
that existed before this kernel carried the defect, so the gate had been
validated only negatively — four inputs, all exit 1. It now has a positive
control.

#### The arithmetic that confirms the mechanism

§4.3 established that `resolve_btfids` creates one `*_impl` twin per kfunc whose
implicit argument it actually strips. If that is right, a kernel in which 18
more kfuncs became eligible should produce exactly 18 more twins:

| kernel | untagged | `*_impl` twins |
|---|---|---|
| Fedora's stock 7.2.6 (pahole 1.30) | 18 | 29 |
| **this build (pahole 1.32)** | **0** | **47** |

**29 + 18 = 47.** Exactly. The 18 kfuncs pahole 1.30 left untagged are the 18
`resolve_btfids` was skipping, and under 1.32 it processes all of them. That is
a prediction of the §4.3 model, not a number it was fitted to.

The DECL_TAG count agrees with the A/B independently: **308** here, **308** for
pahole 1.32 in §1's table, against 288 for both 1.30 and the shipped kernel.

#### Verified again from outside the container

The build's own gate could in principle pass for a reason peculiar to the
builder image. Both readers were re-run against the `vmlinux.btf` extracted from
the finished image (`podman create` + `podman cp` — `FROM scratch` has no
shell):

| reader | where | result |
|---|---|---|
| `apex-kernel-btf-gate`, host-built binary | the host | `verdict: ok`, **exit 0** |
| `btf-xcheck.sh` | a clean `fedora:43` container | 68 examined, 0 untagged, **exit 0** |

#### What it produced

```text
kver    7.2.6-cachyos1.apex1.fc43.x86_64   (matches core's *cachyos* glob)
cc      gcc (GCC) 15.3.1 20260722 (Red Hat 15.3.1-1)
dwarves 1.32-1.fc43 · pahole_version 132 · btf_scx=usable · rpms=5
```

| rpm | bytes |
|---|---|
| `kernel-cachyos-modules` | 151,446,434 |
| `kernel-cachyos-devel` | 19,361,540 |
| `kernel-cachyos-core` | 18,332,257 |
| `kernel-cachyos` | 6,637 |
| `kernel-cachyos-devel-matched` | 6,529 |
| **total** | **189,153,397 (180.4 MiB)** |

The published image is **196,028,338 bytes (186.9 MiB)** — the RPMs, the config,
the BTF blob and the pin, and nothing else, because it is `FROM scratch`. **No
user ever downloads it**; only the `core` build pulls it. `vmlinux` itself
(475 MiB) is deliberately not shipped.

#### The core contract, checked before trusting a 50-minute core build

`RPMS=… ./tests/check-kernel-contract.sh` installs the produced RPMs into a
scratch `fedora-bootc:43` and checks every assumption `Containerfile.core`
makes. **All pass, `fail=0`, exit 0**: the four hard-gated packages; the
`ls -d /usr/lib/modules/*cachyos*` glob matching **exactly one** directory;
`vmlinuz`/`config`/`System.map`/`build` present; `sign-file` executable and
`CONFIG_MODULE_SIG_HASH` readable (module signing dies without either);
`depmod -a`; `vmlinuz` starting with `MZ`, so it is a real PE that `sbsign` can
sign and a UKI can use as a stub; and `SCHED_CLASS_EXT`, `DEBUG_INFO_BTF`,
`DEBUG_INFO_BTF_MODULES` and `SCHED_BORE` all `=y` with
`MODULE_ALLOW_BTF_MISMATCH` unset.

That test had to be repaired before it could say anything. Run for the first
time today — it had never been runnable, because it needs RPMs — it turned out
to execute **zero** checks and exit 0: `podman run` without `-i` gives the
container an empty stdin, so `bash -s` read EOF and the whole heredoc of
contracts was discarded. The verdict above is from the repaired version, which
fails on a negative control.

#### Cost, now measured end to end rather than estimated

* **73m42s of `rpmbuild`, 76m55s wall.** Longer than the first run's 49m14s, and
  the difference is not the kernel: an unrelated CPU-heavy application was
  running on this machine throughout. `-j12` on 16 cores does not get 12 cores
  when something else wants four. The honest figure for a quiet machine is the
  first run's, ~45–50 minutes.
* **Disk:** `/var` went 450 GB free to 398 GB observed mid-build. The ~100 GB
  figure from the first run stays the one to plan against — this run was
  sampled rather than watched, and the tree is deleted inside the same `RUN`, so
  the low point is easy to miss.
* **Fleet cost: unchanged.** `core` installs the same kernel from a different
  source.

#### What is now proven rather than inferred

§3 said the A/B "does not prove a full APEX kernel built with 1.32 loads a
scheduler", and that the end-to-end proof is "a built kernel whose
post-`resolve_btfids` BTF the shipped reader calls `Usable`". **That kernel now
exists and the shipped reader calls it exactly that** — from inside the build,
and again from outside it.

One thing is still inferred, and saying so is the point of this section. The
reader's verdict is that the prototypes are the shape libbpf expects. **No scx
scheduler has been loaded on this kernel**, because that needs it booted on real
hardware, which this unit deliberately did not do. The remaining gap is
`scx_lavd` actually attaching on a machine running an image built from these
RPMs.
