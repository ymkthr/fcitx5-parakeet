# fcitx5-parakeet

[日本語](README.md) | English

fcitx5-parakeet provides local voice input for fcitx5 using NVIDIA Parakeet.
It runs as a module that stays loaded in fcitx5 and starts dictation the moment you press a trigger key.
It never switches the input method itself.

## Installation

fcitx5-parakeet supports Arch Linux and can be packaged for Debian/Ubuntu (`.deb`) and Fedora (`.rpm`). It expects fcitx5 and PipeWire to be running.

### From the AUR

```sh
paru -S fcitx5-parakeet   # or: yay -S fcitx5-parakeet
```

The package does not include the models. After installing, run the following as your desktop user:

```sh
parakeetd-download-models
systemctl --user enable --now parakeetd.socket
busctl --user call org.fcitx.Fcitx5 /controller org.fcitx.Fcitx.Controller1 Restart
parakeet-capslock-menu apply   # GNOME: make CapsLock act as the Menu key
```

### From a deb or rpm

On Debian/Ubuntu and Fedora, build the package on a machine with docker or podman and install it.

```sh
packaging/build.sh deb    # writes a .deb to packaging/dist/ (default image: debian:trixie)
packaging/build.sh rpm    # writes an .rpm to packaging/dist/ (default image: fedora:42)
sudo apt install ./packaging/dist/fcitx5-parakeet_*.deb
sudo dnf install ./packaging/dist/fcitx5-parakeet-*.rpm
```

The post-install steps are the same as for the AUR package. See `packaging/README.md` for details.

### From the repository

Run the installer from the repository root.

```sh
./scripts/install.sh
```

The installer performs the following steps:

1. Builds and installs the Rust daemon and fcitx5 addon.
2. Downloads the Japanese, English, and Silero VAD models, and the jinen-v2-small kana-kanji model.
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
parakeet-capslock-menu revert
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

While recording, `🎙️` appears near the cursor followed by a live transcript of what you have said so far (its last 40 characters).
The live transcript updates about every 0.5 seconds and is only a preview.
The inserted text comes from transcribing the whole recording again after it stops, with homophone correction.
`…` appears while the recording is being transcribed.

Press Escape while recording to cancel.
All other keys continue to work normally in the current input method, so you can edit right after dictating.

The trigger does not start dictation while the input method has uncommitted composition text.

The recognition language defaults to `auto`.
Both Japanese and English are recognized, and the daemon picks the language.
You can also pin it to `ja` or `en` in the configuration.

The first dictation after login can take a few seconds longer, because the daemon loads its models on first use.

## Japanese homophone correction

Japanese transcripts sometimes pick the wrong homophone, such as 機会 for 機械.
The daemon turns the transcript back into its reading and converts it again with the kana-kanji model [jinen-v2-small](https://huggingface.co/togatogah/jinen-v2-small.gguf).
The conversion uses up to 64 characters before the cursor as context.
The context is sent only when the application provides surrounding text, and it is never logged.

The transcript is replaced only when the model finds its own conversion clearly more likely than the transcript.
On 96 recorded utterances this fixed 6 errors and broke no correct transcript.
A correction takes about 50 ms on 4 CPU threads.

`parakeetd-download-models` (`scripts/download-models.sh` in the repository) downloads the model (about 80 MB) into `~/.local/share/parakeetd/models/jinen-v2-small/`.
Without the model file, correction is disabled and transcripts are typed as recognized.

To turn correction off, add the following to `~/.config/parakeetd/config.toml`.

```toml
[correction]
enabled = false
```

`margin` is the log-likelihood difference, in nats, that a conversion needs to replace the transcript (default 4.0).
Larger values replace less often, smaller values more often.

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
| ShowStatus | True | Shows `🎙️` with the live transcript, or `…`, near the cursor |

To use a different microphone, create `~/.config/parakeetd/config.toml` and specify its PipeWire source name.
See `daemon/config.example.toml` for the complete configuration example.

```toml
target = "alsa_input.example"
```

`partial_interval_ms` sets how often the live transcript updates, in milliseconds (default 500).
Set it to 0 to turn the live transcript off.

```toml
partial_interval_ms = 0
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

The NVIDIA Parakeet models downloaded at install time are distributed under [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/), the Silero VAD model under the MIT License, and jinen-v2-small under [CC BY-SA 4.0](https://creativecommons.org/licenses/by-sa/4.0/).
The models are not part of the package.

The daemon embeds [llama.cpp](https://github.com/ggml-org/llama.cpp) (MIT License) to run the kana-kanji model and the IPADIC dictionary (`licenses/ipadic.LICENSE`) to look up readings.

See `THIRD_PARTY_NOTICES.md` for the full list of bundled and downloaded components.
