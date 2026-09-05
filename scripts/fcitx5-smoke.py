#!/usr/bin/env python3
"""Smoke-test the installed addon against the *running* fcitx5 over D-Bus.

Creates a private input context (so the desktop's own contexts are untouched),
switches it to parakeet-<lang>, holds the trigger key, optionally plays a wav
into a PipeWire sink whose monitor is the current default source, releases,
and prints what fcitx5 committed.

    python3 scripts/fcitx5-smoke.py --lang en --hold 1            # silence -> no commit
    python3 scripts/fcitx5-smoke.py --lang en --wav en.wav --sink parakeet_test --hold 5
"""

from __future__ import annotations

import argparse
import subprocess
import sys

import gi

gi.require_version("Gio", "2.0")
from gi.repository import Gio, GLib  # noqa: E402

FCITX = "org.fcitx.Fcitx5"
IC_IFACE = "org.fcitx.Fcitx.InputContext1"
KEYVAL_SPACE, KEYCODE_SPACE = 0x20, 65  # X11 keycode = evdev(57) + 8


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--lang", default="en")
    parser.add_argument("--wav")
    parser.add_argument("--sink", help="pw-play target; its monitor must be the default source")
    parser.add_argument("--hold", type=float, default=5.0, help="seconds to hold the trigger")
    parser.add_argument("--timeout", type=float, default=15.0, help="seconds to wait for the commit")
    args = parser.parse_args()

    bus = Gio.bus_get_sync(Gio.BusType.SESSION)

    def call(path: str, iface: str, method: str, params: GLib.Variant | None = None):
        return bus.call_sync(FCITX, path, iface, method, params, None, Gio.DBusCallFlags.NONE, -1, None)

    path, _uuid = call(
        "/org/freedesktop/portal/inputmethod",
        "org.fcitx.Fcitx.InputMethod1",
        "CreateInputContext",
        GLib.Variant("(a(ss))", [[("program", "fcitx5-parakeet-smoke")]]),
    ).unpack()
    print(f"input context: {path}", file=sys.stderr)

    commits: list[str] = []
    loop = GLib.MainLoop()

    def on_signal(_bus, _sender, _path, _iface, signal, params):
        if signal == "CommitString":
            (text,) = params.unpack()
            print(f"CommitString: {text!r}", file=sys.stderr)
            commits.append(text)
            if args.wav and len(commits) >= 2:
                loop.quit()

    bus.signal_subscribe(FCITX, IC_IFACE, None, path, None, Gio.DBusSignalFlags.NONE, on_signal)

    call(path, IC_IFACE, "FocusIn")
    call("/controller", "org.fcitx.Fcitx.Controller1", "SetCurrentIM", GLib.Variant("(s)", [f"parakeet-{args.lang}"]))
    current = call("/controller", "org.fcitx.Fcitx.Controller1", "CurrentInputMethod").unpack()[0]
    print(f"current input method: {current}", file=sys.stderr)
    if current != f"parakeet-{args.lang}":
        print("failed to switch input method", file=sys.stderr)
        return 1

    def key(release: bool) -> bool:
        return call(
            path, IC_IFACE, "ProcessKeyEvent",
            GLib.Variant("(uuubu)", [KEYVAL_SPACE, KEYCODE_SPACE, 0, release, 0]),
        ).unpack()[0]

    key(False)
    key(True)

    player = None

    def start_hold():
        nonlocal player
        assert key(False), "press was not accepted by the engine"
        if args.wav and args.sink:
            GLib.timeout_add(500, lambda: (subprocess.Popen(["pw-play", "--target", args.sink, args.wav]) and False))
        GLib.timeout_add(int(args.hold * 1000), release)
        return False

    def release():
        assert key(True), "release was not accepted by the engine"
        GLib.timeout_add(int(args.timeout * 1000), lambda: loop.quit() or False)
        return False

    GLib.timeout_add(300, start_hold)
    loop.run()

    call(path, IC_IFACE, "FocusOut")
    call(path, IC_IFACE, "DestroyIC")

    if not commits or commits[0] != " ":
        print("tap did not type a space", file=sys.stderr)
        return 1
    if args.wav:
        if len(commits) < 2 or not commits[1].strip():
            print("no transcript committed", file=sys.stderr)
            return 1
        print(commits[1])
    elif len(commits) > 1:
        print(f"unexpected commit on silence: {commits[1]!r}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
