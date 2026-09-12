# Virtual machines on APEX

`apex vm` is APEX's virtualization UX: full guests with their own kernel,
rootless, headless, and enforcing Secure Boot by default.

This page records what is built, what is proven, and — in the last section —
what is **not** built, because a page that lists six flows and describes three
is how a gap gets shipped.

## Why `apex vm` and not `apex env`

They solve different problems and share no state.

| | `apex env` (capsule) | `apex vm` |
|---|---|---|
| kernel | the host's | its own |
| your home | mounted inside | not reachable |
| host filesystem | at `/run/host` | not reachable |
| a guest crash | kills a container | kills a VM |
| another OS | no | yes |

A capsule is the right answer to "`pip install --user` wants a mutable `/usr`".
A VM is the right answer to "boot another operating system", "test the
installer", "give a task a kernel it can take down", and "run code I do not
want near my files".

## The stack is not in the image, and that is deliberate

`qemu`, `libvirt`, OVMF, `swtpm` and `virtiofsd` are userspace. AGENTS.md's
dividing line is that an out-of-tree kernel module must be built and signed
into the image because it cannot be added at runtime under Secure Boot, and
that userspace must **not** be baked in. KVM needs no out-of-tree module at
all: `kvm`, `kvm_intel` and `kvm_amd` are in-tree in the signed kernel and are
already on every APEX machine.

So the engine ships in the image and the stack arrives on demand:

```
apex vm doctor
sudo apex install qemu-kvm libvirt-daemon-kvm libvirt-client edk2-ovmf \
                 swtpm swtpm-tools virtiofsd mtools dosfstools
```

Three of those are in the line because leaving them out produces a failure that
reads as something else, and all three were found by running the flows rather
than by reading a package list:

* **`swtpm-tools`** carries `swtpm_setup`, which libvirt runs to create a
  domain's TPM state. `swtpm` neither requires nor recommends it on Fedora 43
  and does not ship it. Without it every `--tpm` domain defines cleanly and
  then fails at start. `apex vm doctor` probes it as its own row.
* **`mtools`** and **`dosfstools`** are what `apex vm run` builds its volumes
  with (`mkfs.vfat`) and reads the egress volume back with (`mcopy`).

`apex vm doctor` probes each piece and prints exactly that line when something
is missing. Every other verb refuses with the same text **before** creating
anything, rather than failing halfway through defining a domain.

`Containerfile.base` asserts both halves — that the engine is present and that
`qemu-system-x86_64` and `virsh` are **not** — so a later change that layers
the stack into the image has to argue for it in a diff.

## Rootless, session-scoped, and nothing left on the host

Every domain lives at `qemu:///session`, libvirt's per-user URI, served by a
`virtqemud` that systemd autospawns for the calling user.

That is structural, not a preference:

* `qemu:///system` needs a root daemon, a polkit prompt to reach it, and puts
  every user's domains in one namespace.
* the system daemon's `default` network is `virbr0` — a host bridge, a
  `dnsmasq`, and forwarding rules that outlive the VM that asked for them.
  Creating one is a change to the **host's** networking.

Session mode has no `default` network to start, so the refusal is a property of
the URI rather than of the engine's care. `--network bridge` is refused by name
with that reason, and points at `virsh -c qemu:///system` for somebody who
genuinely needs a LAN-reachable guest.

Networking is therefore one of two things:

* `--network user` (the default) — qemu's user-mode stack. Outbound IPv4, no
  inbound anything, nothing on the host.
* `--network none` — no interface at all.

## Headless, always

There is no viewer, no SPICE, no VNC, and no `--graphics`. The console is a
serial port written to a file plus a pty:

```
apex vm console NAME          # attach; Ctrl-] detaches
apex vm console NAME --log    # print what it has written so far
```

A VM viewer is a second product — a window, a clipboard channel, a USB
redirection path, a remote protocol — and shipping half of one is worse than
shipping none. The absence is asserted rather than reviewed: the CLI test
`there_is_no_viewer_verb_and_no_graphics_flag` and a `Containerfile.base` grep
both fail if a `<graphics>` element or a viewer verb appears.

## The six flows

P2-008's acceptance line is "KVM/QEMU/libvirt VM create/snapshot/share/USB/TPM/
Secure Boot flows work". Read as a list that is six flows, so here is each one
and where it stands.

### create

```
apex vm create dev --memory 4096 --cpus 4 --disk 40G
apex vm start dev
apex vm console dev
```

Secure Boot enforcing and an emulated TPM 2.0 by default. `--import FILE`
starts from an existing disk image — **converted** with `qemu-img`, never used
as a backing file. Not a backing file because that makes the VM a delta over a
path the user may move; not a plain copy because the domain declares
`<driver type='qcow2'/>` and a raw image would define cleanly and then fail at
start with qemu refusing a magic number. Converting also gives an imported disk
the internal snapshots `apex vm snapshot` needs, which a raw image cannot hold
at all.

### Secure Boot

`<os firmware='efi'>` declares two features and **no paths**:

```xml
<firmware>
  <feature enabled='yes' name='enrolled-keys'/>
  <feature enabled='yes' name='secure-boot'/>
</firmware>
<loader secure='yes'/>
```

libvirt resolves those against the qemu firmware descriptors in
`/usr/share/qemu/firmware` and picks a build. On Fedora 43 that is
`OVMF_CODE_4M.secboot.qcow2` with `OVMF_VARS_4M.secboot.qcow2`.

The `enrolled-keys` feature is the half that is easy to miss and is the
difference between enforcement and theatre. `OVMF_VARS_4M.qcow2` — one word
away — is the **empty** variable store: no PK, no KEK, no db, so the firmware
comes up in setup mode and boots anything while the domain still says
`secure='yes'`. The engine's first implementation derived the varstore path
from the code path by stripping `_CODE`, which produced exactly that. It is
recorded in the engine's own comments so the next person does not re-derive it.

`<smm state='on'/>` is not optional either: without System Management Mode the
guest can write the variable store, and libvirt refuses `<loader secure='yes'>`
on a domain that lacks it.

`--uefi-vars FILE` starts from a variable store you supply, which is how a
kernel signed with a key that is not in the shipped firmware gets tested.

### TPM

```xml
<tpm model='tpm-crb'>
  <backend type='emulator' version='2.0'/>
</tpm>
```

`tpm-crb` rather than `tpm-tis`, because CRB is the interface a modern UEFI
guest expects. libvirt starts and stops `swtpm` with the domain.

### snapshot

```
apex vm snapshot create dev before-update
apex vm snapshot revert dev before-update
```

The disk **and** the UEFI variable store, together. That pairing is the part a
snapshot implementation usually gets wrong: the variable store holds the
guest's boot entries and enrolled keys, so restoring the disk without it leaves
a firmware pointing at a boot entry the disk no longer has.

This is why `apex vm create` writes the variable store as **qcow2**. libvirt
refuses an internal snapshot of a domain whose nvram is raw pflash — there is
nowhere in a raw image to put one — and that refusal reads as a snapshot bug
rather than as a firmware-format one.

### share

```
apex vm create dev --share /srv/data:data --share-ro /srv/reference:ref
apex vm share add dev /srv/more:more
```

virtiofs, not 9p. In the guest:

```
mount -t virtiofs data /mnt
```

The element people forget is not the filesystem, it is the memory:

```xml
<memoryBacking>
  <source type='memfd'/>
  <access mode='shared'/>
</memoryBacking>
```

A vhost-user filesystem maps the guest's RAM into `virtiofsd`. With anonymous
private memory there is nothing to map and qemu exits with "unable to map
backing store for guest RAM" — which reads as a memory problem. The engine
emits it only when a share is configured, and `apex vm share add` **refuses**
on a domain that was created without one rather than defining a device that
would fail at start.

### USB

```
apex vm usb attach dev 046d:c52b
```

`VENDOR:PRODUCT` is what `lsusb` prints. The engine checks `lsusb` first,
because libvirt's "no such device" names a bus and device number the user never
typed — and **refuses when `lsusb` is not installed at all**, rather than
skipping the check. "The question could not be asked" is not "the answer is
yes", and a `<hostdev>` for a device that is not there gives a domain that
defines and then will not start. `apex vm create --usb` makes the same check;
it used not to, so the same device reached through two verbs got two answers.

Two things are emitted that are easy to miss: `<controller type='usb'
model='qemu-xhci' ports='15'/>`, because q35's implicit controller has no free
port for a hostdev and the failure reads as a problem with the device; and
`managed='yes'` on the `<hostdev>`, which is what makes libvirt detach the
device from the host driver and give it back afterwards. While the VM holds the
device the **host cannot use it**, and `apex vm usb attach` says so before it
happens.

## Everything else the verb does

```
apex vm list            what exists, and the state libvirt says each one is in
apex vm list --json     the same, for a script
apex vm info dev        what the VM was made from — the record, not the domain
apex vm stop dev        ask the guest to shut down
apex vm stop dev --force   pull the plug
apex vm rm dev          undefine it and delete its disk
apex vm rm dev --keep-disk   undefine it and keep the qcow2
```

`apex vm list` reports a VM whose daemon cannot be reached as `unknown`, never
as `shut off`. "The question could not be asked" and "the answer is no" are
different answers, and this repository has shipped the confusion between them
before.

`apex vm info` reads the record written at create time — memory, vCPUs, whether
Secure Boot is enforcing, whether there is a TPM, the network mode, the shares
and the USB devices. It is a file rather than a `virsh dumpxml`, because "does
this VM enforce Secure Boot" is the question asked six months later and
`dumpxml` answers it only while the domain is still defined.

`apex vm rm` undefines with `--nvram`, so a VM recreated with the same name
does not inherit the old one's enrolled keys. The recursive removal of the VM's
directory is fenced four ways — the name must match a narrow pattern, the final
path component must not be a symlink, the path is resolved with `realpath`, and
the result must equal exactly `<root>/<name>`. Anything else hard-exits without
removing a thing.

## Disposable VMs for agent tasks

```
apex vm run --image guest.qcow2 \
    --copy-in /home/u/src \
    --egress report.json --egress-to /home/u/results \
    -- ./build-and-report.sh
```

`apex vm run` creates a VM, runs one task in it, and deletes the whole thing —
domain, disks and volumes. It is a different and stronger boundary than
`apex disposable`, which is a container that can reach the user's home through
`/run/host` and says so at length. Here the guest is another kernel with no
view of the host filesystem.

Two volumes, and both are plain FAT images rather than shares:

* **`in.img`**, labelled `APEXIN`, attached **read-only**. Built on the host
  from the `--copy-in` paths plus a `task.sh` holding the command.
* **`out.img`**, labelled `APEXOUT`, attached read-write and blank. The guest
  writes whatever it likes here.

Nothing is bind-mounted and no virtiofs share is used, because a share is a
live window into a host directory and this design wants a boundary that can be
inspected **after** the fact. When the guest powers off, the host reads
`out.img` with `mcopy` — userspace FAT, no mount, no loop device, no root — and
copies out exactly the filenames given to `--egress`.

The loop is over the **nominations**, never over the contents of the volume.
That direction is the security property: iterating the image and copying what
matched would let the guest decide what leaves by choosing filenames.

Default-deny at both ends. Without `--copy-in` the guest gets only the task
script. Without `--egress-to` nothing leaves at all, whatever `--egress` says.
`--egress` takes a plain filename: no directory component, no `..`, no
wildcard.

`--console-to FILE` saves the guest's serial output before teardown deletes it.
It is opt-in rather than a default, and that is the argument rather than
caution: the serial log is bytes the guest chose to write, so it leaves only
where the caller named a destination — the same rule as every other file. It is
also the only diagnostic a guest that never mounted `APEXIN` can leave behind,
and without it "an empty egress and a timeout warning" came with no way to find
out why.

A third decision is separate from both: a nominated file that would land on
top of something already at the destination is **not** copied unless `--force`
says so. The file coming out was written by code the user ran in a VM because
they did not trust it, so replacing something of theirs with it is a decision
rather than a default. `--egress-to` is also refused if it resolves inside the
VM root, because teardown deletes that tree — every file would be reported
copied and none would survive.

**There is no network and no flag to give it one.** `apex vm run --network`
is refused with that reason rather than ignored: a disposable VM exists to run
something the user does not trust, and an outbound interface would make the
file-egress boundary decorative.

### The guest contract

`apex vm run` does **not** build a guest image and does not install anything
into one. `--image` is required and the image must:

* mount the filesystem labelled `APEXIN` and run `task.sh` from it;
* write anything it should hand back to the filesystem labelled `APEXOUT`;
* power off.

A guest that does none of that produces an empty egress and a timeout warning,
which is the correct outcome rather than a silent success.

## What is proven, and how

Two suites, and they prove different things.

`tests/test-apex-vm.sh` drives the shipped engine with recording stubs for
`virsh`, `qemu-img`, `mkfs.vfat`, `mcopy`, `lsusb`, `swtpm` and `swtpm_setup`
on `$PATH` and asserts the exact argv and the exact XML. That is the claim
"the engine emits the right domain", and it runs everywhere.

`tests/vmlab/run-vmlab` boots real guests through the engine against a real
`virtqemud` inside `bootlab/`'s container, and emits a verdict per flow. That
is the different claim "libvirt accepts it, qemu starts it, the firmware
enforces it and the guest can see it". It uses the chaos harness's three-state
verdict (`tests/chaos/lib.sh`) with one arm renamed: a flow that could not be
exercised reports `could-not-run` **with a reason**, never a pass.

```
podman build -t apex-bootlab bootlab/
./tests/vmlab/run-vmlab                 # all seven
./tests/vmlab/run-vmlab --flow share    # one
./tests/vmlab/run-vmlab --no-kvm        # the could-not-run arm, on purpose
```

Every assertion is made either by the guest about itself over the serial
console or by a tool reading an artefact from outside. None is made against a
string the lab also produced.

### The last full run, on a laptop with KVM

| flow | verdict | what the guest or the host observed |
|---|---|---|
| create | verified (13) | libvirt defined and started it; the guest reached userspace, got the command line the UKI was *signed* with, and powered itself off; `rm` left no domain and no directory |
| Secure Boot | verified (8) | three boots of the same signed UKI: against the APEX variable store it boots and reports `SecureBoot=1 SetupMode=0`; against libvirt's own `enrolled-keys` store (Microsoft's db) the firmware **refuses it**; against a key-free store it boots in setup mode — which is what shows the refusal was about keys |
| TPM | verified (5) | `/sys/class/tpm/tpm0` with `tpm_version_major=2` inside the guest, and **absent** in a `--no-tpm` guest |
| snapshot | verified (10) | a tally the guest keeps on its own ESP: boot (1), snapshot, boot (2), revert, boot — and it is 2, not 3. `qemu-img snapshot -l` shows the snapshot in **both** `disk.qcow2` and `nvram.qcow2` |
| share | verified (16) | two virtiofs devices; the guest mounts both tags, reads the host's file, writes through the read-write one so the file appears **on the host**, and is refused on the read-only one |
| USB | **could-not-run** | see below |
| egress (P2-009) | verified (16) | the hostile task's unnominated file did not leave, while the guest's own log proves it existed; the guest sees only `lo`; the copy-in volume refused its write; and an engine mutated to iterate the volume instead of the nominations **does** leak it |

After all seven: no domains, **no libvirt networks**, and an empty VM root.

Two caveats, stated rather than left in the verdict file:

* **USB passthrough is `could-not-run`, and always will be from a suite.** It
  means taking a physical device away from the machine running the tests. What
  the lab does exercise against a real libvirt is the refusal when `lsusb`
  cannot be run at all, the same refusal from `create --usb`, the malformed-id
  refusal, and that no `<hostdev>` reached the domain in any of them.
* **The share flow runs `virtiofsd` through a wrapper adding `--sandbox
  none`.** Its default `namespace` sandbox unshares a user namespace and mounts
  its own `/proc`, which cannot nest inside the lab container's user namespace
  — measured three ways: `namespace` fails with `MountProc` EPERM (also under
  `seccomp=unconfined`), `chroot` refuses for a non-root user, `none` works.
  libvirt's `<binary>` offers only the first two. Everything else in that flow
  is the shipped path, and the wrapper logs the argv it substituted into the
  run's bundle.

## What is not built

Stated rather than left to be discovered:

* **Host USB passthrough is not exercised live.** The XML, the `lsusb` guard
  and the argv are tested; actually detaching a device from the developer's
  laptop to hand to a guest is not something a test suite should do, and
  session-mode passthrough additionally needs the `/dev/bus/usb` node to be
  readable by the user. The lab records this as `could-not-run` with that
  reason.
* **There is no VM viewer, and there will not be one from this verb.**
* **`--network bridge` is refused**, not implemented. A guest that other
  machines can reach needs `qemu:///system`, and putting a host bridge on
  somebody's laptop is not a thing `apex vm` will do on their behalf.
* **No guest image catalogue.** `apex vm create` makes a blank disk or imports
  one you already have; it does not download Fedora for you. `apex vm run`
  likewise requires an image that already satisfies the guest contract above.
* **`apex vm run` does not put an agent CLI into a guest.** Running `claude` or
  `codex` inside a disposable VM needs a guest image that carries it, and
  building that image is not part of this verb. What is built and proven is the
  boundary: the volumes, the nomination-only egress, and the teardown.
* **No live migration, no CPU pinning, no PCI/GPU passthrough.**
* **virtiofsd's own namespace sandbox is not exercised by the lab**, only by a
  real machine. See the caveat above.
* **The lab boots guests that are shut off.** `apex vm snapshot` on a *running*
  domain, `apex vm stop` over ACPI (the lab's guests power themselves off, so
  `shutdown` is never sent), `usb detach` and `share remove` are argv-tested
  and not live-tested.
