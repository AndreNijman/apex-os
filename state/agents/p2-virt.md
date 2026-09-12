# p2-virt — first-party virtualization UX and disposable VM agent execution

Tasks: **P2-008** and **P2-009**.

Repo: apex-os.
Round 1 branch `task/p2-virt` — merged into `roadmap/v2.2` as `bece32d3`.
Round 2 branch `task/p2-virt-2`, worktree `/var/tmp/apex-work/wt-p2-virt2`,
from `origin/roadmap/v2.2` @ `2aa9ed45`.

## Criteria, quoted from roadmap.yaml

- P2-008: "KVM/QEMU/libvirt VM create/snapshot/share/USB/TPM/Secure Boot flows
  work."
- P2-009: "Agent task can run in disposable VM with explicit file egress."

Six flows in P2-008, read as a list: create, snapshot, share, USB, TPM, Secure
Boot. A unit that builds three and claims six is the failure this program keeps
catching.

## Round 1 (merged) — the engine

`files/system/libexec/apex-vm`: rootless (`qemu:///session`, always), headless
(serial only, no `<graphics>` anywhere), Secure-Boot-enforcing by default, with
`apex vm run` for P2-009. A clap surface in `apexd/apex/src/vm.rs`.
`tests/test-apex-vm.sh` asserted the argv and the XML against stubs.

Round 1 ended with the engine **never having booted a guest**, and with
`docs/virtualization.md` and `tests/test-apex-vm.sh` both citing
`tests/vmlab/run-vmlab` — a live lab that **did not exist**.

## Round 2 (this branch) — the lab, and what it found

Four commits on `task/p2-virt-2`, all pushed.

`tests/vmlab/run-vmlab` + `tests/vmlab/mk-guest`: real guests booted through the
shipped engine against a real `virtqemud`, inside `bootlab/`'s container, as a
**non-root user** (uid 0's `qemu:///session` resolves to the SYSTEM daemon's
socket, so a root lab would silently test the daemon apex-vm refuses to use).
Verdicts are the chaos harness's, with `could-not-inject` renamed
`could-not-run`.

### Live verdicts, last full run on the L16

| flow | verdict |
|---|---|
| create | verified |
| secure-boot | verified |
| tpm | verified |
| snapshot | verified |
| share | verified (virtiofsd wrapped with `--sandbox none`; see below) |
| usb | **could-not-run** — passthrough means taking a device off this laptop |
| egress (P2-009) | verified, including the falsifying mutation |

After all seven: no domains, **no libvirt networks**, empty VM root.

### Three defects the lab found that no stub could

1. `INSTALL_HINT` did not name `swtpm-tools`, and `have_swtpm` did not probe
   `swtpm_setup` — so `apex vm doctor` said "Every flow is available" on a
   machine where every `--tpm` domain defines and then fails at start. `swtpm`
   neither Requires nor Recommends `swtpm-tools` on Fedora 43 (checked). The
   line also lacked `mtools`/`dosfstools`, which `apex vm run` needs.
2. The USB guard read `command -v lsusb && ! lsusb -d ID && die`, which SKIPS
   the check on a machine without usbutils — "the question could not be asked"
   reported as "the answer is yes". `create --usb` made no check at all.
3. `apex vm run` deleted its own only diagnostic: teardown removes the VM
   directory, serial log included. `--console-to FILE` saves it, opt-in, with
   the same validation and no-clobber rule as `--egress-to`.

`tests/test-apex-vm.sh`: 106 → 133 assertions, 0 failed. Each new one was shown
to fail against a mutated engine, restored byte-identical.

## NEXT

**Round 2 is complete and pushed. Do not re-derive any of the above.** Read
`docs/virtualization.md` first — it now carries the live verdict table, the two
caveats, and a "what is not built" list that is accurate.

If a round 3 is dispatched, these are the gaps, in order of how much they are
worth:

1. **`apex vm snapshot` on a RUNNING domain.** The lab only snapshots a
   shut-off one. libvirt's rules for an internal snapshot of a live domain with
   a TPM emulator and qcow2 nvram are version-specific and may refuse; if they
   do, that is a defect to record, not to paper over.
2. **`apex vm stop` over ACPI.** The lab's guests power themselves off, so
   `virsh shutdown` is never sent and the "a guest with no ACPI handler will
   ignore it" message is untested live. Needs a guest that stays up.
3. **`share remove` and `usb detach`** are argv-tested only.
4. **A late `share add` that actually boots.** The refusal on a share-less
   domain is live-tested; the accepted path (add to a domain that already has
   memfd backing, then boot and mount the new tag) is not.
5. **virtiofsd's default `namespace` sandbox.** The lab wraps it with
   `--sandbox none` because the namespace sandbox cannot nest inside the
   container's user namespace (measured: `MountProc` EPERM, also under
   `seccomp=unconfined`; `chroot` needs root; `none` works). Proving the
   default needs a lab that is not in a container — which means installing the
   stack on a machine, which AGENTS.md's drift rule argues against. State it,
   do not quietly "fix" it.
6. **A guest image that carries an agent CLI.** `apex vm run` proves the
   BOUNDARY — volumes, nomination-only egress, teardown — and deliberately does
   not build a guest. Putting `claude` or `codex` in one is a separate piece of
   work and `docs/virtualization.md` says so.

Do **not**: install the virt stack on the L16 (the live `apex-pkg` has no
`rpm -qf` image-owner guard and `etc.list` holds `passwd`/`shadow` — that is
the katana `/etc` failure path); open a VM viewer; or let the lab define a
libvirt network.
