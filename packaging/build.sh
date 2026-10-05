#!/usr/bin/env bash
# Build a .deb or .rpm of the current working tree inside a container.
#
#   packaging/build.sh deb [IMAGE]    default image: debian:trixie
#   packaging/build.sh rpm [IMAGE]    default image: fedora:44
#
# Packages land in packaging/dist/. The tree is exported with git (tracked and
# untracked-but-not-ignored files), so build products never leak in. The build
# itself runs without network, like on OBS: the sherpa-onnx release, the IPADIC
# source and the vendored crates are prepared beforehand and cached in
# packaging/cache/. Needs docker or podman, and cargo on the host for
# `cargo vendor`.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PKG=fcitx5-voice-ja
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/daemon/Cargo.toml" | head -n1)"
SHERPA_VERSION=1.13.7
SHERPA_ARCHIVE="sherpa-onnx-v${SHERPA_VERSION}-linux-x64-shared-no-tts"
SHERPA_URL="https://github.com/k2-fsa/sherpa-onnx/releases/download/v${SHERPA_VERSION}/${SHERPA_ARCHIVE}.tar.bz2"
IPADIC_ARCHIVE=mecab-ipadic-2.7.0-20250920.tar.gz
IPADIC_URL="https://Lindera.dev/$IPADIC_ARCHIVE"
DIST="$ROOT/packaging/dist"
CACHE="$ROOT/packaging/cache"

usage() { sed -n '2,12p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; }

format=${1:-}
case $format in
  deb) image=${2:-debian:trixie} ;;
  rpm) image=${2:-fedora:44} ;;
  *)
    usage >&2
    exit 2
    ;;
esac

if command -v docker >/dev/null 2>&1; then
  engine=docker
elif command -v podman >/dev/null 2>&1; then
  engine=podman
else
  echo "docker or podman is required" >&2
  exit 1
fi

mkdir -p "$DIST" "$CACHE"

fetch() {
  local file=$1 url=$2
  [ -f "$CACHE/$file" ] && return
  echo "== fetching $file"
  curl -fL --retry 3 -o "$CACHE/$file.$$" "$url"
  mv "$CACHE/$file.$$" "$CACHE/$file"
}
fetch "$SHERPA_ARCHIVE.tar.bz2" "$SHERPA_URL"
fetch "$IPADIC_ARCHIVE" "$IPADIC_URL"

if [ ! -f "$CACHE/vendor.tar.xz" ] || [ "$ROOT/daemon/Cargo.lock" -nt "$CACHE/vendor.tar.xz" ]; then
  echo "== vendoring crates"
  "$ROOT/packaging/vendor.sh" "$CACHE/vendor.tar.xz"
fi

echo "== exporting working tree ($PKG-$VERSION)"
# Written via a per-process temp file so `build.sh deb & build.sh rpm` don't
# read each other's half-written export.
src_tar="$CACHE/$PKG-$VERSION.tar.gz"
(cd "$ROOT" && git ls-files -coz --exclude-standard |
  tar --null -T - --transform "s,^,$PKG-$VERSION/," -czf "$src_tar.$$")
mv "$src_tar.$$" "$src_tar"

echo "== building $format in $image"
inputs="$PKG-$VERSION.tar.gz $SHERPA_ARCHIVE.tar.bz2 vendor.tar.xz $IPADIC_ARCHIVE"
case $format in
  deb)
    deps_script=$(cat <<EOF
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
apt-get update -q
apt-get install -qy --no-install-recommends build-essential debhelper devscripts ca-certificates
mkdir -p /tmp/deps && tar xzf /in/$PKG-$VERSION.tar.gz -C /tmp/deps
apt-get build-dep -qy /tmp/deps/$PKG-$VERSION/packaging/deb
rm -rf /tmp/deps
EOF
    )
    build_script=$(cat <<EOF
set -euo pipefail
mkdir -p /build && cd /build
tar xzf /in/$PKG-$VERSION.tar.gz
cd $PKG-$VERSION
cp -r packaging/deb/debian debian
for f in $inputs; do cp /in/\$f .; done
dpkg-buildpackage -b -us -uc
cp ../*.deb /out/
EOF
    )
    ;;
  rpm)
    deps_script=$(cat <<EOF
set -euo pipefail
dnf install -qy rpm-build rpmdevtools dnf5-plugins
tar xzf /in/$PKG-$VERSION.tar.gz --strip-components=3 -C /tmp $PKG-$VERSION/packaging/rpm/$PKG.spec
dnf builddep -qy /tmp/$PKG.spec
rm /tmp/$PKG.spec
EOF
    )
    build_script=$(cat <<EOF
set -euo pipefail
rpmdev-setuptree
for f in $inputs; do cp /in/\$f ~/rpmbuild/SOURCES/; done
tar xzf /in/$PKG-$VERSION.tar.gz --strip-components=3 -C ~/rpmbuild/SPECS $PKG-$VERSION/packaging/rpm/$PKG.spec
rpmbuild -bb ~/rpmbuild/SPECS/$PKG.spec
cp ~/rpmbuild/RPMS/*/*.rpm /out/
EOF
    )
    ;;
esac

# Build dependencies are installed with network into a throwaway image; the
# build then runs in that image without network, as OBS builds do.
deps_image="localhost/$PKG-builddeps-$format:$$"
deps_container="$("$engine" create --network host -v "$CACHE:/in:ro" "$image" bash -c "$deps_script")"
"$engine" start -a "$deps_container"
"$engine" commit "$deps_container" "$deps_image" >/dev/null
"$engine" rm "$deps_container" >/dev/null
"$engine" run --rm --network none \
  -v "$CACHE:/in:ro" \
  -v "$DIST:/out" \
  "$deps_image" bash -c "$build_script"
"$engine" rmi "$deps_image" >/dev/null
