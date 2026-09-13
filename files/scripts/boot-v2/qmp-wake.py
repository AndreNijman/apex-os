#!/usr/bin/env python3
# ─────────────────────────────────────────────────────────────────────────────
#  qmp-wake.py — watch a guest over QMP, and wake it when it suspends to RAM.
#
#  L-001's suspend/resume criterion needs two things that cannot come from the
#  same place:
#
#    1. something has to WAKE the guest. A guest that enters S3 with nobody
#       able to resume it stays there until vm_boot's timeout kills it, which
#       in the serial log is indistinguishable from a kernel that hung on the
#       way down. On real hardware the lid switch or the power button does
#       this; under qemu it is the `system_wakeup` QMP command.
#    2. something OTHER THAN THE GUEST has to say the guest suspended. The
#       guest reports its own /sys/power/suspend_stats counter, and that is a
#       good observation — but it is the only party in the room if nobody else
#       is watching, and "the test asserted a string the thing under test
#       printed" is how this repository has been fooled before. qemu's own
#       `query-status` returning `suspended` is an independent witness with no
#       access to the guest's narrative.
#
#  So this records what it saw, with timestamps, into a JSON file the scenario
#  reads. It never asserts anything itself: a scenario that only consulted the
#  waker would be trusting the waker, and a scenario that only consulted the
#  guest would be trusting the guest. Both are required to agree.
#
#  WHY IT NEVER FAILS THE RUN. Exiting non-zero here would abort a vm_boot that
#  might otherwise have produced a perfectly readable "the kernel refused to
#  suspend" result — and COULD-NOT-RUN is its own answer in this program, never
#  a failure. Every outcome is written to the record and the exit status is
#  always 0; the scenario decides what the record means.
# ─────────────────────────────────────────────────────────────────────────────
import argparse
import json
import os
import socket
import sys
import time


def connect(path, deadline):
    """Wait for qemu to create the QMP socket, then connect and handshake.

    qemu creates the socket a moment after exec, so a connect that runs first
    gets ENOENT/ECONNREFUSED. Polling beats sleeping: on a fast box a fixed
    sleep is wasted time and on a loaded one it is a flake.
    """
    while time.monotonic() < deadline:
        try:
            s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            s.settimeout(5.0)
            s.connect(path)
            return s
        except OSError:
            time.sleep(0.05)
    return None


class Qmp:
    def __init__(self, sock):
        self.sock = sock
        self.buf = b""

    def readline(self):
        while b"\n" not in self.buf:
            chunk = self.sock.recv(65536)
            if not chunk:
                raise ConnectionError("QMP closed the connection")
            self.buf += chunk
        line, self.buf = self.buf.split(b"\n", 1)
        return json.loads(line.decode("utf-8", "replace"))

    def command(self, name):
        """Send a command and return its return value.

        QMP interleaves asynchronous events with command replies on the same
        stream, so a reader that took the next line as the answer would read
        SHUTDOWN or SUSPEND as a command result. Events are skipped here, and
        that is not defensive coding: SUSPEND is the very event this tool
        provokes, so it is guaranteed to arrive mid-conversation.
        """
        self.sock.sendall((json.dumps({"execute": name}) + "\n").encode())
        while True:
            msg = self.readline()
            if "return" in msg:
                return msg["return"]
            if "error" in msg:
                raise RuntimeError(msg["error"].get("desc", str(msg["error"])))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--socket", required=True)
    ap.add_argument("--record", required=True)
    ap.add_argument("--timeout", type=float, default=240.0)
    args = ap.parse_args()

    rec = {
        "observer": "qmp",
        "suspended_seen": False,
        "wakeup_sent": False,
        "statuses": [],
        "reason": "",
    }

    def save():
        tmp = args.record + ".tmp"
        with open(tmp, "w") as fh:
            json.dump(rec, fh, indent=2, sort_keys=True)
            fh.write("\n")
        os.replace(tmp, args.record)

    started = time.monotonic()
    deadline = started + args.timeout
    save()

    sock = connect(args.socket, deadline)
    if sock is None:
        rec["reason"] = "the QMP socket never appeared"
        save()
        return 0

    try:
        q = Qmp(sock)
        q.readline()            # the QMP greeting
        q.command("qmp_capabilities")

        # Poll rather than wait on the SUSPEND event. An event-only watcher
        # misses a guest that suspended during the capabilities handshake, and
        # `query-status` is authoritative either way — it is qemu's own view of
        # the vCPU state, not a notification that may already have been sent.
        last = None
        while time.monotonic() < deadline:
            status = q.command("query-status").get("status", "?")
            if status != last:
                rec["statuses"].append(
                    {"at": round(time.monotonic() - started, 3), "status": status}
                )
                save()
                last = status
            if status == "suspended":
                rec["suspended_seen"] = True
                save()
                # A real machine takes a moment to settle into S3 before the
                # wake source is armed. Nothing depends on this number; it is
                # here so a wakeup cannot race the suspend it is answering.
                time.sleep(0.5)
                q.command("system_wakeup")
                rec["wakeup_sent"] = True
                rec["woke_at"] = round(time.monotonic() - started, 3)
                rec["reason"] = "guest suspended and was woken over QMP"
                save()
                # Record that it actually came back, so "we sent a wakeup" and
                # "it resumed" stay separate claims.
                while time.monotonic() < deadline:
                    status = q.command("query-status").get("status", "?")
                    if status != last:
                        rec["statuses"].append(
                            {"at": round(time.monotonic() - started, 3),
                             "status": status}
                        )
                        last = status
                        save()
                    if status == "running":
                        rec["resumed_seen"] = True
                        save()
                        return 0
                    time.sleep(0.1)
                rec["reason"] = "wakeup was sent but the guest never ran again"
                save()
                return 0
            time.sleep(0.1)

        rec["reason"] = "the guest never entered a suspended state"
        save()
    except (OSError, ConnectionError, RuntimeError, ValueError) as exc:
        # A closed connection is the NORMAL end of a guest that powered off
        # without ever suspending. It is recorded, not raised: this process
        # must never be the reason a boot is reported as failed.
        rec["reason"] = rec["reason"] or f"{type(exc).__name__}: {exc}"
        save()
    finally:
        try:
            sock.close()
        except OSError:
            pass
    return 0


if __name__ == "__main__":
    sys.exit(main())
