#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
KEEP_CAPSLOCK=0

usage() {
  cat <<EOF
usage: scripts/install.sh [--keep-capslock]

Builds and installs the daemon and the fcitx5 module, downloads the models,
enables the systemd user socket, restarts fcitx5 and, on GNOME, makes CapsLock
act as the Menu key (the default dictation trigger).

  --keep-capslock  leave CapsLock alone; dictate with a Menu key or change the
                   trigger key in fcitx5-configtool
EOF
}

for arg in "$@"; do
  case $arg in
    --keep-capslock) KEEP_CAPSLOCK=1 ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      echo "unknown option: $arg" >&2
      usage >&2
      exit 2
      ;;
  esac
done

step() { printf '\n\033[1m== %s\033[0m\n' "$*"; }

step "migrate from fcitx5-parakeet"
if pacman -Q fcitx5-parakeet >/dev/null 2>&1; then
  systemctl --user disable --now parakeetd.socket parakeetd.service >/dev/null 2>&1 || true
  sudo pacman -R --noconfirm fcitx5-parakeet
fi
move_once() { if [[ -e $1 && ! -e $2 ]]; then mkdir -p "$(dirname "$2")" && mv -v "$1" "$2"; fi; }
move_once "$HOME/.config/parakeetd" "$HOME/.config/voice-jad"
move_once "$HOME/.local/share/parakeetd" "$HOME/.local/share/voice-jad"
move_once "$HOME/.config/fcitx5/conf/parakeet.conf" "$HOME/.config/fcitx5/conf/voiceja.conf"
if [[ -f $HOME/.config/voice-jad/config.toml ]]; then
  sed -i 's#/parakeetd/#/voice-jad/#g' "$HOME/.config/voice-jad/config.toml"
fi

step "native daemon and fcitx5 addon"
(cd "$ROOT/packaging/arch" && makepkg -sif --noconfirm)
# Remove files installed by the former Python/uv version. User-unit copies
# otherwise shadow the package's units under /usr/lib/systemd/user.
if command -v uv >/dev/null 2>&1; then
  uv tool uninstall parakeetd >/dev/null 2>&1 || true
fi
rm -f "$HOME/.config/systemd/user/parakeetd.service" "$HOME/.config/systemd/user/parakeetd.socket"

step "models"
"$ROOT/scripts/download-models.sh"

step "systemd --user socket"
systemctl --user daemon-reload
systemctl --user enable --now voice-jad.socket
systemctl --user try-restart voice-jad.service
voice-ja-ctl hello

step "restart fcitx5"
if busctl --user status org.fcitx.Fcitx5 >/dev/null 2>&1; then
  busctl --user call org.fcitx.Fcitx5 /controller org.fcitx.Fcitx.Controller1 Restart
else
  echo "fcitx5 is not running; start it so the module loads"
fi

step "trigger key"
if ((KEEP_CAPSLOCK)); then
  echo "leaving CapsLock alone; the trigger key is Menu"
else
  "$ROOT/scripts/capslock-menu.sh" apply
fi

step "done"
echo "Press CapsLock (or Menu) in a text field: a microphone mark near the cursor means the key reached fcitx5."
echo "No mark: make sure the keyboard itself (firmware, key remapper) emits CapsLock."
