#!/usr/bin/env python3
# ─────────────────────────────────────────────────────────────────────────────
#  tests/atspi-walk.py — read an application's accessibility tree the way a
#  screen reader reads it, over AT-SPI on D-Bus.
#
#  This is the other half of tests/lib/atspi.sh. That file stands up a private
#  a11y bus; this one walks what an application publishes onto it.
#
#  ── Why this is not the same as reading Accessible.name in QML ──────────────
#
#  A QML assertion reads the attached object on a QQuickItem. AT-SPI reads what
#  the toolkit bridge decided to EXPORT, and the two differ in ways that matter
#  and are invisible from the QML side:
#
#    * The bridge suppresses some properties. Qt returns an empty name for any
#      item with Accessible.passwordEdit set (qquickaccessibleattached_p.h) --
#      so a password field that "has a name" in QML has none on the bus.
#    * The bridge applies its own role mapping, and roles that QML spells one
#      way arrive with AT-SPI's spelling and numbering.
#    * Items the bridge considers uninteresting never appear at all, so a
#      control can be perfectly labelled in QML and absent from the tree.
#    * Actions are a separate interface. "A screen reader can press this" is
#      org.a11y.atspi.Action, not a signal handler in QML.
#
#  ── Talking to the bus directly, and why ────────────────────────────────────
#
#  The obvious client is pyatspi, which is not installed here and is not in the
#  image. Rather than report the row unmeasurable over a Python binding, this
#  speaks the D-Bus interfaces itself through Gio -- which IS present, because
#  the installer's own test suite already depends on python3-gi. The protocol is
#  small: GetChildren, GetRole, GetState, GetInterfaces and the Action
#  interface are the whole of what is used here.
#
#  ── Safety ──────────────────────────────────────────────────────────────────
#
#  It refuses to run against anything but a private bus. AT_SPI_BUS_ADDRESS must
#  be set and must be a unix path; the caller (atspi.sh) has already checked
#  that the path is inside a directory it created. Without the variable this
#  exits non-zero rather than falling back to the session default, because the
#  session default on a developer's machine is a live desktop.
#
#  Usage:
#      atspi-walk.py --count                 how many apps are registered
#      atspi-walk.py --dump [--app NAME]     one line per node
#      atspi-walk.py --json [--app NAME]     the tree as JSON
#      atspi-walk.py --do-action NAME[:IDX] --app APP
#                                            press something, as a reader would
# ─────────────────────────────────────────────────────────────────────────────
import json
import os
import sys

import gi
gi.require_version("Gio", "2.0")
from gi.repository import Gio, GLib  # noqa: E402

ATSPI = "org.a11y.atspi"
ACCESSIBLE = ATSPI + ".Accessible"
ACTION = ATSPI + ".Action"
COMPONENT = ATSPI + ".Component"
PROPS = "org.freedesktop.DBus.Properties"
ROOT_PATH = "/org/a11y/atspi/accessible/root"

# Roles are NOT decoded from a table here. An earlier draft carried a hand-built
# map of AT-SPI role numbers and it was wrong in a way that would have poisoned
# every assertion built on it: a Qt EditableText arrived as 47 and the table
# called it "table-column", a push button came out as "panel". The enum has been
# renumbered across at-spi2 releases and a test that hardcodes one release's
# numbering is asserting against the wrong vocabulary on every other.
#
# So the role name is asked of the bus -- GetRoleName is part of the Accessible
# interface precisely so clients do not have to keep such a table. The numeric
# value is kept alongside it, because that is what a mutation can move.

# AT-SPI state bit numbers -> names. The bitset arrives as two uint32s.
STATES = {
    1: "active", 2: "armed", 3: "busy", 4: "checked", 5: "collapsed",
    6: "defunct", 7: "editable", 8: "enabled", 9: "expandable",
    10: "expanded", 11: "focusable", 12: "focused", 13: "has-tooltip",
    14: "horizontal", 15: "iconified", 16: "modal", 17: "multi-line",
    18: "multiselectable", 19: "opaque", 20: "pressed", 21: "resizable",
    22: "selectable", 23: "selected", 24: "sensitive", 25: "showing",
    26: "single-line", 27: "stale", 28: "transient", 29: "vertical",
    30: "visible", 31: "manages-descendants", 32: "indeterminate",
    33: "required", 34: "truncated", 35: "animated", 36: "invalid-entry",
    37: "supports-autocompletion", 38: "selectable-text", 39: "is-default",
    40: "visited", 41: "checkable", 42: "has-popup", 43: "read-only",
}


def die(msg, code=2):
    print("atspi-walk: " + msg, file=sys.stderr)
    sys.exit(code)


def connect():
    addr = os.environ.get("AT_SPI_BUS_ADDRESS", "").strip()
    if not addr:
        die("AT_SPI_BUS_ADDRESS is not set. This tool will not fall back to the "
            "session default, which on a workstation is a live desktop's bus.")
    if not addr.startswith("unix:"):
        die("AT_SPI_BUS_ADDRESS is not a unix socket address: %r" % addr)
    try:
        return Gio.DBusConnection.new_for_address_sync(
            addr,
            Gio.DBusConnectionFlags.AUTHENTICATION_CLIENT
            | Gio.DBusConnectionFlags.MESSAGE_BUS_CONNECTION,
            None, None)
    except GLib.Error as e:
        die("could not connect to the private a11y bus: %s" % e.message)


class Tree:
    def __init__(self, conn):
        self.conn = conn

    def call(self, dest, path, iface, method, body=None, reply=None):
        try:
            r = self.conn.call_sync(dest, path, iface, method, body, reply,
                                    Gio.DBusCallFlags.NO_AUTO_START, 3000, None)
        except GLib.Error:
            return None
        return r.unpack() if r is not None else None

    def prop(self, dest, path, name):
        r = self.call(dest, path, PROPS, "Get",
                      GLib.Variant("(ss)", (ACCESSIBLE, name)),
                      GLib.VariantType("(v)"))
        return r[0] if r else None

    def children(self, dest, path):
        r = self.call(dest, path, ACCESSIBLE, "GetChildren", None,
                      GLib.VariantType("(a(so))"))
        return list(r[0]) if r else []

    def role(self, dest, path):
        """The role NAME, straight from the application, plus its number."""
        r = self.call(dest, path, ACCESSIBLE, "GetRoleName", None,
                      GLib.VariantType("(s)"))
        name = r[0] if r else ""
        n = self.call(dest, path, ACCESSIBLE, "GetRole", None,
                      GLib.VariantType("(u)"))
        num = n[0] if n else -1
        return name if name else "role-%d" % num

    def role_number(self, dest, path):
        n = self.call(dest, path, ACCESSIBLE, "GetRole", None,
                      GLib.VariantType("(u)"))
        return n[0] if n else -1

    def states(self, dest, path):
        r = self.call(dest, path, ACCESSIBLE, "GetState", None,
                      GLib.VariantType("(au)"))
        if not r:
            return []
        words = list(r[0])
        out = []
        for w_i, word in enumerate(words):
            for bit in range(32):
                if word & (1 << bit):
                    out.append(STATES.get(w_i * 32 + bit, "state-%d" % (w_i * 32 + bit)))
        return sorted(out)

    def interfaces(self, dest, path):
        r = self.call(dest, path, ACCESSIBLE, "GetInterfaces", None,
                      GLib.VariantType("(as)"))
        return sorted(r[0]) if r else []

    def actions(self, dest, path):
        """The actions a screen reader could invoke on this node.

        NActions is a PROPERTY on org.a11y.atspi.Action, not a method. An
        earlier draft called GetNActions() and got nothing back, so every node
        reported an empty action list while DoAction(0) on the very same node
        worked -- a test written against that output would have asserted that a
        button exposes no way to press it and passed.
        """
        n = self.call(dest, path, PROPS, "Get",
                      GLib.Variant("(ss)", (ACTION, "NActions")),
                      GLib.VariantType("(v)"))
        count = n[0] if n else 0
        if not count:
            return []
        out = []
        for i in range(count):
            r = self.call(dest, path, ACTION, "GetName",
                          GLib.Variant("(i)", (i,)), GLib.VariantType("(s)"))
            out.append(r[0] if r else "action-%d" % i)
        return out

    def node(self, dest, path, depth, seen):
        key = (dest, path)
        if key in seen or depth > 24:
            return None
        seen.add(key)
        n = {
            "bus": dest,
            "path": path,
            "name": self.prop(dest, path, "Name") or "",
            "description": self.prop(dest, path, "Description") or "",
            "role": self.role(dest, path),
            "role_number": self.role_number(dest, path),
            "states": self.states(dest, path),
            "interfaces": self.interfaces(dest, path),
            "actions": self.actions(dest, path),
            "depth": depth,
            "children": [],
        }
        for cdest, cpath in self.children(dest, path):
            if cpath in ("/org/a11y/atspi/null", ""):
                continue
            c = self.node(cdest or dest, cpath, depth + 1, seen)
            if c:
                n["children"].append(c)
        return n

    def apps(self):
        return [(d or ATSPI + ".Registry", p)
                for d, p in self.children(ATSPI + ".Registry", ROOT_PATH)]


def flatten(n, out=None):
    out = [] if out is None else out
    out.append(n)
    for c in n["children"]:
        flatten(c, out)
    return out


def main():
    args = sys.argv[1:]
    conn = connect()
    t = Tree(conn)

    want_app = None
    if "--app" in args:
        want_app = args[args.index("--app") + 1]

    apps = t.apps()
    if "--count" in args:
        print(len(apps))
        return 0

    roots = []
    for dest, path in apps:
        node = t.node(dest, path, 0, set())
        if node is None:
            continue
        if want_app and want_app not in (node["name"] or ""):
            continue
        roots.append(node)

    if "--do-action" in args:
        # The action index is its own flag. An earlier draft spelled this
        # "NAME:INDEX" and split on the colon, which crashed on the first real
        # control it met -- the greeter's layout pill is named
        # "Keyboard layout: us (press Space to change)". Accessible names are
        # prose; they must not be parsed.
        target = args[args.index("--do-action") + 1]
        idx = int(args[args.index("--index") + 1]) if "--index" in args else 0
        for r in roots:
            for n in flatten(r):
                if n["name"] == target and ACTION in n["interfaces"]:
                    ok = t.call(n["bus"], n["path"], ACTION, "DoAction",
                                GLib.Variant("(i)", (idx,)),
                                GLib.VariantType("(b)"))
                    print("DoAction %s[%d] -> %s" % (target, idx, ok[0] if ok else "no reply"))
                    return 0 if (ok and ok[0]) else 1
        print("no node named %r exposing the Action interface" % target, file=sys.stderr)
        return 1

    if "--get-text" in args:
        # What org.a11y.atspi.Text hands to anyone on the bus. For a password
        # field this is the question that matters: a bridge that returned the
        # real characters would be publishing the password to every process
        # connected to the accessibility bus.
        target = args[args.index("--get-text") + 1]
        TEXT = ATSPI + ".Text"
        # FOCUSABLE nodes only. A caption sitting beside a field is a Gtk.Label,
        # it carries the Text interface too, and its accessible name is the very
        # string the field was named after -- so a plain name match returned the
        # caption's text ("Username") instead of what the user had typed, and an
        # assertion built on it compared a label against itself. A caption is
        # never focusable; the field always is.
        candidates = []
        for r in roots:
            for n in flatten(r):
                if target in (n["name"], n["description"]) and "focusable" in n["states"]:
                    candidates.append(n)
        for n in candidates:
            if TEXT not in n["interfaces"]:
                print("NO-TEXT-INTERFACE")
                return 0
            got = t.call(n["bus"], n["path"], TEXT, "GetText",
                         GLib.Variant("(ii)", (0, -1)),
                         GLib.VariantType("(s)"))
            print(got[0] if got else "")
            return 0
        print("no FOCUSABLE node named or described %r" % target, file=sys.stderr)
        return 1

    if "--json" in args:
        print(json.dumps(roots, indent=1))
        return 0

    # --dump (the default): one line per node, stable and greppable.
    for r in roots:
        for n in flatten(r):
            print("%s%s | role=%s | name=%s | desc=%s | states=%s | actions=%s" % (
                "  " * n["depth"], n["path"].rsplit("/", 1)[-1],
                n["role"], n["name"], n["description"],
                ",".join(n["states"]), ",".join(n["actions"])))
    return 0


if __name__ == "__main__":
    sys.exit(main())
