# kernel-build — dwarves 1.32 fixes the kfunc defect, measured

2026-09-20, unit `kernel-build`. Follows `ROADMAP/evidence/kernel-btf-scx-20260920.md`,
which established that **no sched-ext scheduler can load on any APEX image** because
some `scx_bpf_*` kfuncs carry no `bpf_kfunc` DECL_TAG in the kernel's BTF, and left
`dwarves-1.32` recorded as **"the candidate, not the answer"**.

**It is now the answer. Measured, not read off a changelog.**

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
