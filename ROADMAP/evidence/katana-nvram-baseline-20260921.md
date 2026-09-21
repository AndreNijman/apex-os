# katana NVRAM, read live 2026-09-21 — the ESP decision's premise, confirmed

Captured before installing any new image, so a later `efibootmgr -v` diff has
something to compare against. Full capture:
`/var/lab-scratch/katana-install/nvram-before.txt`.

```
BootCurrent: 0000
BootOrder:   0000,0001,0002,0003,0004,0005
Boot0000* APEX-OS Primary   HD(1,GPT,2ba9a2ea-…,0x800,0x64000)/\EFI\APEX\SHIMX64.EFI
Boot0001* APEX-OS           HD(2,GPT,99af3362-…,0x1000,0x100000)/\EFI\FEDORA\SHIMX64.EFI
Boot0002* Windows Boot Mgr  HD(1,GPT,2ba9a2ea-…,0x800,0x64000)/\EFI\MICROSOFT\BOOT\BOOTMGFW.EFI
```

`Boot0000` and `Boot0002` carry the **same PARTUUID**. katana boots APEX from
the ESP Windows created, and it is first in BootOrder — so this machine depends
on another operating system's disk to start. `docs/apex-owns-its-esp.md` asserts
exactly this; here it is measured rather than quoted.

**`Boot0001` already points at katana's own ESP** (`99af3362…`, partition 2,
0x100000 sectors = 512 MiB) and sits second in BootOrder. So the ESP decision's
"cheapest first proof" needs no partitioning on this machine: the target already
exists and already has a loader path. What it needs is for that entry to be
first, and a boot through it, with the Windows ESP left untouched.

Two things this does NOT establish, named rather than glossed:

- Whether `\EFI\FEDORA\SHIMX64.EFI` on `99af3362` is current. The entry exists;
  nothing here booted it.
- Whether the firmware will honour it. `BootNext` is how to find out, and it is
  self-reverting — which is the whole reason `apex-boot-migrate` commits that
  way.

Deployments at capture time: booted
`apex-661a9d80…` (digest `61f7935c…`), rollback `apex-97c9e8f2…` (digest
`55fc9e4e…`). **These are different digests**, which retires a blocker P0-001
has carried: its rollback-reboot row was unverifiable because the rollback slot
held the same commit as the booted one. It no longer does.
