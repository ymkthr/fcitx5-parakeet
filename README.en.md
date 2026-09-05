# fcitx5-parakeet

[日本語](README.md) | English

fcitx5-parakeet provides local voice input for fcitx5 using NVIDIA Parakeet.
It supports Japanese, English, and automatic Japanese/English detection.

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
4. Adds the Parakeet input methods to the current fcitx5 input method group.

The build requires `base-devel`, `cargo`, `clang`, `cmake`, `extra-cmake-modules`, and `gettext`.
`makepkg` prompts to install missing packages.
The speech recognition models are stored in `~/.local/share/parakeetd/models/`.

If fcitx5 was not running during installation, start it and register the input methods manually.

```sh
python3 scripts/fcitx5-register.py
```

Verify the connection to the daemon after installation.

```sh
parakeet-ctl hello
parakeet-ctl status
```

## Usage

1. Select “Parakeet Voice” with the fcitx5 input method switch key.
2. Press and hold Space.
3. Start speaking when `🎙️` appears near the cursor.
4. Release Space to transcribe the recording and insert the result at the cursor.

Pressing Space for less than 250 milliseconds inserts a normal space without recording.
Press Escape while recording to cancel.
All keys other than Space and Escape continue to work normally.

Choose an input method according to the recognition language.

| Input method | Recognition language |
| --- | --- |
| Parakeet Voice (Auto, Japanese/English) | Detects Japanese or English automatically |
| Parakeet Voice (Japanese) | Japanese |
| Parakeet Voice (English) | English |

Use “Auto, Japanese/English” for normal use.
Switch to the Japanese or English input method when you need to fix the recognition language.

## Configuration

Open the Parakeet input method settings in `fcitx5-configtool`.
The settings are stored in `~/.config/fcitx5/conf/parakeet.conf`.

| Setting | Default | Description |
| --- | --- | --- |
| Recording mode | Push to talk | Select Toggle to start and stop recording with separate Space presses |
| Trigger key | Space | Starts and stops recording |
| Cancel key | Escape | Cancels the current recording |
| TapThresholdMs | 250 | Maximum press duration treated as normal key input |
| SocketPath | Empty | Uses `$XDG_RUNTIME_DIR/parakeetd.sock` when empty |
| ShowStatus | True | Shows the recording state near the cursor |

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
