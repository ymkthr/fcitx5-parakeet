#!/usr/bin/env bash
# Regenerate licenses/rust-crates.LICENSE after daemon/Cargo.lock changes.
# Needs cargo-about: cargo install --locked cargo-about --features cli
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
cargo about generate \
  --manifest-path "$root/daemon/Cargo.toml" \
  --config "$root/licenses/about.toml" \
  --locked \
  --output-file "$root/licenses/rust-crates.LICENSE" \
  "$root/licenses/about.hbs"
