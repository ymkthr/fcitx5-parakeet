#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

step() { printf '\n\033[1m== %s\033[0m\n' "$*"; }

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
systemctl --user enable --now parakeetd.socket
systemctl --user try-restart parakeetd.service
parakeet-ctl hello

step "restart fcitx5"
if busctl --user status org.fcitx.Fcitx5 >/dev/null 2>&1; then
  busctl --user call org.fcitx.Fcitx5 /controller org.fcitx.Fcitx.Controller1 Restart
else
  echo "fcitx5 is not running; start it so the module loads"
fi

step "done"
echo "Tap or hold the trigger key (Menu, or CapsLock if remapped) to dictate."
