# docker/sandbox/Dockerfile
#
# General-purpose build/execution sandbox image.
# Base: Microsoft's devcontainers universal image (Python, Node.js, Java, Go preinstalled).
# Adds: Rust toolchain via rustup.

FROM mcr.microsoft.com/devcontainers/universal:latest

# Install Rust for all users via rustup, non-interactively.
ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | \
    sh -s -- -y --default-toolchain stable --no-modify-path \
    && chmod -R a+w /usr/local/rustup /usr/local/cargo

# Make cargo/rustc available on PATH for all users (not just root),
# since devcontainers/universal typically runs as a non-root user.
RUN echo 'export PATH=/usr/local/cargo/bin:$PATH' > /etc/profile.d/rust.sh \
    && chmod +x /etc/profile.d/rust.sh

# Verify install
RUN cargo --version && rustc --version

WORKDIR /workspace
