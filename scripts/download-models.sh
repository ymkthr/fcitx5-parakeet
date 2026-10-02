#!/usr/bin/env bash
# Fetch the sherpa-onnx exports of the Parakeet models parakeetd uses.
#   ja: nvidia/parakeet-tdt_ctc-0.6b-ja  (sherpa-onnx-nemo-parakeet-tdt_ctc-0.6b-ja-35000-int8)
#   en: nvidia/parakeet-tdt-0.6b-v3      (sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8, 25 languages)
# plus the Silero VAD and the jinen-v2-small kana-kanji model that corrects
# Japanese homophones.
# The v3 model is symlinked from omp's cache when present to avoid a second 640 MB copy.
set -euo pipefail

MODELS_DIR="${PARAKEETD_MODELS_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/parakeetd/models}"
RELEASE="https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models"
JINEN_URL="https://huggingface.co/togatogah/jinen-v2-small.gguf/resolve/main"
JINEN="jinen-v2-small-Q5_K_M.gguf"
JA="sherpa-onnx-nemo-parakeet-tdt_ctc-0.6b-ja-35000-int8"
EN="sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8"
OMP_EN="$HOME/.omp/agent/cache/tiny-models/csukuangfj/$EN"

mkdir -p "$MODELS_DIR"
cd "$MODELS_DIR"

fetch() {
  local name="$1"
  if [ -f "$name/tokens.txt" ]; then
    echo "[skip] $name already present"
    return
  fi
  echo "[get ] $name"
  curl -fL --retry 3 -o "$name.tar.bz2" "$RELEASE/$name.tar.bz2"
  tar xjf "$name.tar.bz2"
  rm -f "$name.tar.bz2"
}

fetch "$JA"

if [ ! -e "$EN" ] && [ -f "$OMP_EN/tokens.txt" ]; then
  echo "[link] $EN -> $OMP_EN"
  ln -s "$OMP_EN" "$EN"
fi
fetch "$EN"

if [ -f silero_vad.onnx ]; then
  echo "[skip] silero_vad.onnx already present"
else
  echo "[get ] silero_vad.onnx"
  curl -fL --retry 3 -o silero_vad.onnx "$RELEASE/silero_vad.onnx"
fi

if [ -f "jinen-v2-small/$JINEN" ]; then
  echo "[skip] jinen-v2-small/$JINEN already present"
else
  echo "[get ] jinen-v2-small/$JINEN"
  mkdir -p jinen-v2-small
  curl -fL --retry 3 -o "jinen-v2-small/$JINEN.part" "$JINEN_URL/$JINEN"
  mv "jinen-v2-small/$JINEN.part" "jinen-v2-small/$JINEN"
fi

echo "models in $MODELS_DIR:"
ls -1 "$MODELS_DIR"
