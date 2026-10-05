#!/usr/bin/env bash
# Write vendor.tar.xz: the crates of daemon/Cargo.lock (vendor/) and the cargo
# config that points at them (.cargo/config.toml), for builds without network.
#
#   packaging/vendor.sh OUT_FILE
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
out=${1:?usage: packaging/vendor.sh OUT_FILE}

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
cargo vendor --quiet --locked --manifest-path "$ROOT/daemon/Cargo.toml" "$work/vendor" >/dev/null
mkdir "$work/.cargo"
cat >"$work/.cargo/config.toml" <<'EOF'
[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "vendor"
EOF
tar -C "$work" --owner=0 --group=0 --sort=name -cJf "$out.$$" vendor .cargo
mv "$out.$$" "$out"
