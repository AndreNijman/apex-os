# sched_ext cannot load on the COPR kernel, and the reason is the one the kernel tier exists for

Measured on katana 2026-09-22, on the image it booted on 2026-09-19.

P1-043 has carried an assertion marked **UNVERIFIED because "no machine here
can load a scheduler to look"**. That is now measured, and it fails for a named
reason rather than an absent one.

## Everything needed is present

| | |
|---|---|
| `CONFIG_SCHED_CLASS_EXT` | `y` |
| `/sys/kernel/sched_ext/state` | exists, reads `disabled` |
| `/sys/kernel/btf/vmlinux` | present, 6,599,456 bytes |
| schedulers installed | 17 of them — `scx_rustland`, `scx_bpfland`, `scx_lavd`, `scx_rusty`, … |
| kernel | `7.2.6-cachyos1.fc43.x86_64` — the **COPR** kernel, not `apex1` |

So this is not a missing feature, a missing package or a missing config.

## What happens when one is actually run

`sudo timeout --signal=INT 12 scx_rustland`, with the state read before, during
and after:

```
state before: disabled  enable_seq=0
state during: disabled  enable_seq=0  switch_all=0
state after : disabled  nr_rejected=0
```

`enable_seq` never moves. The scheduler never attaches. Its own error says why:

```
Error: the running kernel's BTF has malformed scx kfunc prototype(s):
scx_bpf_cidperf_cap, … scx_bpf_dsq_insert___v2, … scx_bpf_test_and_clear_cpu_idle.

These kfuncs are KF_IMPLICIT_ARGS but their public BTF prototype still carries
the implicit 'struct bpf_prog_aux *' argument, which makes BPF programs fail to
load with 'func_proto incompatible with vmlinux'. This happens when the kernel
was built with pahole < 1.26.
```

Twenty-two kfuncs, one cause: **the kernel was built with pahole older than
1.26**, so `resolve_btfids` emitted prototypes that still carry the implicit
argument and every BPF scheduler is rejected by the verifier.

## Why this matters beyond one row

This is precisely the defect **APEX's own kernel tier exists to fix**. That tier
builds with dwarves/pahole 1.32 and its manifest asserts `btf_scx=usable`;
`Containerfile.core` refuses a kernel whose manifest does not say so, and the
comment there says a core that fell back to the COPR kernel "would ship the BTF
defect the kernel tier exists to fix, and nobody would find out until a
sched-ext scheduler failed to load on a user's machine."

That sentence has now been measured. It is not a hypothetical: a scheduler does
fail to load, on this machine, today, on the COPR kernel.

## The prediction this makes, which is the point

The image now building carries `7.2.6-cachyos1.apex1.fc43` from APEX's own
kernel tier. **If the tier works, `scx_rustland` attaches on the new image and
`enable_seq` moves off 0.** If it does not, the tier's entire justification is
in question and `btf_scx=usable` is asserting something untrue.

Either answer is worth having, and it is one command after the install. Run the
same three reads — before, during, after — and compare against this file.
