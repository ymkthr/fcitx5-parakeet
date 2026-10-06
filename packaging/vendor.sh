#!/usr/bin/env bash
# Write vendor.tar.xz: the crates of daemon/Cargo.lock (vendor/) and the cargo
# config that points at them (.cargo/config.toml), for builds without network.
#
#   packaging/vendor.sh OUT_FILE
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export LC_ALL=C
export TZ=UTC
SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH:-$(git -C "$ROOT" show -s --format=%ct HEAD)}
export SOURCE_DATE_EPOCH
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
XZ_DEFAULTS= XZ_OPT=--threads=1 tar -C "$work" \
  --sort=name \
  --format=gnu \
  --owner=0 \
  --group=0 \
  --numeric-owner \
  --mode='a=rX,u+w' \
  --mtime="@$SOURCE_DATE_EPOCH" \
  -cJf "$out.$$" vendor .cargo
mv "$out.$$" "$out"
