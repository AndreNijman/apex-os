#!/usr/bin/env python3
"""keymap-boot-drive.py — boot the SHIPPED APEX initramfs under qemu, type a
passphrase on the EMULATED KEYBOARD through QMP, and report what the guest did
with it.

WHY THE KEYSTROKES GO THROUGH QMP. `send-key` takes qcodes, which are key
POSITIONS named after their US layout. The host says "the key where a US
keyboard has y"; the guest's loaded console keymap decides whether that becomes
`y` or `z`. That is the whole property under test, and no file-content
assertion can stand in for it: a machine whose /etc/vconsole.conf says `de` and
whose VT never loaded the table types `y` at the passphrase prompt and locks
its owner out.

It is a repository file rather than a heredoc inside the suite so `python3 -m
py_compile` can see it.
"""
import argparse, json, os, re, socket, subprocess, sys, time

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

# The physical keys, named by their US-layout position, which is what QMP
# qcodes are. This is the whole point: the HOST sends key POSITIONS and the
# GUEST's loaded keymap decides which characters they become.
KEYS = {"a": "a", "p": "p", "e": "e", "x": "x", "y": "y", "d": "d", "1": "1",
        "z": "z", "q": "q", "m": "m", "0": "0"}

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--work", required=True)
    ap.add_argument("--name", required=True)
    ap.add_argument("--kernel", required=True)
    ap.add_argument("--initrd", required=True)
    ap.add_argument("--disk", required=True)
    ap.add_argument("--append", required=True)
    ap.add_argument("--keys", required=True, help="comma-separated qcodes to type")
    ap.add_argument("--smbios", default="")
    ap.add_argument("--timeout", type=int, default=180)
    ap.add_argument("--accel", default="kvm")
    a = ap.parse_args()

    serial = os.path.join(a.work, "serial-%s.log" % a.name)
    qmp    = os.path.join(a.work, "qmp-%s.sock" % a.name)
    qerr   = os.path.join(a.work, "qemu-%s.err" % a.name)
    for p in (serial, qmp, qerr):
        if os.path.exists(p):
            os.unlink(p)
    open(serial, "w").close()

    cmd = ["qemu-system-x86_64",
           "-machine", "q35,accel=%s" % a.accel, "-cpu", "max",
           "-m", "2048", "-smp", "2",
           "-kernel", a.kernel, "-initrd", a.initrd, "-append", a.append,
           "-drive", "if=virtio,format=raw,file=%s,media=disk" % a.disk,
           "-device", "VGA",
           "-serial", "file:%s" % serial,
           "-qmp", "unix:%s,server=on,wait=off" % qmp,
           "-display", "none", "-nodefaults", "-no-reboot"]
    if a.smbios:
        cmd += ["-smbios", a.smbios]

    err = open(qerr, "wb")
    proc = subprocess.Popen(cmd, stdout=err, stderr=err)
    deadline = time.time() + a.timeout
    typed_at = None
    verdict = "no-verdict"
    try:
        s = qmp_connect(qmp, min(deadline, time.time() + 30))
        if s is None:
            verdict = "qmp-never-answered"
            raise SystemExit
        q = Qmp(s)
        # ── wait for the guest to say the keymap is loaded ──────────────────
        # Typing before systemd-vconsole-setup has run would translate the
        # keys with the kernel's built-in `us` table whatever the channel
        # said, and a run that did that would pass or fail for the wrong
        # reason. The probe unit is ordered After=systemd-vconsole-setup.
        while time.time() < deadline:
            txt = open(serial, errors="replace").read()
            if "APEX-KEYMAP-PROBE: READY" in txt:
                break
            if proc.poll() is not None:
                verdict = "guest-died-before-ready"
                raise SystemExit
            time.sleep(0.25)
        else:
            verdict = "never-became-ready"
            raise SystemExit

        # Give systemd-cryptsetup a moment to open the prompt. Characters
        # typed a little early are not lost: they sit in the tty input buffer
        # and ask-password uses TCSADRAIN, which does not flush input.
        time.sleep(4)
        for k in a.keys.split(","):
            q.send_key(KEYS.get(k, k))
            time.sleep(0.12)
        q.send_key("ret")
        typed_at = time.time()

        while time.time() < deadline:
            txt = open(serial, errors="replace").read()
            if "APEX-KEYMAP-RESULT: DONE" in txt:
                verdict = "result-reported"
                break
            if proc.poll() is not None:
                verdict = "guest-exited-without-result"
                break
            time.sleep(0.25)
        else:
            verdict = "timeout-after-typing"
    except SystemExit:
        pass
    finally:
        for _ in range(60):
            if proc.poll() is not None:
                break
            time.sleep(0.5)
        if proc.poll() is None:
            proc.kill()
            proc.wait()
        err.close()

    txt = open(serial, errors="replace").read()
    m = re.search(r"APEX-KEYMAP-RESULT: (unlocked=\S+.*)", txt)
    print("verdict=%s" % verdict)
    print("qemu-rc=%s" % proc.returncode)
    print("typed=%s" % ("yes" if typed_at else "no"))
    print("result=%s" % (m.group(1).strip() if m else "<none>"))
    return 0

if __name__ == "__main__":
    sys.exit(main())
