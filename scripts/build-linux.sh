#!/bin/bash
# Debian packages (.deb) for arm64 and amd64, built in Docker (Rancher Desktop) with
# scripts/linux/Dockerfile. arm64 builds natively on Apple silicon; amd64 is cross-compiled in an
# arm64 container (scripts/linux/Dockerfile.cross-amd64), since emulated x86 rustc crashes.
# The cargo registry and the Linux target dirs live in Docker volumes per architecture, so the
# macOS build in target/ is never touched. Packages land in target/linux/.
#
#   scripts/build-linux.sh            # both architectures
#   scripts/build-linux.sh arm64      # one
#   scripts/build-linux.sh arm64 -- cargo test --workspace   # any command in the container
#   scripts/build-linux.sh smoke arm64   # install + start the built .deb (in Debian 12;
#                                        # SMOKE_IMAGE=ubuntu:22.04 for Ubuntu / Pop!_OS)
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.rd/bin:$PATH"
if ! docker info >/dev/null 2>&1; then
  open -a "Rancher Desktop"
  until docker info >/dev/null 2>&1; do sleep 3; done
fi
if [ "${1:-}" = "smoke" ]; then
  shift
  for arch in "${@:-arm64}"; do
    deb=$(cd target/linux && ls -t fxfm_*_"$arch".deb | head -1)
    docker run --rm --platform "linux/$arch" -v "$PWD/target/linux":/out -v "$PWD/scripts/linux":/s:ro \
      "${SMOKE_IMAGE:-debian:bookworm}" sh /s/smoke.sh "$deb" "$arch"
  done
  exit 0
fi
ARCHS=()
while [ $# -gt 0 ] && [ "$1" != "--" ]; do ARCHS+=("$1"); shift; done
[ "${1:-}" = "--" ] && shift
[ ${#ARCHS[@]} -eq 0 ] && ARCHS=(arm64 amd64)
RV=$(sed -n 's/^channel = "\(.*\)"/\1/p' rust-toolchain.toml)
# The certificates this Mac trusts (system roots and the admin-installed ones, e.g. of a
# TLS-inspecting firewall), for downloads in the container; a temp file, never in the repo.
CA=$(mktemp -t fxfm-ca)
trap 'rm -f "$CA"' EXIT
security find-certificate -a -p /System/Library/Keychains/SystemRootCertificates.keychain /Library/Keychains/System.keychain >"$CA"
OUT=target/linux
mkdir -p "$OUT"
for arch in "${ARCHS[@]}"; do
  if [ "$arch" = amd64 ]; then
    file=scripts/linux/Dockerfile.cross-amd64 target=x86_64-unknown-linux-gnu
  else
    file=scripts/linux/Dockerfile target=
  fi
  docker build --platform linux/arm64 -f "$file" --build-arg RUST_VERSION="$RV" --secret id=ca,src="$CA" -t "fxfm-linux-build:$arch" scripts/linux
  run=(docker run --rm --platform linux/arm64 -e TARGET="$target"
    -v "$PWD":/src
    -v "fxfm-cargo-$arch":/usr/local/cargo/registry
    -v "fxfm-target-$arch":/src/target
    -v "$PWD/$OUT":/out
    -v "$CA":/etc/fxfm-ca.pem:ro -e CARGO_HTTP_CAINFO=/etc/fxfm-ca.pem -e SSL_CERT_FILE=/etc/fxfm-ca.pem
    "fxfm-linux-build:$arch")
  if [ $# -gt 0 ]; then
    "${run[@]}" "$@"
  else
    # Only the package just built (the bundle folder in the volume keeps older versions).
    "${run[@]}" sh -c 'd=target/${TARGET:+$TARGET/}release/bundle/deb && cargo tauri build ${TARGET:+--target "$TARGET"} --bundles deb && cp "$d/$(ls -t "$d" | grep "\.deb$" | head -1)" /out/'
  fi
done
ls -la "$OUT"
