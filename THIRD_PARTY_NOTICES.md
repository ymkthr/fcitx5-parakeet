# Third-party notices

fcitx5-parakeet itself is released under the MIT License (see `LICENSE`).
The binary package and the models it downloads include or depend on the
following third-party components.

## Shipped in the package

### sherpa-onnx

- Copyright (c) 2023 Xiaomi Corporation
- License: Apache License 2.0 (`LICENSE-APACHE-2.0`)
- Source: https://github.com/k2-fsa/sherpa-onnx
- Files: `/usr/lib/parakeetd/libsherpa-onnx-c-api.so`, `/usr/lib/parakeetd/libsherpa-onnx-cxx-api.so`,
  and the C API header vendored at `daemon/vendor/sherpa-onnx/c-api.h`

### ONNX Runtime

- Copyright (c) Microsoft Corporation
- License: MIT License (`licenses/onnxruntime.LICENSE`)
- Source: https://github.com/microsoft/onnxruntime
- Files: `/usr/lib/parakeetd/libonnxruntime.so` (bundled in the sherpa-onnx release archive)

### Rust crates

The daemon statically links the crates listed in `daemon/Cargo.lock`.
All of them are available under MIT and/or Apache-2.0 (some additionally under Unicode-3.0 or Zlib).

## Downloaded at install time

The installer downloads these models into `~/.local/share/parakeetd/models/`.
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
