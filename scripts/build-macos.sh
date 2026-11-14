#!/bin/bash
# Universal macOS build, signed with the local self-signed identity "fxfm local signing"
# so macOS keeps folder-access grants (TCC) across builds.
# The identity lives in ~/.fxfm-signing/fxfm-signing.keychain-db (not in the repo).
set -euo pipefail
cd "$(dirname "$0")/.."
KC="$HOME/.fxfm-signing/fxfm-signing.keychain-db"
security unlock-keychain -p "$(cat "$HOME/.fxfm-signing/keychain-pass")" "$KC"
export APPLE_SIGNING_IDENTITY="fxfm local signing"
# Leftover read-write images from an interrupted DMG step keep a volume mounted and make
# the next DMG step fail with "Resource busy": detach them first.
hdiutil info | awk '/image-path/{p=$0} /^\/dev\/disk[0-9]+\t/{if (p ~ /rw\.[0-9]+\.fxfm/) print $1}' | sort -u |
  while read -r d; do hdiutil detach "$d" -force >/dev/null 2>&1 || true; done
rm -f target/universal-apple-darwin/release/bundle/macos/rw.*.dmg
# The DMG step occasionally fails while Finder holds the fresh image; one retry is enough.
cargo tauri build --target universal-apple-darwin || {
  hdiutil info | awk '/image-path/{p=$0} /^\/dev\/disk[0-9]+\t/{if (p ~ /rw\.[0-9]+\.fxfm/) print $1}' | sort -u |
    while read -r d; do hdiutil detach "$d" -force >/dev/null 2>&1 || true; done
  rm -f target/universal-apple-darwin/release/bundle/macos/rw.*.dmg
  sleep 3
  cargo tauri build --target universal-apple-darwin
}
ls target/universal-apple-darwin/release/bundle/dmg/fxfm_*_universal.dmg >/dev/null
APP=target/universal-apple-darwin/release/bundle/macos/fxfm.app
codesign --verify --strict "$APP"
codesign -dr - "$APP" 2>&1 | tail -1
lipo -archs "$APP/Contents/MacOS/fxfm"
