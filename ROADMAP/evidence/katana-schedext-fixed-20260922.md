# The kernel tier works: sched_ext loads on `apex1` and does not on the COPR kernel

Measured on katana 2026-09-22, across a single reboot. Same machine, same
scheduler binary, same command, same 17 `scx_*` packages. **Only the kernel
changed.**

## The controlled comparison

|  | COPR `7.2.6-cachyos1.fc43` | APEX `7.2.6-cachyos1.apex1.fc43` |
|---|---|---|
| `state` before | `disabled` | `disabled` |
| `state` **during** | **`disabled`** | **`enabled`** |
| `enable_seq` during | **`0`** | **`1`** |
| `switch_all` during | `0` | **`1`** |
| verdict | scheduler never attached | **attached, and took every CPU** |

`sudo timeout --signal=INT 12 /usr/bin/scx_rustland`, both times.

On the COPR kernel it refused with its own diagnosis — 22 scx kfuncs whose
public BTF prototype still carries the implicit `struct bpf_prog_aux *`,
"which happens when the kernel was built with pahole < 1.26". Full text in
`katana-schedext-20260922.md`.

On `apex1` it started clean:

```
RustLand version 1.1.3 x86_64-unknown-linux-gnu - scx_rustland_core 2.4.14
```

`switch_all=1` is the strong half: the scheduler did not merely load, it took
over scheduling for all tasks. It then detached cleanly on SIGINT and the state
returned to `disabled` with `nr_rejected=0`.

## Why this is worth more than one roadmap row

`Containerfile.core` refuses any kernel whose manifest does not say
`btf_scx=usable`, and its comment justifies the whole tier by predicting that a
COPR fallback "would ship the BTF defect the kernel tier exists to fix, and
nobody would find out until a sched-ext scheduler failed to load on a user's
machine."

Both halves of that are now measured on one machine, hours apart:

- the COPR kernel **does** ship the defect, and a scheduler **does** fail;
- APEX's own kernel **does** fix it.

So `btf_scx=usable` asserts something true, and the tier — which cost this
program weeks — is justified by a controlled experiment rather than by the
argument that motivated it. The prediction was written down in
`katana-schedext-20260922.md` **before** the image was installed, which is what
makes this a test rather than a story told afterwards.

## Also measured across the same reboot

- **NVRAM is byte-identical** before the switch, after the switch and after the
  reboot. katana's `Boot0000 APEX-OS Primary` shares a PARTUUID with
  `Boot0002 Windows Boot Manager`, so this is the check that matters most there;
  `bootc switch` and the reboot touched neither.
- **0 failed system units** on the new image, as on the old.
- BTF is present either way (6,562,087 bytes on `apex1`; 6,599,456 on COPR) —
  which is the point: BTF being *present* was never the question, its kfunc
  prototypes being *correct* was.

## Still could-not-run

`a11y-tree`: katana has no graphical session up, so quickshell is not running
and the AT-SPI tree cannot be walked. Both images carry quickshell-git at
`c6a5160`, which contains the accessibility fix, so the software is in place and
only the session is missing. Recorded as could-not-run rather than passed.
