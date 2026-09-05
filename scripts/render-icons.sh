#!/usr/bin/env bash
# Re-render the committed panel icons (fcitx5/icons/hicolor) from fcitx5/icons/template.svg.
# Needs rsvg-convert. Run after editing the template or the flags below.
#
# The indicator is 16-24 px: the parakeet is the constant shape, the language
# is a flag in the bottom-right corner (none for auto), and the background
# turns red / amber while recording / transcribing.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TEMPLATE="$ROOT/fcitx5/icons/template.svg"
OUT="$ROOT/fcitx5/icons/hicolor"
SIZES=(16 22 24 32 48 64 128 256)

command -v rsvg-convert >/dev/null || { echo "rsvg-convert (librsvg) is required"; exit 1; }

calc() { awk "BEGIN { printf \"%.2f\", $1 }"; }

BG_IDLE="#3b5f8a"
BG_RECORDING="#d1352b"
BG_BUSY="#d98c1f"

FLAG_X=63 FLAG_Y=79 FLAG_W=56 FLAG_H=38
frame() {
  printf '<rect x="%s" y="%s" width="%s" height="%s" rx="8" fill="#ffffff"/>' \
    $((FLAG_X - 3)) $((FLAG_Y - 3)) $((FLAG_W + 6)) $((FLAG_H + 6))
  printf '<clipPath id="flag"><rect x="%s" y="%s" width="%s" height="%s" rx="5"/></clipPath>' \
    "$FLAG_X" "$FLAG_Y" "$FLAG_W" "$FLAG_H"
}

flag_japan() {
  frame
  printf '<g clip-path="url(#flag)">'
  printf '<rect x="%s" y="%s" width="%s" height="%s" fill="#ffffff"/>' "$FLAG_X" "$FLAG_Y" "$FLAG_W" "$FLAG_H"
  printf '<circle cx="%s" cy="%s" r="%s" fill="#bc002d"/>' \
    "$(calc "$FLAG_X + $FLAG_W / 2")" "$(calc "$FLAG_Y + $FLAG_H / 2")" "$(calc "$FLAG_H * 0.3")"
  printf '</g>'
}

flag_usa() {
  frame
  printf '<g clip-path="url(#flag)">'
  printf '<rect x="%s" y="%s" width="%s" height="%s" fill="#b22234"/>' "$FLAG_X" "$FLAG_Y" "$FLAG_W" "$FLAG_H"
  local stripe
  stripe="$(calc "$FLAG_H / 13")"
  for i in 1 3 5 7 9 11; do
    printf '<rect x="%s" y="%s" width="%s" height="%s" fill="#ffffff"/>' \
      "$FLAG_X" "$(calc "$FLAG_Y + $i * $stripe")" "$FLAG_W" "$stripe"
  done
  local cw ch
  cw="$(calc "$FLAG_W * 0.42")"
  ch="$(calc "$stripe * 7")"
  printf '<rect x="%s" y="%s" width="%s" height="%s" fill="#3c3b6e"/>' "$FLAG_X" "$FLAG_Y" "$cw" "$ch"
  # A 3x3 hint of stars; individual stars cannot survive 16-24 px anyway.
  for row in 1 2 3; do
    for col in 1 2 3; do
      printf '<circle cx="%s" cy="%s" r="1.4" fill="#ffffff"/>' \
        "$(calc "$FLAG_X + $cw * $col / 4")" "$(calc "$FLAG_Y + $ch * $row / 4")"
    done
  done
  printf '</g>'
}

ICONS=(
  "fcitx-parakeet-ja        $BG_IDLE      flag_japan"
  "fcitx-parakeet-en        $BG_IDLE      flag_usa"
  "fcitx-parakeet-auto      $BG_IDLE"
  "fcitx-parakeet-recording $BG_RECORDING"
  "fcitx-parakeet-busy      $BG_BUSY"
)

render() {
  local name="$1" bg="$2" badge_fn="${3:-}"
  local bird badge=""
  if [ -n "$badge_fn" ]; then
    # Bird tucked to the top-left so the flag in the bottom-right corner stays clear.
    bird="translate(-8 0) scale(0.8)"
    badge="$("$badge_fn")"
  else
    bird="translate(6 0) scale(0.95)"
  fi
  local svg
  svg="$(sed -e "s|@BG@|$bg|g" -e "s|@BIRD@|$bird|" -e "s|@BADGE@|$badge|" "$TEMPLATE")"
  for size in "${SIZES[@]}"; do
    mkdir -p "$OUT/${size}x${size}/apps"
    rsvg-convert -w "$size" -h "$size" -o "$OUT/${size}x${size}/apps/$name.png" <<<"$svg"
  done
  echo "rendered $name"
}

rm -rf "$OUT"
for spec in "${ICONS[@]}"; do
  # shellcheck disable=SC2086
  render $spec
done
