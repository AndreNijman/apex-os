# unit: kernel-btf-scx — P1-043 (sched-ext kernel BTF)

**Status: COMPLETE and PUSHED, not merged.**
Repo `apex-os`, branch `task/kernel-btf-scx`, worktree
`/var/tmp/apex-work/wt-kernel-btf-scx`, from `roadmap/v2.2` (`303221d5`).

Two commits, tip `b5bea21f`:

```
b5bea21f docs(gaming): sched-ext cannot load on any APEX kernel, and section
         6.8 asked for a reading no machine can give
aafc6898 feat(gaming): say WHICH kind of 'not loaded' sched-ext is, by reading
         the kernel's BTF
```

`git merge-tree --write-tree origin/roadmap/v2.2 task/kernel-btf-scx` → exit
0, tree `9ec57a0d`, **0 conflicts**, against `origin/roadmap/v2.2` at
`303221d5` (re-fetched 2026-09-20; the tip had not moved). Run, not assumed.

Evidence: `ROADMAP/evidence/kernel-btf-scx-20260920.md` (tracked, on the
branch). P1-043 recorded `partial` with `set-status.py`: prior evidence read
out with `yaml.safe_load` first and carried forward **whole** — verified
afterwards by `prior in evidence` → True, 21 172 → 29 460 chars, a per-item
diff showing P1-043 as the only row that changed, 128 tasks before and after,
`global_agent_rules` intact, and 0 hyphen-space corruptions in the new text.

---

## The headline

**No sched-ext scheduler can load on an APEX image, and APEX cannot fix it.**
The unit was sent to confirm the cause from the artefacts rather than inherit
it, and **both inherited causes are wrong**:

1. **APEX does not build this kernel.** `Containerfile.core` lines 216–224
   `dnf5 install`s the prebuilt `kernel-cachyos` RPM from COPR
   `bieszczaders/kernel-cachyos`. `kernel/**` is six M0 spike files and builds
   nothing that ships — so a follow-up item cannot land there, which is what
   the final-image evidence said. Corrected in place.
2. **"Built with pahole < 1.26" is `scx_utils`' own guess**, printed
   unconditionally on this failure. The COPR build used `dwarves-1.30-2.fc43`
   and the running kernel says `CONFIG_PAHOLE_VERSION=130`. The mechanism that
   flag enables **ran** — 282 `bpf_kfunc` DECL_TAGs, 44 `_impl` variants.

The measured defect is narrower: **22 of 68 `scx_bpf_*` kfuncs carry no
`bpf_kfunc` DECL_TAG**, so `resolve_btfids`' `btf2btf()` never stripped their
implicit `struct bpf_prog_aux *`, and `libbpf` refuses every scheduler.
`SCX_SETTLE` is not implicated.

**The control that decides the recommendation:** Fedora's own stock
`kernel-core-7.2.6-100.fc43` — same upstream version, same GCC 15.3.1, same
pahole 1.30 — has **18 of 68 affected**, ten of them kfuncs `scx_lavd` needs.
`scx_lavd` would fail there too. This is the Fedora 43 toolchain against a 7.2
tree, not CachyOS packaging, and switching to the stock kernel fixes nothing
while costing BORE and 1000 Hz.

## What was done

* `apexd/apexd-core/src/kernelbtf.rs` — bounded BTF reader, no dependency,
  rooted at `sys_root` like `read_scx_state`. Five answers, none folded;
  `Unreadable` blames nothing.
* `apex game status` gains `scx_btf` (`ok` / `implicit-args` / `no-sched-ext` /
  `absent` / `unreadable` / `not probed`) and `scx_detail` gains a clause — but
  only when loading is blocked **and** no scheduler attached. Reported while
  game mode is off too.
* `docs/gaming-and-sessions.md` §5d new, §6.8 rewritten (its Rows A and C
  expected a reading **no APEX image can give**), §5c cross-referenced.
  `docs/apexd-dbus.md` documents the key. `final-image-20260920.md` §3
  corrected in place and marked.

Gates: `cargo test --locked --workspace` **3476 passed, 0 failed** (was 3450);
clippy `-D warnings` clean; `test-apex-gaming` 131/0, `test-apex-modes` 67/0,
`test-apex-gaming-session` 46/0; `check-doc-verbs` 0 stale;
`check-suites-run-in-ci` 0 unrun; `check-shellcheck-coverage` 0 newly failing;
`check-containerfile-assertions` 195 checked, 0 failed, 0 inert; no conflict
markers.

26 new assertions, **12 mutations, 3 escaped the first pass** and are written
up in the evidence rather than tidied away — MB4 (a failed `stat` folded into
`Absent`: `metadata()` on a *directory* succeeds, so the existing row never
reached the arm), MB8 (dropping the zero-parameter guard would have **panicked
`apex game status`** on a correctly built kernel), MB9 (the negative rows
asserted only that alarming words were absent — a gate that inspects nothing).

**The reader was validated against real kernels, not only fixtures, and that
caught a defect**: over katana's real 6.6 MB `vmlinux` BTF the first version
named 47 affected kfuncs where `libbpf` names 22. The extra 25 were `_impl`
twins, which are *supposed* to carry the argument. With them skipped it names
exactly the 22.

## Machines

**The L16 was not touched.** **katana was read-only** and is unchanged: booted
`apex-661a9d80` (`sha256:61f7935c…`), three deployments with the September 18
one pinned, greetd active and untouched, no `efibootmgr` write,
`systemctl --failed` empty, `sched_ext/state` `disabled`. Three throwaway
`/tmp/btf*.py` parser files were written and **removed** (verified: the glob no
longer matches). No image was built.

---

## NEXT

Nothing is half-done. In priority order for whoever picks this up:

1. **Decide, or get Andre to decide, whether APEX builds its own kernel.**
   This is the only thing that would certainly fix the scheduler tier, and it
   was deliberately not started: it changes what every machine boots, adds a
   kernel build to the `core` tier (already ~45 min and ~5 GB of fleet pull),
   and is permanent maintenance. The evidence §4 lays out every alternative
   with its cost. **Do not engineer this into the tree without him.**

2. **Watch `dwarves-1.32-1.fc43`.** It is in Fedora 43 **updates-testing**
   (stable is 1.30). Its changelog names "fix kfunc bounds", "Prefer strong
   function definitions for BTF generation" and "Factor out BPF kfunc
   emission" — the right shape, and **UNVERIFIED**. The sequence out is:
   1.32 reaches F43 stable → COPR rebuilds `kernel-cachyos` → APEX runs a
   **`force_core`** rebuild. Note the third step: the kernel is installed
   unpinned but only during a `core` rebuild, which an ordinary image build
   does not trigger.

   To check whether a new COPR kernel is fixed **without an image build**:
   download its `kernel-cachyos-core` rpm, `rpm2cpio | cpio -idm`, find the
   zstd payload inside `vmlinuz` and decompress it to the `vmlinux` ELF,
   `objcopy --dump-section .BTF=btf.bin`, then run the shipped reader over it
   (`apexd_core::kernelbtf::scx_btf_from_bytes`). That is exactly how Fedora's
   kernel was checked; it takes about ten minutes.

3. **Report it upstream to the COPR maintainer.** Evidence §1–§3 *is* the
   report: the 22 names, the tag/`_impl` counts, `dwarves-1.30-2.fc43` from
   the buildroot log, the config diff, and the Fedora control. Nobody has sent
   it.

4. **`apex game status` has never printed `scx_btf` on a machine.** It needs
   an image build and one was not justified for a status key. When the next
   image is built for any reason, run `docs/gaming-and-sessions.md` §6.8
   **Row 0** and **Row A-alt** — they are written for an affected kernel and
   take two minutes. Cross-check the kfunc names in `scx_detail` against
   `journalctl -u scx_loader`; they matched exactly here (22 of 68) and a
   disagreement is a defect in `kernelbtf.rs`, not in the kernel.

5. **Split this item.** P1-043 is titled *"GPU parity across NVIDIA/AMD/Intel"*
   and now carries four sched-ext rounds and ~29 kB of evidence. The sched-ext
   work wants its own id. Its GPU half is also still open on its own terms:
   safe GPU controls on AMD/Intel cannot distinguish "left at default" from
   "no knob exists", and Safe Graphics' automatic dGPU branch needs the panel
   genuinely dark.

Still unreachable on any machine here, and unchanged by this round:
`root/ops` has never been read, `scx_ops_matches()`' expectation that
`scx_lavd` attaches as `lavd` is unverified, and Row C's "stopped it rather
than restoring it" needs a scheduler that was already running.
