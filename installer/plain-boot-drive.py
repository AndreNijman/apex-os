#!/usr/bin/env python3
# ─────────────────────────────────────────────────────────────────────────────
#  plain-boot-drive.py — boot an UNENCRYPTED disk this installer wrote, under
#  OVMF, and read the verdict off the guest's own serial console.
#
#  WHY A SECOND DRIVER AND NOT installer/luks-boot-drive.py.
#  That one exists to type a passphrase: it waits for dracut's literal "Please
#  enter passphrase" prompt, sends qcodes through QMP, and only then looks for
#  "Switching root". An unencrypted disk never draws that prompt, so every one
#  of those steps would time out on a disk that is booting perfectly. Rather
#  than add a "--passphrase ''" mode that silently skips half of the other
#  file's reason for existing, this is the same qemu invocation with the typing
#  removed and one thing ADDED: a boot is not finished at "Switching root".
#
#  WHAT "BOOTS" MEANS HERE, STATED BEFORE THE RUN RATHER THAN AFTER.
#  Two separate observations, both made by the GUEST about itself:
#
#    switched-root   dracut handed off to the real root filesystem. This is the
#                    property luks-boot-drive.py stops at, and on its own it
#                    only proves firmware -> shim -> GRUB -> kernel -> pivot.
#    reached         the installed systemd got somewhere a user could log in:
#                    a getty prompt ("login:"), or "Reached target Multi-User
#                    System" / "Graphical Interface". WHICH one fired is
#                    printed, never collapsed into a boolean — a guest that
#                    reaches multi-user but never draws a getty is a different
#                    machine from one that does, and the caller should see it.
#
#  NO KVM IS ASSUMED. --accel defaults to kvm:tcg so the same command answers
#  on a machine without /dev/kvm, slowly. The caller decides the timeout.
#
#  NOTHING HERE TOUCHES THE HOST'S FIRMWARE. Two pflash FILES and one disk
#  IMAGE FILE, all inside --work, read by an ordinary qemu process. There is no
#  efivarfs in this picture at all, which is why the install phase is wrapped
#  in tests/lab/nvram-guard and this phase is not — same reasoning as
#  installer/test-installer-luks-boot.sh's header.
# ─────────────────────────────────────────────────────────────────────────────
import argparse
import os
import subprocess
import sys
import time

# The markers, in the order they should appear. Each is a string the GUEST
# emits; none is produced by this script or by the suite that calls it.
SWITCH_MARKER = "Switching root"
REACHED_MARKERS = [
    ("login-prompt", "login:"),
    ("graphical-target", "Reached target Graphical Interface"),
    ("multiuser-target", "Reached target Multi-User System"),
    # systemd >= 250 renders unit names rather than descriptions in some
    # configurations; both spellings are the same event.
    ("multiuser-target", "Reached target multi-user.target"),
    ("graphical-target", "Reached target graphical.target"),
]
# Failures worth stopping on rather than waiting out the whole timeout.
FAIL_MARKERS = [
    ("emergency-shell", "Entering emergency mode"),
    ("dracut-timeout", "dracut-initqueue timeout"),
    ("no-root", "Cannot open root device"),
]


def read(path):
    try:
        with open(path, "rb") as f:
            return f.read().decode("utf-8", "replace")
    except OSError:
        return ""


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--work", required=True)
    ap.add_argument("--name", required=True)
    ap.add_argument("--code", required=True, help="raw OVMF_CODE pflash image")
    ap.add_argument("--vars-template", required=True,
                    help="raw OVMF_VARS pflash image; copied, never written")
    ap.add_argument("--disk", required=True,
                    help="the whole-disk image the installer wrote")
    ap.add_argument("--timeout", type=int, default=600)
    ap.add_argument("--mem", type=int, default=4096)
    ap.add_argument("--smp", type=int, default=4)
    ap.add_argument("--accel", default="kvm:tcg")
    a = ap.parse_args()

    serial = os.path.join(a.work, "serial-%s.log" % a.name)
    dbg = os.path.join(a.work, "%s.ovmf.log" % a.name)
    qerr = os.path.join(a.work, "qemu-%s.err" % a.name)
    myvars = os.path.join(a.work, "vars-%s.fd" % a.name)
    for p in (serial, dbg, qerr, myvars):
        if os.path.exists(p):
            os.unlink(p)
    open(serial, "w").close()
    open(dbg, "w").close()

    # A private, writable copy: qemu mutates the varstore in place and the
    # template has to stay pristine for a second boot of the same disk.
    with open(a.vars_template, "rb") as src, open(myvars, "wb") as dst:
        dst.write(src.read())

    cmd = ["qemu-system-x86_64",
           "-machine", "q35,smm=on,accel=%s" % a.accel, "-cpu", "max",
           "-m", str(a.mem), "-smp", str(a.smp),
           "-global", "driver=cfi.pflash01,property=secure,value=on",
           "-drive", "if=pflash,unit=0,format=raw,readonly=on,file=%s" % a.code,
           "-drive", "if=pflash,unit=1,format=raw,file=%s" % myvars,
           "-drive", "if=virtio,format=raw,file=%s,media=disk" % a.disk,
           "-debugcon", "file:%s" % dbg, "-global", "isa-debugcon.iobase=0x402",
           "-serial", "file:%s" % serial,
           "-display", "none", "-nodefaults"]

    err = open(qerr, "wb")
    proc = subprocess.Popen(cmd, stdout=err, stderr=err)
    deadline = time.time() + a.timeout
    switched_at = None
    reached_by = ""
    verdict = "no-verdict"
    try:
        # ── the pivot ────────────────────────────────────────────────────────
        while time.time() < deadline:
            txt = read(serial)
            if SWITCH_MARKER in txt:
                switched_at = time.time()
                break
            hit = [n for n, m in FAIL_MARKERS if m in txt]
            if hit:
                verdict = "failed-before-pivot:%s" % hit[0]
                raise SystemExit
            if proc.poll() is not None:
                verdict = "guest-exited-before-pivot"
                raise SystemExit
            time.sleep(0.5)
        else:
            verdict = "never-switched-root"
            raise SystemExit

        # ── somewhere a user could log in ───────────────────────────────────
        while time.time() < deadline:
            txt = read(serial)
            for name, marker in REACHED_MARKERS:
                if marker in txt:
                    reached_by = name
                    break
            if reached_by:
                verdict = "booted"
                break
            hit = [n for n, m in FAIL_MARKERS if m in txt]
            if hit:
                verdict = "pivoted-then-%s" % hit[0]
                break
            if proc.poll() is not None:
                verdict = "guest-exited-after-pivot"
                break
            time.sleep(0.5)
        else:
            verdict = "pivoted-but-never-reached-a-login"
    except SystemExit:
        pass
    finally:
        # A guest that is about to print the marker gets the same grace either
        # way; qemu is killed rather than asked, because there is no QMP socket
        # here and a disposable guest has nothing to flush.
        if proc.poll() is None:
            proc.kill()
            proc.wait()
        err.close()

    txt = read(serial)
    print("verdict=%s" % verdict)
    print("qemu-rc=%s" % proc.returncode)
    print("switched-root=%s" % ("yes" if SWITCH_MARKER in txt else "no"))
    print("reached-by=%s" % (reached_by or "none"))
    for name, marker in REACHED_MARKERS:
        print("marker[%s][%s]=%s" % (name, marker, "yes" if marker in txt else "no"))
    print("seconds-to-pivot=%s" % (
        "%.0f" % (switched_at - (deadline - a.timeout)) if switched_at else "n/a"))
    print("serial-bytes=%d" % len(txt))
    return 0


if __name__ == "__main__":
    sys.exit(main())
