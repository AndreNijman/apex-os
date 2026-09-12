# p2-virt — first-party virtualization UX and disposable VM agent execution

Tasks: **P2-008** (`todo`) and **P2-009** (`todo`, depends on P2-008).

Repo: apex-os. Branch `task/p2-virt`, from `origin/roadmap/v2.2`.
Worktree `/var/tmp/apex-work/wt-p2-virt`.

## Criteria, quoted from roadmap.yaml

- P2-008: "KVM/QEMU/libvirt VM create/snapshot/share/USB/TPM/Secure Boot flows
  work."
- P2-009: "Agent task can run in disposable VM with explicit file egress."

Both are one line and both are wider than they look. Read them as a list:
create, snapshot, share (filesystem passthrough), USB passthrough, vTPM, and
Secure Boot are six separate flows, and a unit that builds three and claims six
is the failure mode this program keeps catching.

## What already exists — do not re-derive, build on it

P1-062 landed a working VM lab on this branch **today** (merge `a0341a50`,
`tests/chaos/`). It already solves most of what a first attempt at this unit
would spend its round solving:

- `bootlab/` builds a container that carries its own kernel and boots guests
  under **Secure-Boot-enforcing OVMF with a software TPM** — so the vTPM and
  Secure Boot halves of P2-008 have a working reference in-tree.
- `tests/chaos/cases/reboot-loop.sh` boots five OVMF guests, reads the ESP out
  of the FAT, and collects serial logs. That is the shape of an automated VM
  assertion here.
- `tests/chaos/lib.sh` carries the **verdict type with a could-not-inject arm**.
  Reuse it: a VM flow that could not be exercised because the runner has no KVM
  must say so, not pass. `accel` detection is already written.

## Constraints

- **Headless.** No VM viewer window, ever. Serial console and disk inspection.
- The L16 has KVM; **GitHub runners mostly do not** — the chaos lab probes for
  it and records which. Any CI assertion you add must degrade to could-not-run
  with a reason rather than fail or fake it.
- **Never touch the host's boot path or its libvirt default network** in a way
  that outlives the test. A rollback is staged on this laptop waiting for Andre
  to reboot.
- P2-009's "explicit file egress" is the security half and the part worth
  testing hostilely: prove a file the task did NOT nominate cannot leave the
  VM, rather than asserting that the nominated one can.

## NEXT

Nothing done yet. Survey what `apexd` already has for VMs before writing
anything — `apex env` manages containers and may own the verb space you need.
