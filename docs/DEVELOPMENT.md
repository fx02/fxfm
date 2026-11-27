# fxfm development

Building and testing fxfm.

## Build

You need Rust (the toolchain from `rust-toolchain.toml`, with the `aarch64-apple-darwin` and `x86_64-apple-darwin` targets) and `tauri-cli` 2 (`cargo install tauri-cli --version "^2"`).

```sh
rustup target add aarch64-apple-darwin x86_64-apple-darwin
cargo tauri build --target universal-apple-darwin
```

Output (universal, runs on Apple Silicon and Intel Macs):

- app: `target/universal-apple-darwin/release/bundle/macos/fxfm.app`
- disk image: `target/universal-apple-darwin/release/bundle/dmg/fxfm_<version>_universal.dmg`

If the `.dmg` step crashes with "Finder is busy" (locked screen, SSH), run the build with `CI=true`: this skips Finder's DMG layout customization.

A plain `cargo tauri build` signs the app ad hoc (`signingIdentity` is `-` in `tauri.conf.json`). `scripts/build-macos.sh` builds with the maintainer's stable local identity "fxfm local signing" instead (kept in `~/.fxfm-signing/`, not in the repo): it unlocks that keychain, sets `APPLE_SIGNING_IDENTITY`, runs the universal build and verifies the signature and architectures. With a stable identity macOS remembers the granted folder access between builds; with an ad hoc signature it asks again after each rebuild.

Check: `lipo -archs target/universal-apple-darwin/release/bundle/macos/fxfm.app/Contents/MacOS/fxfm` prints `x86_64 arm64`.

Tests: `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings` and `node --test ui/`.

