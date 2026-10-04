#!/usr/bin/env python3
"""Smoke-test the installed addon against the *running* fcitx5 over D-Bus.

Creates a private input context (so the desktop's own contexts are untouched),
presses the trigger key, optionally plays a wav into a PipeWire sink whose
monitor is the current default source, releases, and prints what fcitx5
committed.

A tap (shorter than TapThresholdMs) locks the recording on, so this script
always holds the trigger for --hold seconds and does not exercise tap-to-lock.

    python3 scripts/fcitx5-smoke.py --hold 1                              # silence -> no commit
    python3 scripts/fcitx5-smoke.py --wav en.wav --sink voiceja_test --hold 5
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
KEYVAL_MENU, KEYCODE_MENU = 0xFF67, 135  # X11 keycode = evdev KEY_COMPOSE(127) + 8


def main() -> int:
    parser = argparse.ArgumentParser()
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
        GLib.Variant("(a(ss))", [[("program", "fcitx5-voice-ja-smoke")]]),
    ).unpack()
    print(f"input context: {path}", file=sys.stderr)

    commits: list[str] = []
    loop = GLib.MainLoop()

    def on_signal(_bus, _sender, _path, _iface, signal, params):
        if signal == "CommitString":
            (text,) = params.unpack()
            print(f"CommitString: {text!r}", file=sys.stderr)
            commits.append(text)
            loop.quit()

    bus.signal_subscribe(FCITX, IC_IFACE, None, path, None, Gio.DBusSignalFlags.NONE, on_signal)

    def key(release: bool) -> bool:
        return call(
            path, IC_IFACE, "ProcessKeyEvent",
            GLib.Variant("(uuubu)", [KEYVAL_MENU, KEYCODE_MENU, 0, release, 0]),
        ).unpack()[0]

    player = None
    error = None

    def after(milliseconds, callback):
        def run():
            nonlocal error
            try:
                callback()
            except Exception as exc:
                error = exc
                loop.quit()
            return False

        GLib.timeout_add(milliseconds, run)

    def play():
        nonlocal player
        player = subprocess.Popen(["pw-play", "--target", args.sink, args.wav])

    def press():
        if not key(False):
            raise RuntimeError("trigger press was not accepted by the module")
        if args.wav and args.sink:
            after(500, play)
        after(int(args.hold * 1000), release)

    def release():
        if not key(True):
            raise RuntimeError("trigger release was not accepted by the module")
        after(int(args.timeout * 1000), loop.quit)

    try:
        call(path, IC_IFACE, "FocusIn")
        after(300, press)
        loop.run()
        if error is not None:
            raise error
    finally:
        if player is not None:
            if player.poll() is None:
                player.terminate()
            player.wait()
        try:
            call(path, IC_IFACE, "FocusOut")
        finally:
            call(path, IC_IFACE, "DestroyIC")
    if args.wav:
        if not commits or not commits[0].strip():
            print("no transcript committed", file=sys.stderr)
            return 1
        print(commits[0])
    elif commits:
        print(f"unexpected commit on silence: {commits[0]!r}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
