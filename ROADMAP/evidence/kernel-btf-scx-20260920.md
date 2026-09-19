# kernel-btf-scx — why no sched-ext scheduler can load, and why APEX cannot fix it

2026-09-20, unit `kernel-btf-scx`, item **P1-043**.

The final image qualification found that **no `scx_*` scheduler loads on an
APEX image at all** and recorded the cause as "APEX's kernel BTF was generated
with `pahole < 1.26`", in a kernel "this repo bakes from `kernel/**`".

This unit was sent to confirm that from the artefacts rather than inherit it.
**The defect is real and worse than described; both stated causes are wrong.**

* APEX does not build this kernel. It installs a prebuilt RPM.
* The kernel was built with **pahole 1.30**, and the mechanism that version
  enables demonstrably ran.
* **Fedora's own stock kernel of the same version has the same defect**, so
  "boot Fedora's kernel instead" is not the workaround it looks like.

Nothing was changed on the L16. **katana was read-only**: `rpm -qa`,
`journalctl`, `zcat /proc/config.gz`, `ls`, and a copy of
`/sys/kernel/btf/vmlinux` — plus three throwaway Python files written under
`/tmp` to parse that BTF in place, all three since removed (`/tmp/btf*.py` no
longer matches). No package was installed, no unit touched, no image built.
`systemctl --failed` empty, `sched_ext/state` `disabled`, `greetd` active
afterwards.

---

## 1. The defect, measured

### 1.1 What the loader says

`sudo journalctl -u scx_loader.service -b -o cat`, katana, boot of
2026-09-19T22:55, kernel `7.2.6-cachyos1.fc43.x86_64`:

```text
[INFO]: starting scx_lavd command
…
libbpf: extern (func ksym) 'scx_bpf_create_dsq': func_proto [1864]
        incompatible with vmlinux [60823]
libbpf: failed to load BPF skeleton 'bpf_bpf': -EINVAL
Error: the running kernel's BTF has malformed scx kfunc prototype(s):
  scx_bpf_cidperf_cap, scx_bpf_cidperf_cur, scx_bpf_cidperf_set,
  scx_bpf_cpu_curr, scx_bpf_cpuperf_set, scx_bpf_create_dsq,
  scx_bpf_destroy_dsq, scx_bpf_dispatch_cancel, scx_bpf_dsq_insert___v2,
  scx_bpf_dsq_nr_queued, scx_bpf_dsq_peek, scx_bpf_dsq_reenq,
  scx_bpf_exit_bstr, scx_bpf_get_idle_cpumask, scx_bpf_kick_cpu,
  scx_bpf_locked_rq, scx_bpf_pick_any_cpu, scx_bpf_pick_any_cpu_node,
  scx_bpf_pick_idle_cpu, scx_bpf_pick_idle_cpu_node, scx_bpf_task_cgroup,
  scx_bpf_test_and_clear_cpu_idle.
These kfuncs are KF_IMPLICIT_ARGS but their public BTF prototype
still carries the implicit 'struct bpf_prog_aux *' argument, which
makes BPF programs fail to load with 'func_proto incompatible with
vmlinux'. This happens when the kernel was built with pahole < 1.26.
Fix: boot a kernel whose BTF was generated with pahole >= 1.26.
Affected distros include Ubuntu 24.04 LTS. See kernel commit
9edd04c4189e ("docs: Raise minimum pahole version to 1.26 for
KF_IMPLICIT_ARGS kfuncs").
[ERROR]: Failed to start scheduler (attempt 1/5)
```

Twenty-two names. Five attempts, then it gives up. **That last paragraph is
`scx_utils`' own hypothesis, printed unconditionally on this failure, and §2
shows it does not hold for this kernel.** The 22 names are a measurement and
they are correct.

### 1.2 What the kernel's BTF actually contains

`/sys/kernel/btf/vmlinux` on katana, 6 599 456 bytes, parsed directly:

```text
FUNC [130574] scx_bpf_create_dsq -> proto [60823] (3 params)
    s32 scx_bpf_create_dsq(u64 dsq_id, s32 node, const struct bpf_prog_aux * aux)
FUNC [130612] scx_bpf_kick_cpu   -> proto [60845] (3 params)
    void scx_bpf_kick_cpu(s32 cpu, u64 flags, const struct bpf_prog_aux * aux)
FUNC [130579] scx_bpf_dsq_insert -> proto [65795] (4 params)
    void scx_bpf_dsq_insert(struct task_struct * p, u64 dsq_id, u64 slice, u64 enq_flags)
```

**Type `60823` is the id `libbpf` named**, which is how the reader was
validated before anything was concluded from it. The BPF program declares the
two-argument form; the kernel publishes three; the loader refuses.
`scx_bpf_dsq_insert`, which is *not* on the failing list, has no such argument
— so the failure is per-kfunc, not per-kernel-feature.

### 1.3 The mechanism ran — for most kfuncs

Kernel commit `9edd04c4189e` states the pipeline, and it is worth quoting
because it is the thing that turns out to be only partly broken:

> `scripts/Makefile.btf` passes `--btf_features=decl_tag_kfuncs` to pahole only
> when pahole >= 1.26. Without that flag, pahole emits no DECL_TAG BTF entries
> for `__bpf_kfunc`-annotated functions. As a result,
> `resolve_btfids/main.c::collect_kfuncs()` finds no `bpf_kfunc` DECL_TAGs,
> short-circuits, and `btf2btf()` never creates the `_impl` variants or strips
> the implicit 'aux' argument from the visible proto.

Counted in katana's own BTF (149 301 types):

| reading | katana |
|---|---|
| `BTF_KIND_DECL_TAG` records | 284 |
| …valued `bpf_kfunc` | **282** |
| …valued `bpf_fastcall` | 2 |
| `FUNC`s ending `_impl` | **44** |
| `scx_bpf_*` kfuncs carrying a `bpf_kfunc` tag | 49 |

So `decl_tag_kfuncs` **did** run, `collect_kfuncs()` **did** find tags, and
`btf2btf()` **did** create `_impl` variants and strip the argument — 282 times.
What is true is narrower and stranger: **the 22 failing kfuncs carry no
`bpf_kfunc` DECL_TAG at all**, while neighbours in the same source file do.
`scx_bpf_cpuperf_cap` and `scx_bpf_cpuperf_cur` are tagged; `scx_bpf_cpuperf_set`
is not. `scx_bpf_dsq_insert` is tagged; `scx_bpf_dsq_insert___v2` is not.

A full sweep of the family, rather than only the 22 names the loader printed:
**22 of 68 `scx_bpf_*` kfuncs are affected, and they are exactly the 22.**

> `_impl` twins are excluded from that population, and that exclusion was
> learned the hard way — see §5.2. `scx_bpf_dsq_insert_impl` is *supposed* to
> carry the argument; it is the pre-strip original, and no BPF program
> references it.

`nr_rejected` stays `0` and `sched_ext/state` never reads `enabling`: the
kernel never sees an attach to reject, because the program will not load.
**`SCX_SETTLE` is not implicated and needs no change.**

---

## 2. Where the kernel comes from, and what built it

### 2.1 APEX does not build a kernel

`Containerfile.core`, stage 1 (lines 216–224):

```dockerfile
dnf5 -y copr enable bieszczaders/kernel-cachyos; \
dnf5 -y copr enable bieszczaders/kernel-cachyos-addons; \
dnf5 -y remove … kernel kernel-core kernel-modules kernel-modules-core; \
dnf5 -y install … kernel-cachyos kernel-cachyos-core kernel-cachyos-modules \
    kernel-cachyos-devel-matched …
```

katana, `rpm -qa | grep ^kernel`:

```text
kernel-cachyos-7.2.6-cachyos1.fc43.x86_64
kernel-cachyos-core-7.2.6-cachyos1.fc43.x86_64
kernel-cachyos-devel-7.2.6-cachyos1.fc43.x86_64
kernel-cachyos-devel-matched-7.2.6-cachyos1.fc43.x86_64
kernel-cachyos-modules-7.2.6-cachyos1.fc43.x86_64
kernel-headers-7.2.4-100.fc43.x86_64      ← Fedora's, headers only
```

`rpm -q kernel` → *not installed*, because Fedora's was **removed and
replaced**, not because the kernel is unpackaged. `kernel/**` in this
repository is **six files** — `Containerfile.kernel-spike`, a `.gitkeep`, and
four `spike-b/` files from M0. It builds nothing that ships. The final-image
evidence's "this repo bakes from `kernel/**` … where a follow-up item has to
land" is wrong and has been corrected in place.

### 2.2 The build that produced it

COPR `bieszczaders/kernel-cachyos`, chroot `fedora-43-x86_64`, build
**10997378** (2026-09-18). `results.json` names
`kernel-cachyos 7.2.6 cachyos1.fc43` — katana's exact NVR. It is also the
**newest** `kernel-cachyos` build in that chroot: there is nothing later to
pick up.

From that build's `builder-live.log.gz` (the buildroot install, lines 1004 and
1040):

```text
dwarves       x86_64  1.30-2.fc43   fedora    394.1 KiB
libdwarves1   x86_64  1.30-2.fc43   fedora    626.3 KiB
```

**pahole 1.30. Not < 1.26.** The running kernel says the same about itself —
`zcat /proc/config.gz`:

```text
CONFIG_CC_VERSION_TEXT="gcc (GCC) 15.3.1 20260722 (Red Hat 15.3.1-1)"
CONFIG_CC_IS_GCC=y
CONFIG_CLANG_VERSION=0
CONFIG_PAHOLE_VERSION=130
CONFIG_SCHED_CLASS_EXT=y
CONFIG_DEBUG_INFO_BTF=y
CONFIG_DEBUG_INFO_BTF_MODULES=y
CONFIG_PAHOLE_HAS_LANG_EXCLUDE=y
```

`scripts/Makefile.btf` adds `decl_tag_kfuncs` at `>= 126` and `attributes` at
`>= 130`, so both flags were passed. The spec sets no `PAHOLE_FLAGS` and its
`%build` is a plain `%make_build … all`; `%define _build_lto 0` leaves
`_lto_args` undefined, so **GCC, not clang**, despite `llvm`/`clang`/`lld`
being in the buildroot.

### 2.3 What the build log shows changing, and what it is not

The COPR build diffs CachyOS's upstream `config` against what `oldconfig`
produces in the Fedora buildroot. Three lines from that diff matter:

```diff
-CONFIG_CC_VERSION_TEXT="gcc (GCC) 16.1.1 20260625"
+CONFIG_CC_VERSION_TEXT="gcc (GCC) 15.3.1 20260722 (Red Hat 15.3.1-1)"
-CONFIG_PAHOLE_VERSION=131
+CONFIG_PAHOLE_VERSION=130
-CONFIG_PAHOLE_HAS_BTF_TAG=y
```

It is tempting to read `PAHOLE_HAS_BTF_TAG` going off as the cause. **It is
not**, and saying so without checking would be this program's usual mistake:

* In stable `v7.2.6`, `lib/Kconfig.debug:411` gates it on `CC_IS_CLANG`, so a
  GCC build never gets it regardless of pahole version.
* It only controls `BTF_TYPE_TAG` (`include/linux/compiler_types.h:37`) — the
  `__user`/`__rcu`/`__percpu` annotations. `__bpf_kfunc` is
  `__used __retain __noclone noinline` (`include/linux/btf.h:89`) and carries
  no `btf_decl_tag`; the kfunc tags come from pahole reading the ELF, not from
  the compiler.

That leaves **pahole 1.30 vs 1.31** and **GCC 15 vs GCC 16** as the two
differences that could explain it, and **they cannot be separated without
building a kernel**. They are left as upstream's bisect rather than guessed at
here.

> **Caveat, stated rather than buried.** `lib/Kconfig.debug` and
> `scripts/Makefile.btf` above were read from stable `v7.2.6`. CachyOS builds
> from its own tree (`Source0: CachyOS/linux @ cachyos-7.2.6-1`) carrying
> sched_ext patches beyond stable — `scx_bpf_cid*` and `scx_bpf_cidperf*` are
> not upstream kfuncs. Those two files are the right shape of the pipeline;
> they are not certainly the exact ones that built this kernel.

---

## 3. The control: Fedora's own kernel has it too

This is the reading that changes the recommendation, so it was taken rather
than reasoned about.

`kernel-core-7.2.6-100.fc43.x86_64` from Fedora koji — **the same upstream
version, the same buildroot toolchain** (`CONFIG_CC_VERSION_TEXT="gcc (GCC)
15.3.1 20260722"`, `CONFIG_PAHOLE_VERSION=130`), no CachyOS patches. `vmlinuz`
unpacked from the rpm, the zstd payload located and decompressed to the
`vmlinux` ELF, `.BTF` dumped with `objcopy`, and the same reader run over it.

| | katana (`kernel-cachyos`) | Fedora stock (`kernel-core`) |
|---|---|---|
| `scx_bpf_*` kfuncs examined | 68 | 68 |
| **carrying the implicit `aux` argument** | **22** | **18** |
| `bpf_kfunc` DECL_TAGs in BTF | 282 | 288 |
| `_impl` FUNCs | 44 | 48 |
| `scx_bpf_create_dsq` | 3 params — broken | 2 params — clean |
| `scx_bpf_dsq_nr_queued` | broken | **broken** |
| `scx_bpf_get_idle_cpumask` | broken | **broken** |

Different eighteen-vs-twenty-two, overlapping in ten. Ten of the 22 kfuncs
`scx_lavd` needs are broken on Fedora's kernel too — `scx_bpf_dsq_nr_queued`,
`scx_bpf_get_idle_cpumask`, `scx_bpf_destroy_dsq`, `scx_bpf_dispatch_cancel`,
`scx_bpf_test_and_clear_cpu_idle`, `scx_bpf_pick_any_cpu`,
`scx_bpf_pick_any_cpu_node`, `scx_bpf_cidperf_cur`, `scx_bpf_cidperf_set`,
`scx_bpf_dsq_insert___v2` — so **`scx_lavd` would fail to load on Fedora 43's
stock kernel as well.**

Two conclusions follow:

1. **This is not CachyOS packaging.** It is the Fedora 43 toolchain
   (`dwarves-1.30`, GCC 15.3.1) against a 7.2 kernel, and which kfuncs fall
   through varies with the tree and config — which is why the two sets differ.
2. **Switching APEX to Fedora's stock kernel would not fix Gaming Mode's
   scheduler tier**, and would cost BORE, 1000 Hz and the rest of the reason
   that kernel was chosen. It is not the cheap escape it appears to be.

---

## 4. Is it fixable here? No — and each alternative is named

**Not in `apex-os`.** APEX consumes a prebuilt kernel RPM; there is no
toolchain in this repository to bump, no BTF generation step to change, and
`kernel/**` is a spike.

Options, with what each actually costs:

| option | verdict |
|---|---|
| **Wait for a fixed COPR kernel** | The realistic path. Build 10997378 is the newest; nothing newer exists. **But "unpinned so it updates itself" is not true in practice** — the kernel is only reinstalled during a **`core`** rebuild, which triggers on `Containerfile.core` or `kernel/**` changing, `force_core`, or a new `fedora-bootc` digest (`docs/update-cost.md`). A fixed kernel needs a deliberate `force_core`. |
| **Regenerate the BTF inside the image** | **Not viable, and nobody should try.** No `vmlinux` is shipped — `/usr/lib/modules/<kver>/` has `vmlinuz` only — and `CONFIG_MODULE_ALLOW_BTF_MISMATCH is not set`, so a rewritten vmlinux BTF would be refused by every module's BTF, including the 14 MOK-signed out-of-tree modules this image builds. |
| **Switch to Fedora's stock kernel** | Measured in §3: does not fix it. Also loses the kernel's whole reason for being. |
| **Pin an older `kernel-cachyos`** | No evidence any older build was ever clean. Three shipped images never loaded a scheduler, across at least two kernel versions. Pinning backwards on a hope costs a rebuild and gives up security updates. |
| **Build the kernel in APEX CI with a newer pahole** | Technically the only thing that would certainly work. It changes what every machine boots, adds a kernel build to the `core` tier (already ~45 min and ~5 GB of fleet pull), and takes on kernel maintenance permanently. **This is Andre's decision, not an agent's at 5 am.** It was not started. |
| **Report it upstream** | Cheap and real. §1–§3 *are* the report: the 22 names, the tag/`_impl` counts, the buildroot versions, the config diff, and the Fedora control. |

### 4.1 The candidate upstream fix, and it is unverified

Fedora has **`dwarves-1.32-1.fc43`** in **updates-testing**; stable is
`dwarves-1.30`. 1.32's changelog folds 1.31 and names, among ~170 entries:

```text
- Add elf_strptr NULL checks and fix kfunc bounds
- Prefer strong function definitions for BTF generation
- Ensure the first same-name function has a non-zero address
- Only skip optimized parms when ABI changed
- Factor out BPF kfunc emission
```

Those are the right shape for "some kfuncs got no DECL_TAG". **Nothing here
verifies that they fix these 22** — doing so needs a kernel built against 1.32,
which is the thing this unit declined to do. It is recorded as the candidate,
not as the answer.

**So the sequence out is:** `dwarves-1.32` reaches Fedora 43 stable → the COPR
rebuilds `kernel-cachyos` against it → APEX runs a `core` rebuild → the
scheduler tier starts working. Every step is somebody else's except the last.

---

## 5. What was built instead: the user-visible claim now matches

### 5.1 The probe

`apexd/apexd-core/src/kernelbtf.rs` — a bounded BTF reader with no dependency,
rooted at `sys_root` exactly as `read_scx_state` is, so every answer is
reachable from a temp directory. It reads `<sys_root>/kernel/btf/vmlinux`,
finds every `scx_bpf_*` `FUNC`, and asks whether its **last** parameter
resolves through pointer and modifiers to `struct bpf_prog_aux`.

Five answers, none folded together: `Usable`, `ImplicitArgs { affected,
examined }`, `NoSchedExtKfuncs`, `Absent`, `Unreadable`. Only the first two
kinds of "no" block loading; **a probe that could not read the BTF blames
nothing.**

`apex game status` gains a fourth sched-ext key, **`scx_btf`** (`ok` /
`implicit-args` / `no-sched-ext` / `absent` / `unreadable` / `not probed`), and
`scx_detail` gains a clause naming a kfunc and the proportion — but only when
the probe says loading is blocked **and** the kernel did not end up with a
scheduler. A `loaded` session is not argued with; a kernel with nothing wrong
earns no sentence. The key is reported **while game mode is off**, so a user
can learn that no session can carry a scheduler without starting one.

What katana's status would now say, from the real BTF through the shipped
reader:

```text
scx_state  : not loaded
scx_btf    : implicit-args
scx_detail : asked for scx_lavd; scxctl refused: … — kernel BTF: 22 of 68
             sched-ext kfuncs still carry the verifier's implicit
             'struct bpf_prog_aux *' argument (e.g. scx_bpf_cidperf_cap), so
             libbpf rejects every scx scheduler with 'func_proto incompatible
             with vmlinux' — NO sched-ext scheduler can load on this kernel,
             and no APEX setting changes that
```

That is the difference between a bug report about APEX and a kernel to wait
for.

### 5.2 The reader was validated against real kernels, and that caught a defect

Fixtures alone would not have caught this. Run over katana's actual 6.6 MB
`vmlinux` BTF, the first version reported **47** affected kfuncs where `libbpf`
names 22. The extra 25 were `_impl` twins — `scx_bpf_dsq_insert_impl` and
friends — which are *supposed* to carry the argument: they are the pre-strip
originals `resolve_btfids` renames, and no BPF program references one.

With `_impl` skipped the reader names **exactly the 22 `libbpf` named**, and 18
of 68 on Fedora's kernel. The skip is now a named constant with a comment
saying how it was found, and two tests pin it in both directions.

### 5.3 Gates

```text
cargo test --locked --workspace --no-fail-fast   3476 passed, 0 failed  (was 3450)
cargo clippy --locked --workspace --all-targets -- -D warnings   clean
test-apex-gaming            131 passed, 0 failed
test-apex-modes              67 passed, 0 failed
test-apex-gaming-session     46 passed, 0 failed, 0 skipped
check-doc-verbs              0 stale, 0 undocumented and undeclared
check-suites-run-in-ci       0 unrun and undeclared
check-shellcheck-coverage    0 newly failing
check-containerfile-assertions  195 checked, 0 failed, 0 inert
check-no-conflict-markers    PASS
```

26 new assertions. **Twelve mutations, each naming the row it turned red, and
two of them escaped the first pass** — recorded rather than tidied away,
because both were the same class this program keeps finding:

| mutation | caught by |
|---|---|
| MB1 count `_impl` twins | `the_impl_twin_is_the_mechanism_working_not_failing` (+1) |
| MB2 check the first parameter, not the last | six rows |
| MB3 accept any struct pointer | `another_struct_pointer_is_not_the_implicit_argument` |
| MB4 fold a failed `stat` into `Absent` | **ESCAPED** → `a_stat_that_fails_for_any_other_reason_is_not_absence` |
| MB5 skip an unknown BTF kind instead of failing | `an_unknown_record_kind_stops_the_parse` |
| MB6 make `Unreadable` block loading | `a_missing_file_is_absent_and_a_directory_is_not` |
| MB7 drop the `scx_bpf_` prefix filter | two rows |
| MB8 drop the zero-parameter guard | **ESCAPED** → `a_kfunc_with_no_parameters_is_read_rather_than_panicked_on` |
| MB9 append the clause on a good BTF | **ESCAPED** → strengthened to `!contains("kernel BTF")` |
| MB10 drop the `loaded` guard | `a_loaded_scheduler_is_not_argued_with` |
| MB11 drop the clause from the idle surface | `the_idle_surface_answers_before_anybody_starts_a_session` |
| MB12 never fill `scx.btf` | four rows |

* **MB4** is permission-denied-is-not-absence, inside the module written to
  stop reporting one thing as another. The existing test put a *directory*
  where the file goes — but `metadata()` on a directory **succeeds**, so it
  never reached the arm being mutated; the failure came later, from the
  `read`. A regular file where `kernel/btf` should be a directory makes the
  `stat` itself fail with `ENOTDIR`, needs no privilege, and closes it.
* **MB8** would have **panicked** `apex game status` on any kernel with a
  zero-parameter sched-ext kfunc — `scx_bpf_locked_rq()` on a *correctly built*
  kernel is exactly that. Nothing in the suite had a zero-parameter prototype.
* **MB9** is a gate that inspects nothing: the negative rows asserted only that
  the alarming words were absent, and the `Usable` sentence contains none of
  them, so appending the clause unconditionally passed everything. They now
  assert the clause is **absent**, not merely harmless.

All sources restored with plain `cp` and verified with `cmp` after every
mutation.

### 5.4 Documents corrected

* `docs/gaming-and-sessions.md` — **new §5d** (the defect, the probe, the five
  `scx_btf` readings) and **§6.8 rewritten**. §6.8's Rows A and C expected
  `scx_state : loaded`, which **cannot happen on any APEX image built to
  date**; whoever ran them next would have read a permanent kernel defect as a
  regression in the round-before's fix. A **Row 0** now decides in one command
  whether A and C are runnable, and a **Row A-alt** proves what *is* provable
  on an affected kernel. §5c cross-references §5d.
* `docs/apexd-dbus.md` — `scx_btf` added to the `Status` table and documented.
* `ROADMAP/evidence/final-image-20260920.md` §3 — corrected in place and marked
  as a correction, for both wrong claims. The rest of that section stands; its
  readings were right.

---

## 6. What this does NOT say

* **Nothing here was run on hardware beyond reads.** The probe's output above
  is the shipped reader over katana's real BTF blob, run on the build machine.
  `apex game status` itself has not printed `scx_btf` on any machine, because
  that needs an image build and one was not justified for a status key.
* **The 22 vs 18 difference between the two kernels is not explained.** Both
  toolchains are identical; the trees and configs are not. Why pahole tags one
  kfunc and not its neighbour is a `dwarves` question and was deliberately not
  chased past the point where it changed the recommendation.
* **`dwarves-1.32` is a candidate, not a verified fix.** Confirming it needs a
  kernel built against it.
* **Row C's other half is still unreached** — "stopped it rather than restoring
  it" needs a scheduler that was running first, which no kernel here can give.
* **`root/ops` remains unread on any machine**, and `scx_ops_matches()`'
  expectation that `scx_lavd` attaches as `lavd` is still unverified on
  hardware. It cannot be verified until a scheduler loads.
* katana is unchanged: booted `apex-661a9d80…` (`sha256:61f7935c…`), three
  deployments with the September 18 one pinned, greetd untouched, no
  `efibootmgr` write, nothing written to any disk but three files under `/tmp`
  which were removed.
