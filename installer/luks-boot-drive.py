#!/usr/bin/env python3
"""luks-boot-drive.py — boot a disk THIS INSTALLER PRODUCED under OVMF, type
the LUKS passphrase on the emulated keyboard through QMP, and report whether
the guest pivoted into the real root.

WHY THIS EXISTS, AND HOW IT DIFFERS FROM keymap-boot-drive.py. That script
boots the shipped initramfs directly (`-kernel`/`-initrd`) against a bare
256 MB LUKS volume with no partition table, no ESP and no firmware at all — it
measures the keymap-to-initrd channel and nothing about the bootloader. This
script boots the WHOLE DISK apex-install wrote — GPT, ESP, plain /boot, the
LUKS2 root — through real UEFI firmware, exactly as a machine would: OVMF
loads shim from the ESP, shim loads GRUB, GRUB loads the kernel and initrd
named in the BLS entry bootupd wrote, and only then does dracut ask for the
passphrase. Nothing here is a lab fixture; every byte booted is what the
engine put on the disk.

WHY THE VARSTORE IS PRISTINE, NOT THE LAB'S SIGNED CHAIN. files/scripts/boot-
v2/run-scenarios enrols an APEX TEST certificate into a fresh varstore for its
own self-signed UKI scenarios. That chain would not trust the real Fedora
shim/GRUB bootc actually installs, so using it here would fail Secure Boot for
a reason that has nothing to do with this installer. The stock 4 MB OVMF
varstore this lab's own image ships (OVMF_VARS_4M.qcow2) carries no enrolled
PK/db at all — confirmed by run-scenarios' own `scenario_prereq`, which calls
`ovmf_vars_template()` and that function DIES if the file has PK or db already
enrolled. So this boot happens in UEFI Setup Mode: the firmware verifies
nothing and shim/GRUB load unconditionally. That is an honest limitation, not
a bug, and the caller must say so rather than call this "Secure Boot
enforcing" — it proves the disk boots, not that it is signed correctly.

WHY NO NVRAM GUARD. Unlike the install phase (a privileged, --pid=host
container that can re-enter the HOST's mount namespace and rewrite the host's
real UEFI boot entries — see BOOT-BREAKAGE-2026-09-20.md), this process is an
UNPRIVILEGED qemu reading two pflash FILES inside a bind-mounted work
directory and a disk IMAGE FILE. There is no host efivarfs anywhere near it.
Every other OVMF scenario in this lab (run-scenarios) boots the same way with
no guard, for the same reason.

WHY NO `-no-reboot` ON THE FIRST RUN. If OVMF cannot find a working boot
entry (a fresh varstore has none, and this loopback install passed
`--generic-image` specifically so no Boot#### variable was ever written
anywhere) it may fall through to the removable-media path
(\\EFI\\BOOT\\BOOTX64.EFI) and, on failure, reset. `-no-reboot` would turn
that into a bare "guest died" with no way to tell a firmware failure from a
kernel panic. This driver instead watches the serial and debugcon logs and
lets the caller's wall-clock --timeout be the only thing that ends the run.

WHY THE READINESS MARKER IS THE PROMPT ITSELF, NOT A CUSTOM PROBE UNIT.
keymap-boot-drive.py boots a synthetic dracut hook it built for that lab
fixture. This script boots the SHIPPED, unmodified initramfs — adding a probe
unit would mean testing a different disk than the one under test. So it waits
for the literal text systemd's console ask-password agent prints
(`Please enter passphrase`), which is a stable, long-standing format string in
systemd's ask-password-api and is what a person watching the console would
also wait for.
"""
import argparse, json, os, socket, subprocess, sys, time

def qmp_connect(path, deadline):
    while time.time() < deadline:
        try:
            s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            s.connect(path)
            return s
        except OSError:
            time.sleep(0.2)
    return None

class Qmp:
    def __init__(self, sock):
        self.s = sock
        self.f = sock.makefile("rwb")
        self.f.readline()                      # greeting
        self.cmd("qmp_capabilities")
    def cmd(self, name, **args):
        msg = {"execute": name}
        if args:
            msg["arguments"] = args
        self.f.write((json.dumps(msg) + "\n").encode())
        self.f.flush()
        while True:
            line = self.f.readline()
            if not line:
                raise RuntimeError("qmp closed")
            d = json.loads(line)
            if "event" in d:
                continue
            return d
    def send_key(self, qcode, hold=60):
        self.cmd("send-key",
                 keys=[{"type": "qcode", "data": qcode}],
                 **{"hold-time": hold})

# Only what a lowercase-letters-and-digits passphrase needs. Deliberately no
# uppercase, punctuation or space: those need shift/qcode combinations this
# driver does not implement, and the property under test is the boot pivot,
# not the keyboard layout (that is test-installer-keymap-boot.sh's subject).
KEYS = {c: c for c in "abcdefghijklmnopqrstuvwxyz0123456789"}

def read(path):
    try:
        with open(path, errors="replace") as f:
            return f.read()
    except FileNotFoundError:
        return ""

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--work", required=True)
    ap.add_argument("--name", required=True)
    ap.add_argument("--code", required=True, help="raw OVMF_CODE pflash image")
    ap.add_argument("--vars-template", required=True,
                     help="raw OVMF_VARS pflash image; COPIED, never booted in place")
    ap.add_argument("--disk", required=True, help="the whole-disk image the installer wrote")
    ap.add_argument("--passphrase", required=True)
    ap.add_argument("--timeout", type=int, default=240)
    ap.add_argument("--mem", type=int, default=3072)
    ap.add_argument("--accel", default="kvm:tcg")
    a = ap.parse_args()

    serial = os.path.join(a.work, "serial-%s.log" % a.name)
    dbg    = os.path.join(a.work, "%s.ovmf.log" % a.name)
    qmp    = os.path.join(a.work, "qmp-%s.sock" % a.name)
    qerr   = os.path.join(a.work, "qemu-%s.err" % a.name)
    myvars = os.path.join(a.work, "vars-%s.fd" % a.name)
    for p in (serial, dbg, qmp, qerr, myvars):
        if os.path.exists(p):
            os.unlink(p)
    open(serial, "w").close()
    open(dbg, "w").close()

    # A private, writable copy. qemu mutates this in place; the template must
    # stay pristine for a second boot (e.g. a future Secure-Boot-enrolled run).
    with open(a.vars_template, "rb") as src, open(myvars, "wb") as dst:
        dst.write(src.read())

    cmd = ["qemu-system-x86_64",
           "-machine", "q35,smm=on,accel=%s" % a.accel, "-cpu", "max",
           "-m", str(a.mem), "-smp", "2",
           "-global", "driver=cfi.pflash01,property=secure,value=on",
           "-drive", "if=pflash,unit=0,format=raw,readonly=on,file=%s" % a.code,
           "-drive", "if=pflash,unit=1,format=raw,file=%s" % myvars,
           "-drive", "if=virtio,format=raw,file=%s,media=disk" % a.disk,
           "-debugcon", "file:%s" % dbg, "-global", "isa-debugcon.iobase=0x402",
           "-serial", "file:%s" % serial,
           "-qmp", "unix:%s,server=on,wait=off" % qmp,
           "-display", "none", "-nodefaults"]

    err = open(qerr, "wb")
    proc = subprocess.Popen(cmd, stdout=err, stderr=err)
    deadline = time.time() + a.timeout
    prompt_seen = False
    typed_at = None
    verdict = "no-verdict"
    try:
        s = qmp_connect(qmp, min(deadline, time.time() + 30))
        if s is None:
            verdict = "qmp-never-answered"
            raise SystemExit
        q = Qmp(s)

        # ── wait for the literal ask-password prompt, never a fixed delay ──
        # Typing during GRUB is actively dangerous: the passphrase used by
        # the caller may contain letters GRUB's own menu binds (e.g. `c` for
        # its command line, `e` for edit), and a delay-based guess has no way
        # to know whether the guest is at GRUB, mid-kernel-boot, or already
        # at the prompt.
        while time.time() < deadline:
            if "Please enter passphrase" in read(serial):
                prompt_seen = True
                break
            if proc.poll() is not None:
                verdict = "guest-exited-before-prompt"
                raise SystemExit
            time.sleep(0.5)
        else:
            verdict = "prompt-never-appeared"
            raise SystemExit

        # ask-password uses TCSADRAIN, not TCSAFLUSH, so a short settle here
        # costs nothing: characters typed a moment early are not discarded.
        time.sleep(3)
        for ch in a.passphrase:
            if ch not in KEYS:
                verdict = "passphrase-has-untypeable-char:%r" % ch
                raise SystemExit
            q.send_key(KEYS[ch])
            time.sleep(0.1)
        q.send_key("ret")
        typed_at = time.time()

        # ── the pivot proof: dracut's own "Switching root" line ────────────
        # This is the property the recipe asks for: not that cryptsetup
        # printed something hopeful, but that dracut actually handed off to
        # the real root filesystem, which it only does after the LUKS device
        # it was told to wait for (rd.luks.uuid=/rd.luks.name=) is open.
        while time.time() < deadline:
            txt = read(serial)
            if "Switching root" in txt:
                verdict = "switched-root"
                break
            if "Failed to activate with specified passphrase" in txt:
                verdict = "passphrase-rejected"
                break
            if proc.poll() is not None:
                verdict = "guest-exited-after-typing"
                break
            time.sleep(0.5)
        else:
            verdict = "timeout-after-typing"
    except SystemExit:
        pass
    finally:
        for _ in range(20):
            if proc.poll() is not None:
                break
            time.sleep(0.5)
        if proc.poll() is None:
            proc.kill()
            proc.wait()
        err.close()

    txt = read(serial)
    print("verdict=%s" % verdict)
    print("qemu-rc=%s" % proc.returncode)
    print("prompt-seen=%s" % ("yes" if prompt_seen else "no"))
    print("typed=%s" % ("yes" if typed_at else "no"))
    print("switched-root=%s" % ("yes" if "Switching root" in txt else "no"))
    print("welcome-seen=%s" % ("yes" if "Welcome to" in txt else "no"))
    return 0

if __name__ == "__main__":
    sys.exit(main())
