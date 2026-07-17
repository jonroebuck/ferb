FROM rust:1.87-slim AS builder

WORKDIR /app

RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
COPY workflows/ workflows/
COPY prompts/ prompts/

RUN cargo build --release -p ferb-cli

FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y ca-certificates libssl3 && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/ferb-cli /usr/local/bin/ferb
COPY workflows/ /app/workflows/
COPY prompts/ /app/prompts/

WORKDIR /app

ENV FERB_WORKFLOW=/app/workflows/default.yaml
ENV FERB_PROMPTS_DIR=/app/prompts

EXPOSE 9090

ENTRYPOINT ["ferb"]
