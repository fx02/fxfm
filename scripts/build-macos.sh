#!/bin/bash
# Universal macOS build, signed with the local self-signed identity "fxfm local signing"
# so macOS keeps folder-access grants (TCC) across builds.
# The identity lives in ~/.fxfm-signing/fxfm-signing.keychain-db (not in the repo).
set -euo pipefail
cd "$(dirname "$0")/.."
KC="$HOME/.fxfm-signing/fxfm-signing.keychain-db"
security unlock-keychain -p "$(cat "$HOME/.fxfm-signing/keychain-pass")" "$KC"
cargo tauri build --target universal-apple-darwin
APP=target/universal-apple-darwin/release/bundle/macos/fxfm.app
codesign --verify --strict "$APP"
codesign -dr - "$APP" 2>&1 | tail -1
lipo -archs "$APP/Contents/MacOS/fxfm"
