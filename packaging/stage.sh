#!/usr/bin/env bash
# Stage the built daemon and fcitx5 addon into a package root. Shared by the
# deb and rpm builds; the PKGBUILDs keep their own install() to stay
# self-contained for AUR.
#
#   packaging/stage.sh DESTDIR SHERPA_ONNX_LIB_DIR LIBDIR [LICENSEDIR]
#
#   DESTDIR             package root (debian/<pkg>, %{buildroot}, ...)
#   SHERPA_ONNX_LIB_DIR unpacked sherpa-onnx release lib/ directory
#   LIBDIR              /usr/lib or /usr/lib64; parakeetd's private libraries
#                       go to LIBDIR/parakeetd and must match SHERPA_ONNX_RPATH
#   LICENSEDIR          default /usr/share/licenses/fcitx5-parakeet
#
# Runs from the source root after `cmake --build fcitx-build` and
# `cargo build --release` in daemon/.
set -euo pipefail

dest=$1
sherpa_lib=$2
libdir=$3
pkg=fcitx5-parakeet
licensedir=${4:-/usr/share/licenses/$pkg}

DESTDIR="$dest" cmake --install fcitx-build
install -Dm755 daemon/target/release/parakeetd "$dest/usr/bin/parakeetd"
install -Dm755 daemon/target/release/parakeet-ctl "$dest/usr/bin/parakeet-ctl"
install -Dm755 scripts/download-models.sh "$dest/usr/bin/parakeetd-download-models"
install -Dm755 scripts/capslock-menu.sh "$dest/usr/bin/parakeet-capslock-menu"
install -Dm755 -t "$dest$libdir/parakeetd" "$sherpa_lib"/*.so
install -Dm644 -t "$dest/usr/lib/systemd/user" systemd/parakeetd.service systemd/parakeetd.socket
install -Dm644 daemon/config.example.toml "$dest/usr/share/doc/$pkg/config.example.toml"
install -Dm644 THIRD_PARTY_NOTICES.md "$dest/usr/share/doc/$pkg/THIRD_PARTY_NOTICES.md"
install -Dm644 LICENSE "$dest$licensedir/LICENSE"
install -Dm644 LICENSE-APACHE-2.0 "$dest$licensedir/LICENSE-APACHE-2.0"
install -Dm644 licenses/onnxruntime.LICENSE "$dest$licensedir/onnxruntime.LICENSE"
