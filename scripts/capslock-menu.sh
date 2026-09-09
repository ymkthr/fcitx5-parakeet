#!/usr/bin/env bash
# Make CapsLock act as the Menu key (fcitx5-parakeet's default trigger) via the
# desktop's XKB options. Only GNOME is automated; other desktops get instructions.
#
#   scripts/capslock-menu.sh apply    add caps:menu (keeps other options)
#   scripts/capslock-menu.sh revert   remove caps:menu only
set -euo pipefail

SCHEMA=org.gnome.desktop.input-sources
KEY=xkb-options
OPTION=caps:menu

on_gnome() {
  [[ ${XDG_CURRENT_DESKTOP:-} == *GNOME* ]] && command -v gsettings >/dev/null 2>&1
}

# One option per line. GNOME only stores plain XKB option names in this key
# (no quotes, commas or spaces inside a name), so a token split is enough.
read_options() {
  local raw item
  local -a items=()
  raw=$(gsettings get "$SCHEMA" "$KEY")
  raw=${raw#@as }
  raw=${raw#[}
  raw=${raw%]}
  IFS=',' read -ra items <<<"$raw"
  for item in "${items[@]}"; do
    item=${item//[\'\" ]/}
    [[ -n $item ]] && echo "$item"
  done
}

write_options() {
  if (($# == 0)); then
    gsettings reset "$SCHEMA" "$KEY"
    return
  fi
  local value
  value=$(printf "'%s', " "$@")
  gsettings set "$SCHEMA" "$KEY" "[${value%, }]"
}

manual_instructions() {
  cat <<EOF
Not a GNOME session; make CapsLock a Menu key yourself (XKB option '$OPTION'):
  X11 session   setxkbmap -option $OPTION          (until logout)
  KDE Plasma    System Settings > Keyboard > Key Bindings > Caps Lock behavior
                > "Make Caps Lock an additional Menu key"
  Sway          input type:keyboard xkb_options $OPTION
Or keep CapsLock and choose another trigger key in fcitx5-configtool
(Addons > Parakeet Voice Input).
EOF
}

apply() {
  if ! on_gnome; then
    manual_instructions
    return
  fi
  local -a opts=()
  local o
  mapfile -t opts < <(read_options)
  for o in "${opts[@]}"; do
    if [[ $o == "$OPTION" ]]; then
      echo "$OPTION is already set"
      return
    fi
    if [[ $o == caps:* ]]; then
      echo "CapsLock is already remapped by '$o'; leaving it alone."
      echo "Dictate with a Menu key, or choose another trigger key in fcitx5-configtool."
      return
    fi
  done
  write_options "${opts[@]}" "$OPTION"
  printf '\033[1mCapsLock now acts as the Menu key (dictation trigger); its lock function is gone.\033[0m\n'
  echo "Undo with: $0 revert"
}

revert() {
  if ! on_gnome; then
    echo "nothing to revert: $OPTION is only managed on GNOME"
    return
  fi
  local -a opts=() kept=()
  local o
  mapfile -t opts < <(read_options)
  for o in "${opts[@]}"; do
    [[ $o == "$OPTION" ]] || kept+=("$o")
  done
  if ((${#kept[@]} == ${#opts[@]})); then
    echo "$OPTION is not set"
    return
  fi
  write_options "${kept[@]}"
  echo "CapsLock restored"
}

case ${1:-} in
  apply) apply ;;
  revert) revert ;;
  *)
    echo "usage: $0 apply|revert" >&2
    exit 2
    ;;
esac
