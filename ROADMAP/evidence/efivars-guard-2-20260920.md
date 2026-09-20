# The efivars guard, second pass — which layer prevents, 2026-09-20

Unit `efivars-guard-2`. It exists because the first pass shipped a layer that
prevents nothing and described it as the protection, and the machine proved it
inert four hours later.

**What this unit ran, precisely, because a vague version of this sentence is
the kind of claim it exists to stop.** Four containers, all `--rm`, none
privileged except one: two `ls`/`strings` probes, one of them `--privileged
--pid=host` (§1 — the measurement only means anything with those flags on, and
it reads two paths and exits), and two runs of `bootc install to-disk --help`
in `localhost/apex-os:daily`, unprivileged and without `--pid=host`, to read
§2's text out of the binary rather than from memory. **No install was
performed**, no disk or image file was written, and `efibootmgr` was never
invoked — every `efibootmgr` line quoted below is read out of a log file that
already existed.

## 0. The honest layer table

| layer | kind | what it does |
| --- | --- | --- |
| `bootc install --generic-image` | **PREVENTION** | bootc skips its firmware step entirely. The only thing here that stops the write. |
| `tests/lab/nvram-guard` | **DETECTION** | before/after `efibootmgr -v` + efivarfs `Boot*` digests. Cannot prevent. Has caught this defect **twice** — the only layer that ever has. |
| `--tmpfs /sys/firmware/efi/efivars` | defence in depth, **inert against bootc** | bounds a process that stays inside the container. bootc is not one. |
| caller `-v /sys:/sys` refusals | defence in depth, inert against bootc | same reason. |
| device-target / tmpfs-target refusals | hygiene | keeps the lab off real disks and out of RAM. Nothing to do with NVRAM. |

## 1. Why the mask cannot work — measured on the L16, not deduced

```
$ for s in nsenter /proc/1/ns/mnt /proc/1/root; do strings /usr/bin/bootc | grep -cF -- "$s"; done
1
1
4
```

Host `bootc 1.16.10`; the same three strings are present in `bootc 1.16.13`
inside `localhost/apex-os:daily`. bootc re-enters PID 1's mount namespace —
the host's — for the bootloader step.

```
$ sudo podman run --rm --privileged --pid=host quay.io/fedora/fedora:43 \
      sh -c 'ls /sys/firmware/efi/efivars | wc -l; grep -c efivars /proc/self/mountinfo'
0
0
```

**Unmasked**, `--privileged --pid=host`, and the container sees zero EFI
variables and has no efivars mount of its own. The host's efivarfs was never in
the container's view, so there was nothing there for a tmpfs to cover. Masking
an empty path changes nothing for a process that stays in the container, and
the process in question deliberately leaves it.

The host at the time of these two commands: 164 entries under
`/sys/firmware/efi/efivars`.

## 2. What `--generic-image` does, out of bootc's own mouth

```
$ sudo podman run --rm localhost/apex-os:daily bootc install to-disk --help
      --generic-image
          Perform configuration changes suitable for a "generic" disk image. At the moment:

          - All bootloader types will be installed - Changes to the system firmware will be skipped
```

## 3. The two incidents, corrected

`BOOT-BREAKAGE-2026-09-20.md` says the first was `bootc install to-disk
--via-loopback`. It was not, and the difference is load-bearing because the
repository scan keys on that literal string.

**Both** were `installer/apex-install` running inside `localhost/apex-os:daily`
with `--privileged --pid=host`, on a `to-filesystem` install, against a **loop
device the test suite had already attached**. Neither command contained
`--via-loopback` anywhere. The first run's own log survives:

`/var/lab-scratch/apex-luks-live.oRDxu4/engine-stdout.txt`

```
Installing APEX-OS to /dev/loop1 (entire disk) …
Partitioning /dev/loop1 (EFI, a plain boot partition, and an encrypted volume …)
Installing image: docker://localhost/apex-os:daily
Bootloader: grub
Installing bootloader via bootupd
Executing: "efibootmgr" "-b" "0000" "-B"
…
Executing: "efibootmgr" "--create" "--disk" "/dev/loop1" "--part" "1" "--loader" "\\EFI\\fedora\\shimx64.efi" "--label" "APEX-OS"
…
Boot0000* APEX-OS	HD(1,GPT,ee26313c-6786-4c6b-9520-5cf49abd4e8c,0x800,0x200000)/\EFI\fedora\shimx64.efi
Installation complete!
```

That is byte-for-byte the entry in Andre's write-up. The `01:02:17` in the
write-up is UTC; locally it was **09:02:17 AWST**, and the file's mtime is
`Sep 20 09:02`. The container was the `luks-installer` unit's live suite, run 4.

**Neither run carried `--generic-image`.** `set_nvram_args_for`, the function in
`installer/apex-install` that adds it for a loop-backed target, did not exist
until `68855e92` (2026-09-20 21:25) — and that commit added only the tmpfs. The
flag itself arrived in `f0ec87de` (22:21), whose subject is *"the efivars mask
was not the guard; skip the firmware step instead"* — written after the second
occurrence, which began at 21:53:26 and had written NVRAM by 22:12:15.

The second occurrence has a surviving log too:
`/var/lab-scratch/apex-luks-live.ZAGaYm/engine-stdout.txt`. Launched **21:53:26**
(`efi-before.txt` mtime); the write had landed by **22:12:15**. Its own words,
in order:

```
nvram-guard[luks-live-install]: before: 29 Boot* variables, 26 entries
Installing APEX-OS to /dev/loop1 (entire disk) …
Loopback target: this machine's UEFI boot entries are masked off and will not be touched.
Bootloader: grub
Installing bootloader via bootupd
Executing: "efibootmgr" "-b" "0000" "-B"
…
Executing: "efibootmgr" "--create" "--disk" "/dev/loop1" "--part" "1" …
…
!!! nvram-guard[luks-live-install]: verdict: nvram-changed — this command MOVED THE HOST'S BOOT ENTRIES.
```

**The engine printed the mask's promise and then broke it in the next four
lines.** "this machine's UEFI boot entries are masked off and will not be
touched" is `set_nvram_args_for`'s own `note()` from `68855e92`, the commit that
added the tmpfs and called it the fix. That is the whole finding in six lines of
one log, and `nvram-guard` — the layer the first pass called secondary — is what
turned it into a restore instead of a second live USB.

So nothing in either incident contradicts `--generic-image`'s sufficiency; the
flag was simply absent both times.

## 4. What changed, and how each claim is made by something else

`tests/lab/bootc-install-lab` already passed `--generic-image` — the luks
unit's card says otherwise, and that is stale — but **nothing asserted it**, so
nothing stopped an edit removing it. It is now on its own tagged line with a
named assertion:

* `generic-image-present`, exit **7**, scans the **assembled argv** for the
  literal, and only **after the `bootc install` pair**. A `--podman-arg`
  carrying the same string cannot satisfy it: podman would eat it and bootc
  would never see it.
* An argv with **no `bootc install` at all** fails the assertion loudly rather
  than scanning an empty range and reporting ok.

`tests/test-bootc-install-guard.sh`: **71 passed, 0 failed.** The new cases:

| case | what makes the claim |
| --- | --- |
| the flag reaches bootc | the STUB PODMAN's recorded argv file, position-aware |
| position discrimination | run with `--podman-arg --generic-image`, assert **both** copies land, delete the wrapper's own copy *from the recording*, require the check to reject what is left |
| **the refusal** (direction 2b) | delete the one tagged line from a **copy**, require exit 7, the name `generic-image-present`, podman **never invoked**, **no image created**, and the message to explain the namespace hop |
| the mutation is discriminating | the mutant is asserted to still carry the efivars mask, or this would re-prove direction 2 under a new name |
| a vacuous argv | a mutant whose `bootc install` is renamed must FAIL, and say it would have inspected nothing |

**The damage was never reproduced.** No `bootc install` ran, no container
started, no EFI variable was read or written by the suite: `podman` and
`efibootmgr` are stubs on PATH and the firmware root is a fixture tree. The
claims are about the argv the wrapper builds and about its refusal, and both
are observable without firmware — the same method the existing suite used for
the mask.

## 5. The repository scan was backwards

It flagged a `--via-loopback` command with no `/sys/firmware/efi/efivars` in
it. That is the **inert** property. Concretely, the old predicate would have

* **PASSED** a loopback install that writes this machine's NVRAM (masked, no
  `--generic-image`), and
* **FLAGGED** one that cannot (unmasked, with `--generic-image`).

It now flags a loopback install with no `--generic-image`. The old predicate is
**kept in the suite** and run against the same fixture, asserting that it
passes the dangerous one — if that ever stops holding, the two predicates have
converged and one of them is wrong.

The `violating.sh` fixture claimed to be *"what the 2026-09-20 run looked
like"* and carried `--generic-image`. Had that been a real transcript it would
have meant the flag does not work. It was not a transcript of anything; see §3.
It now carries the efivars tmpfs and is dangerous anyway, which is exactly the
case the old predicate called compliant.

## 6. What the scan still cannot cover, said out loud

The scan matches the literal `--via-loopback`. **Neither real incident
contained that string.** It therefore covers the shape a person types at a
prompt, and covers `apex-install`'s loop-device path **not at all** — that one
is covered by `installer/test-installer-luks.sh`'s NVRAM section and by the
live suite asserting `nvram-guard`'s verdict. This is written into the suite
header, because a scan believed to cover more than it does is how the second
occurrence happened.

## 7. Residuals, stated rather than discovered later

* **`--generic-image` has not been measured on a `to-disk` loopback install.**
  The `luks-installer` unit proved it live on `to-filesystem` (run 8, engine
  exit 0, 41/0, `nvram-guard` verdict `verified`, `Boot0000` still PARTUUID
  `1c417de2-…`). `bootc-install-lab` is `to-disk`. This unit is bounded from
  running one against real firmware, so prevention on the `to-disk` path rests
  on bootc's documented semantics plus the `to-filesystem` evidence, **not on
  a `to-disk` measurement**. Anyone who runs one should do it under
  `nvram-guard` and record the verdict here.
* **The mask is kept and is still inert.** It costs nothing and bounds a
  different class of process. It is labelled in every file that mentions it.
  If a future reader deletes it, nothing about firmware safety changes.
* **Nothing stops somebody typing `podman run --privileged` at a prompt.**
  Unchanged from the first pass, and still true.
