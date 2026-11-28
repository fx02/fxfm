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


## Linux packages

`scripts/build-linux.sh` builds the Debian packages in Docker (Rancher Desktop on the Mac):

```sh
scripts/build-linux.sh                 # arm64 and amd64 → target/linux/*.deb
scripts/build-linux.sh arm64           # one architecture
scripts/build-linux.sh arm64 -- cargo test --workspace   # any command in the build container
scripts/build-linux.sh smoke arm64     # install the .deb in a clean Debian, start it on Xvfb, screenshot
```

- `scripts/linux/Dockerfile`: Ubuntu 22.04 with WebKitGTK 4.1, the toolchain from
  `rust-toolchain.toml` and the Tauri CLI (arm64, native on Apple silicon). Built against
  glibc 2.35, the packages run on Ubuntu 22.04+, Pop!_OS 22.04+, Mint 21+ and Debian 12+.
- `scripts/linux/Dockerfile.cross-amd64`: amd64 is cross-compiled in an arm64 container with
  Ubuntu's multiarch amd64 libraries. Emulated x86 rustc crashes under QEMU, and Rosetta did
  not work with Rancher's kernel. Four -dev packages that pull gobject-introspection:amd64 are
  unpacked instead of installed.
- **Certificates:** the script passes the certificates the Mac trusts to the container
  (a temporary file, never in the repository or the image), so downloads work behind a
  TLS-inspecting network.
- **Caches:** the cargo registry and the target dirs are Docker volumes per architecture, so
  the macOS build in `target/` is untouched.
