#!/usr/bin/env bash
# Fetch the sherpa-onnx exports of the Parakeet models voice-jad uses.
#   ja: nvidia/parakeet-tdt_ctc-0.6b-ja  (sherpa-onnx-nemo-parakeet-tdt_ctc-0.6b-ja-35000-int8)
#   en: nvidia/parakeet-tdt-0.6b-v3      (sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8, 25 languages)
# plus the Silero VAD, and for correcting Japanese homophones the
# jinen-v2-small kana-kanji model and the TinySwallow-1.5B language model.
# The v3 model is symlinked from omp's cache when present to avoid a second 640 MB copy.
set -euo pipefail

MODELS_DIR="${VOICE_JAD_MODELS_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/voice-jad/models}"
RELEASE="https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models"
JINEN_URL="https://huggingface.co/togatogah/jinen-v2-small.gguf/resolve/main/jinen-v2-small-Q5_K_M.gguf"
JUDGE_URL="https://huggingface.co/mmnga/TinySwallow-1.5B-gguf/resolve/main/TinySwallow-1.5B-Q4_K_M.gguf"
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

# fetch_file <dir> <url>: a partial download never passes for the model.
fetch_file() {
  local path="$1/${2##*/}"
  if [ -f "$path" ]; then
    echo "[skip] $path already present"
    return
  fi
  echo "[get ] $path"
  mkdir -p "$1"
  curl -fL --retry 3 -o "$path.part" "$2"
  mv "$path.part" "$path"
}

fetch_file jinen-v2-small "$JINEN_URL"
fetch_file tinyswallow-1.5b "$JUDGE_URL"

echo "models in $MODELS_DIR:"
ls -1 "$MODELS_DIR"
