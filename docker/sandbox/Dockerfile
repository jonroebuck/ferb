# docker/sandbox/Dockerfile
#
# General-purpose build/execution sandbox image.
# Base: Microsoft's devcontainers universal image (Python, Node.js, Java, Go preinstalled).
# Adds: Rust toolchain from the official Rust image.

FROM rust:1.87-slim AS rust

FROM mcr.microsoft.com/devcontainers/universal:6.0.8-noble

ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH

COPY --from=rust /usr/local/cargo /usr/local/cargo
COPY --from=rust /usr/local/rustup /usr/local/rustup

RUN chmod -R a+w /usr/local/rustup /usr/local/cargo

# Verify install
RUN cargo --version && rustc --version

WORKDIR /workspace
