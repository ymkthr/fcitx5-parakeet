#!/usr/bin/env python3
"""Add parakeet-ja / parakeet-en / parakeet-auto to the current fcitx5 input method group.

Talks to the running fcitx5 over D-Bus so the change is applied live and
persisted to ~/.config/fcitx5/profile by fcitx5 itself (editing the file while
fcitx5 runs would be overwritten on exit).
"""

from __future__ import annotations

import sys

import gi

gi.require_version("Gio", "2.0")
from gi.repository import Gio, GLib  # noqa: E402

FCITX = "org.fcitx.Fcitx5"
CONTROLLER = "org.fcitx.Fcitx.Controller1"
WANTED = ("parakeet-ja", "parakeet-en", "parakeet-auto")


def main() -> int:
    bus = Gio.bus_get_sync(Gio.BusType.SESSION)

    def call(method: str, params: GLib.Variant | None = None):
        return bus.call_sync(FCITX, "/controller", CONTROLLER, method, params, None, Gio.DBusCallFlags.NONE, -1, None)

    available = {entry[0] for entry in call("AvailableInputMethods").unpack()[0]}
    missing = [im for im in WANTED if im not in available]
    if missing:
        print(f"fcitx5 does not know {missing}; is fcitx5-parakeet installed and fcitx5 restarted?", file=sys.stderr)
        return 1

    group = call("CurrentInputMethodGroup").unpack()[0]
    layout, items = call("InputMethodGroupInfo", GLib.Variant("(s)", [group])).unpack()
    names = [name for name, _ in items]
    added = [im for im in WANTED if im not in names]
    if not added:
        print(f"group {group!r} already contains {', '.join(WANTED)}")
        return 0
    items = list(items) + [(im, "") for im in added]
    call("SetInputMethodGroupInfo", GLib.Variant("(ssa(ss))", [group, layout, items]))
    print(f"added {', '.join(added)} to group {group!r}: {[name for name, _ in items]}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
