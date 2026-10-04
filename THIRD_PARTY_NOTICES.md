# Third-party notices

fcitx5-voice-ja itself is released under the MIT License (see `LICENSE`).
The binary package and the models it downloads include or depend on the
following third-party components.

## Shipped in the package

### sherpa-onnx

- Copyright (c) 2023 Xiaomi Corporation
- License: Apache License 2.0 (`LICENSE-APACHE-2.0`)
- Source: https://github.com/k2-fsa/sherpa-onnx
- Files: `/usr/lib/voice-jad/libsherpa-onnx-c-api.so`, `/usr/lib/voice-jad/libsherpa-onnx-cxx-api.so`,
  and the C API header vendored at `daemon/vendor/sherpa-onnx/c-api.h`

### ONNX Runtime

- Copyright (c) Microsoft Corporation
- License: MIT License (`licenses/onnxruntime.LICENSE`)
- Source: https://github.com/microsoft/onnxruntime
- Files: `/usr/lib/voice-jad/libonnxruntime.so` (bundled in the sherpa-onnx release archive)

### llama.cpp

- Copyright (c) 2023-2026 The ggml authors
- License: MIT License (`licenses/llama.cpp.LICENSE`)
- Source: https://github.com/ggml-org/llama.cpp
- Statically linked into `voice-jad` through the `llama-cpp-2` crate; runs the correction models

### IPADIC (mecab-ipadic 2.7.0)

- Copyright 2000-2003 Nara Institute of Science and Technology
- License: IPADIC license (`licenses/ipadic.LICENSE`)
- Source: https://github.com/lindera/lindera (dictionary build of mecab-ipadic-2.7.0-20250920)
- Embedded in `voice-jad` through the `lindera-ipadic` crate; provides readings for the correction model

### Rust crates

The daemon statically links the crates in `daemon/Cargo.lock` that are not
build-time only (`cargo tree -e normal`). They are available under MIT and/or
Apache-2.0, except the ICU4X crates (`icu_*`, `zerovec`, `yoke` and friends),
which are under Unicode-3.0, and `encoding_rs`, which adds BSD-3-Clause for
its data. Some crates also offer BSL-1.0, Unlicense or Zlib as alternatives.

## Downloaded at install time

The installer downloads these models into `~/.local/share/voice-jad/models/`.
They are not part of the package.

### NVIDIA Parakeet TDT 0.6B v3 (`parakeet-tdt-0.6b-v3`)

- Copyright NVIDIA Corporation
- License: CC BY 4.0 (https://creativecommons.org/licenses/by/4.0/)
- Source: https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3
- Downloaded as the sherpa-onnx export `sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8`

### NVIDIA Parakeet TDT-CTC 0.6B Japanese (`parakeet-tdt_ctc-0.6b-ja`)

- Copyright NVIDIA Corporation
- License: CC BY 4.0 (https://creativecommons.org/licenses/by/4.0/)
- Source: https://huggingface.co/nvidia/parakeet-tdt_ctc-0.6b-ja
- Downloaded as the sherpa-onnx export `sherpa-onnx-nemo-parakeet-tdt_ctc-0.6b-ja-35000-int8`

### Silero VAD

- Copyright (c) 2020-present Silero Team
- License: MIT License
- Source: https://github.com/snakers4/silero-vad
- Downloaded as `silero_vad.onnx` from the sherpa-onnx release assets

### jinen-v2-small

- Created by Hitoshi Togasaki; its training data includes bibliographic data processed from
  the National Diet Library (国立国会図書館「全国書誌データ」), which does not endorse the model
- License: CC BY-SA 4.0 (https://creativecommons.org/licenses/by-sa/4.0/)
- Source: https://huggingface.co/togatogah/jinen-v2-small.gguf
- Downloaded as `jinen-v2-small/jinen-v2-small-Q5_K_M.gguf`; corrects Japanese homophones

### TinySwallow-1.5B

- Copyright Sakana AI and the Swallow team; distilled from Qwen2.5-32B-Instruct into
  Qwen2.5-1.5B-Instruct (both Apache License 2.0)
- License: Apache License 2.0 (`LICENSE-APACHE-2.0`)
- Source: https://huggingface.co/SakanaAI/TinySwallow-1.5B, quantized to GGUF by mmnga at
  https://huggingface.co/mmnga/TinySwallow-1.5B-gguf
- Downloaded as `tinyswallow-1.5b/TinySwallow-1.5B-Q4_K_M.gguf`; judges the homophone
  corrections jinen-v2-small proposes
- The model card states that the model is an experimental prototype for research and
  development, not intended for commercial use or mission-critical deployment, and that it is
  provided without any guarantee or liability
