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
2. Downloads the Japanese, English, and Silero VAD models, and for homophone correction the jinen-v2-small kana-kanji model and the TinySwallow-1.5B language model.
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
The inserted text comes from transcribing the recording again as a whole after it stops, with homophone correction.
`…` appears while the recording is being transcribed.

Long dictation is inserted in parts while you keep speaking.
Once 60 seconds of recording are not yet inserted, the part up to the longest pause in their second half is transcribed the same way and inserted, and recording continues.
Recording ends by itself after 600 seconds in total, and the rest is inserted as if you had stopped.

Press Escape while recording to cancel the part not yet inserted.
All other keys continue to work normally in the current input method, so you can edit right after dictating.

If focus moves to another window while recording (a notification, a window switch), recording stops and is transcribed.
The text is inserted when the original text field gets focus back.

The trigger does not start dictation while the input method has uncommitted composition text; a message near the cursor asks you to commit it first.

The recognition language defaults to `auto`.
Both Japanese and English are recognized, and the daemon picks the language.
You can also pin it to `ja` or `en` in the configuration.

The first dictation after login can take a few seconds longer, because the daemon loads its models on first use.

## Japanese homophone correction

Japanese transcripts sometimes pick the wrong homophone, such as 機会 for 機械.
The daemon turns the transcript back into its reading and converts it again with the kana-kanji model [jinen-v2-small](https://huggingface.co/togatogah/jinen-v2-small.gguf).
Each place where the conversion differs from the transcript is a candidate fix, and the language model [TinySwallow-1.5B](https://huggingface.co/SakanaAI/TinySwallow-1.5B) accepts or rejects each candidate.
Both models use up to 64 characters before the cursor as context.
The context is sent only when the application provides surrounding text, and it is never logged.
Parts already inserted during a long dictation are context for the parts that follow.

A candidate is accepted only when the language model finds the fixed text clearly more likely than the transcript.
The transcript is corrected in pieces, cut at sentence ends or at a phrase boundary about every 40 characters, with the preceding text as context.
On 96 short synthesized utterances this removed errors from 12 of them (3 before the language model judged the fixes).
On 22 dictations of 10 seconds to 2 minutes it cut character errors from 128 to 111, and on 12 plain-form paragraphs (no です/ます endings) from 39 to 32, without making any utterance worse.
On 4 CPU threads a correction takes about 0.1 s for a short utterance, 1 s for 30 seconds of speech and 2.4 to 3.1 s for a minute.
A long recording is cut at pauses while it continues and the parts are corrected as they are cut, so after it stops only the last part is left to correct (for a 2.5-minute recording, 7.1 s from stop to text against 4.3 s without correction).

`parakeetd-download-models` (`scripts/download-models.sh` in the repository) downloads the models (about 80 MB for jinen-v2-small, 940 MB for TinySwallow-1.5B) into `~/.local/share/parakeetd/models/`.
Correction adds about 1.1 GB to the daemon's memory use.
Without either model file, correction is disabled and transcripts are typed as recognized.
Correction needs a CPU with AVX2, FMA, F16C and BMI2 (Intel Haswell, AMD Excavator or later); on other CPUs it is disabled automatically.

To turn correction off, add the following to `~/.config/parakeetd/config.toml`.

```toml
[correction]
enabled = false
```

`judge_margin` is the log-likelihood difference, in nats, that a candidate needs to be accepted (default 2.0).
Larger values accept fewer candidates, smaller values more.
The former `margin` setting was removed; the daemon refuses to start while the config file still contains it, so delete it.

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

While that microphone is unplugged, the default source is recorded; once it is connected again, recording returns to it.

`partial_interval_ms` sets how often the live transcript updates, in milliseconds (default 500).
Set it to 0 to turn the live transcript off.

```toml
partial_interval_ms = 0
```

`commit_after_seconds` (default 60) sets how much recording is held before a part is inserted during long dictation.
A part without any pause is cut at its quietest moment once it reaches `max_seconds` (default 120).
`max_recording_seconds` (default 600) ends a recording by itself.

```toml
commit_after_seconds = 30
max_recording_seconds = 900
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

The NVIDIA Parakeet models downloaded at install time are distributed under [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/), the Silero VAD model under the MIT License, jinen-v2-small under [CC BY-SA 4.0](https://creativecommons.org/licenses/by-sa/4.0/), and TinySwallow-1.5B under the Apache License 2.0.
The TinySwallow-1.5B model card describes the model as an experimental prototype for research and development, not intended for commercial use or mission-critical deployment, used at the user's own risk and without guaranteed performance.
The models are not part of the package.

The daemon embeds [llama.cpp](https://github.com/ggml-org/llama.cpp) (MIT License) to run the correction models and the IPADIC dictionary (`licenses/ipadic.LICENSE`) to look up readings.

See `THIRD_PARTY_NOTICES.md` for the full list of bundled and downloaded components.
