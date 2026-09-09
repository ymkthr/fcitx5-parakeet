# fcitx5-parakeet

[日本語](README.md) | English

fcitx5-parakeet provides local voice input for fcitx5 using NVIDIA Parakeet.
It runs as a module that stays loaded in fcitx5 and starts dictation the moment you press a trigger key.
It never switches the input method itself.

## Installation

fcitx5-parakeet supports Arch Linux.
Run the installer from the repository root on a system with fcitx5 and PipeWire running.

```sh
./scripts/install.sh
```

The installer performs the following steps:

1. Builds and installs the Rust daemon and fcitx5 addon.
2. Downloads the Japanese, English, and Silero VAD models.
3. Enables the systemd user socket.
4. Restarts fcitx5, if it is running, so the new module loads.
5. On GNOME, adds the XKB option `caps:menu` so CapsLock acts as the Menu key.

The build requires `base-devel`, `cargo`, `clang`, `cmake`, `extra-cmake-modules`, and `gettext`.
`makepkg` prompts to install missing packages.
The speech recognition models are stored in `~/.local/share/parakeetd/models/`.

If fcitx5 was not running during installation, start it manually.

Verify the connection to the daemon after installation.

```sh
parakeet-ctl hello
parakeet-ctl status
```

## Usage

The default trigger key is `Menu`, and CapsLock is used as the Menu key.
On Wayland fcitx5 cannot turn the caps-lock state back off, so the desktop remaps the key instead of fcitx5 intercepting it.

On GNOME, the installer applies this remap automatically.
It keeps existing XKB options and appends `caps:menu`; if an option starting with `caps:` is already present, it leaves the settings unchanged to avoid a conflict.
After the remap, CapsLock no longer locks capital letters.

To keep CapsLock as it is, pass `--keep-capslock` to the installer.
Then dictate with a physical Menu key, or change the trigger key in the configuration.

```sh
./scripts/install.sh --keep-capslock
```

To undo the remap, run the following command.
It removes only `caps:menu` and keeps the other options.

```sh
./scripts/capslock-menu.sh revert
```

On desktops other than GNOME, the installer changes nothing and prints instructions instead.
On X11 desktops, run the following command (effective until logout).

```sh
setxkbmap -option caps:menu
```

On KDE, open System Settings, go to Keyboard, then Key Bindings, then Caps Lock behavior, and choose "Make Caps Lock an additional Menu key".

Press CapsLock in a text field: a microphone mark near the cursor means the key reached fcitx5.
If no mark appears, check whether the keyboard firmware or a key remapper turns CapsLock into another key.

Tapping the trigger key (shorter than 250 milliseconds) starts recording and keeps it on.
Tap again to stop recording; the transcript is inserted at the cursor.

Holding the trigger key records while it is held down.
Releasing it stops recording; the transcript is inserted at the cursor.

`🎙️` appears near the cursor while recording.
`…` appears while the recording is being transcribed.

Press Escape while recording to cancel.
All other keys continue to work normally in the current input method, so you can edit right after dictating.

The trigger does not start dictation while the input method has uncommitted composition text.

The recognition language defaults to `auto`.
Both Japanese and English are recognized, and the daemon picks the language.
You can also pin it to `ja` or `en` in the configuration.

The first dictation after login can take a few seconds longer, because the daemon loads its models on first use.

## Configuration

Open `fcitx5-configtool`, go to Addons, and select "Parakeet Voice Input" to configure it.
The settings are stored in `~/.config/fcitx5/conf/parakeet.conf`.

| Setting | Default | Description |
| --- | --- | --- |
| TriggerKey | Menu | Starts and stops recording |
| CancelKey | Escape | Cancels the current recording |
| TapThresholdMs | 250 | Presses shorter than this lock the recording on; longer presses record only while held |
| Language | auto | auto, ja, or en |
| SocketPath | Empty | Uses `$XDG_RUNTIME_DIR/parakeetd.sock` when empty |
| ShowStatus | True | Shows `🎙️` or `…` near the cursor |

To use a different microphone, create `~/.config/parakeetd/config.toml` and specify its PipeWire source name.
See `daemon/config.example.toml` for the complete configuration example.

```toml
target = "alsa_input.example"
```

Restart the daemon after changing its configuration.

```sh
systemctl --user restart parakeetd.service
```

Use the following command to inspect daemon logs.

```sh
journalctl --user -u parakeetd.service -f
```

## License

fcitx5-parakeet itself is released under the MIT License (`LICENSE`).

The package bundles the shared libraries of [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx) (Apache License 2.0, `LICENSE-APACHE-2.0`) and [ONNX Runtime](https://github.com/microsoft/onnxruntime) (MIT License) as the speech recognition runtime.

The NVIDIA Parakeet models downloaded at install time are distributed under [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/), and the Silero VAD model under the MIT License.
The models are not part of the package.

See `THIRD_PARTY_NOTICES.md` for the full list of bundled and downloaded components.
