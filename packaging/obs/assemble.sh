#!/usr/bin/env bash
# Write the OBS package files into DIR (normally an `osc checkout` of the
# package): _service, the spec, debian.tar.xz, a .dsc for debtransform and
# vendor.tar.xz.
#
#   packaging/obs/assemble.sh DIR
#
# The release tarball, sherpa-onnx and IPADIC are not written; OBS fetches
# them through _service. The crates are vendored here because the public OBS
# instance has no cargo_vendor source service.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PKG=fcitx5-voice-ja
DEBIAN="$ROOT/packaging/deb/debian"
export LC_ALL=C
export TZ=UTC
SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH:-$(git -C "$ROOT" show -s --format=%ct HEAD)}
export SOURCE_DATE_EPOCH

out=${1:?usage: packaging/obs/assemble.sh DIR}
mkdir -p "$out"

deb_version="$(sed -n '1s/^[^ ]* (\([^)]*\)).*/\1/p' "$DEBIAN/changelog")"
upstream_version=${deb_version%-*}
rules_var() { sed -n "s/^$1 = //p" "$DEBIAN/rules"; }
sherpa="$(rules_var SHERPA_ARCHIVE).tar.bz2"
ipadic="$(rules_var IPADIC_ARCHIVE)"

cp "$ROOT/packaging/obs/_service" "$ROOT/packaging/rpm/$PKG.spec" "$out/"
"$ROOT/packaging/vendor.sh" "$out/vendor.tar.xz"
XZ_DEFAULTS= XZ_OPT=--threads=1 tar -C "$DEBIAN/.." \
  --sort=name \
  --format=gnu \
  --owner=0 \
  --group=0 \
  --numeric-owner \
  --mode='a=rX,u+w' \
  --mtime="@$SOURCE_DATE_EPOCH" \
  -cJf "$out/debian.tar.xz" debian

# A .dsc carries the source paragraph of debian/control plus the package list.
# DEBTRANSFORM-FILES puts the extra archives at the top of the unpacked tree,
# where debian/rules looks for them.
{
  echo "Format: $(cat "$DEBIAN/source/format")"
  awk '/^$/ { exit } /^Rules-Requires-Root:/ { next } { print }' "$DEBIAN/control"
  echo "Binary: $(sed -n 's/^Package: //p' "$DEBIAN/control" | paste -sd, | sed 's/,/, /g')"
  sed -n 's/^Architecture: /Architecture: /p' "$DEBIAN/control" | head -n1
  echo "Version: $deb_version"
  echo "DEBTRANSFORM-TAR: $PKG-$upstream_version.tar.gz"
  echo "DEBTRANSFORM-FILES: $sherpa vendor.tar.xz $ipadic"
} >"$out/$PKG.dsc"
