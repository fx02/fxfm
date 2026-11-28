# syntax=docker/dockerfile:1
# Build environment for the Debian packages (scripts/build-linux.sh): Ubuntu 22.04 with the
# WebKitGTK 4.1 stack Tauri 2 needs, the project's Rust toolchain and the Tauri CLI. Built
# against Ubuntu 22.04's glibc (2.35), a package runs on Ubuntu 22.04+, Pop!_OS 22.04+,
# Linux Mint 21+ and Debian 12+.
# The secret `ca` (the Mac's trusted certificates, for a TLS-inspecting network) is used only
# while downloading and is not kept in the image.
FROM ubuntu:22.04
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
      build-essential curl ca-certificates file pkg-config patchelf \
      libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev libayatana-appindicator3-dev \
      libssl-dev libasound2-dev libdbus-1-dev libxdo-dev \
    && rm -rf /var/lib/apt/lists/*
ENV RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo PATH=/usr/local/cargo/bin:$PATH
ARG RUST_VERSION
RUN --mount=type=secret,id=ca \
    curl -sSf --retry 5 --retry-all-errors --cacert /run/secrets/ca -o /tmp/rustup.sh https://sh.rustup.rs \
    && SSL_CERT_FILE=/run/secrets/ca RUSTUP_USE_CURL=1 sh /tmp/rustup.sh -y --profile minimal --default-toolchain "$RUST_VERSION" \
    && rm /tmp/rustup.sh \
    && SSL_CERT_FILE=/run/secrets/ca rustup component add rustfmt clippy
RUN --mount=type=secret,id=ca CARGO_HTTP_CAINFO=/run/secrets/ca cargo install tauri-cli --version "^2" --locked
WORKDIR /src
