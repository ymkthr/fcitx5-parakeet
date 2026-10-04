#!/usr/bin/env bash
# Build a .deb or .rpm of the current working tree inside a container.
#
#   packaging/build.sh deb [IMAGE]    default image: debian:trixie
#   packaging/build.sh rpm [IMAGE]    default image: fedora:42
#
# Packages land in packaging/dist/. The tree is exported with git (tracked and
# untracked-but-not-ignored files), so build products never leak in. Needs
# docker or podman; the sherpa-onnx release archive is cached in
# packaging/cache/ so repeated builds do not download it again.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PKG=fcitx5-voice-ja
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/daemon/Cargo.toml" | head -n1)"
SHERPA_VERSION=1.13.7
SHERPA_ARCHIVE="sherpa-onnx-v${SHERPA_VERSION}-linux-x64-shared-no-tts"
SHERPA_URL="https://github.com/k2-fsa/sherpa-onnx/releases/download/v${SHERPA_VERSION}/${SHERPA_ARCHIVE}.tar.bz2"
DIST="$ROOT/packaging/dist"
CACHE="$ROOT/packaging/cache"

usage() { sed -n '2,10p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; }

format=${1:-}
case $format in
  deb) image=${2:-debian:trixie} ;;
  rpm) image=${2:-fedora:42} ;;
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

if [ ! -f "$CACHE/$SHERPA_ARCHIVE.tar.bz2" ]; then
  echo "== fetching $SHERPA_ARCHIVE"
  curl -fL --retry 3 -o "$CACHE/$SHERPA_ARCHIVE.tar.bz2.$$" "$SHERPA_URL"
  mv "$CACHE/$SHERPA_ARCHIVE.tar.bz2.$$" "$CACHE/$SHERPA_ARCHIVE.tar.bz2"
fi

echo "== exporting working tree ($PKG-$VERSION)"
# Written via a per-process temp file so `build.sh deb & build.sh rpm` don't
# read each other's half-written export.
src_tar="$CACHE/$PKG-$VERSION.tar.gz"
(cd "$ROOT" && git ls-files -coz --exclude-standard |
  tar --null -T - --transform "s,^,$PKG-$VERSION/," -czf "$src_tar.$$")
mv "$src_tar.$$" "$src_tar"

echo "== building $format in $image"
case $format in
  deb)
    build_script=$(cat <<EOF
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
apt-get update -q
apt-get install -qy --no-install-recommends build-essential debhelper devscripts ca-certificates
mkdir -p /build && cd /build
tar xzf /in/$PKG-$VERSION.tar.gz
tar xjf /in/$SHERPA_ARCHIVE.tar.bz2
cd $PKG-$VERSION
cp -r packaging/deb/debian debian
apt-get build-dep -qy ./
SHERPA_ONNX_DIR=/build/$SHERPA_ARCHIVE dpkg-buildpackage -b -us -uc
cp ../*.deb /out/
EOF
    )
    ;;
  rpm)
    build_script=$(cat <<EOF
set -euo pipefail
dnf install -qy rpm-build rpmdevtools dnf5-plugins
rpmdev-setuptree
cp /in/$PKG-$VERSION.tar.gz /in/$SHERPA_ARCHIVE.tar.bz2 ~/rpmbuild/SOURCES/
tar xzf /in/$PKG-$VERSION.tar.gz --strip-components=3 -C ~/rpmbuild/SPECS $PKG-$VERSION/packaging/rpm/$PKG.spec
dnf builddep -qy ~/rpmbuild/SPECS/$PKG.spec
rpmbuild -bb ~/rpmbuild/SPECS/$PKG.spec
cp ~/rpmbuild/RPMS/*/*.rpm /out/
EOF
    )
    ;;
esac

# Host networking: the build only fetches sources and needs no bridge setup.
"$engine" run --rm --network host \
  -v "$CACHE:/in:ro" \
  -v "$DIST:/out" \
  "$image" bash -c "$build_script"
