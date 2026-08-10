# syntax=docker/dockerfile:1
#
# Build from the workspace root:
#   docker build -f services/export-builder/Dockerfile -t <repo>/export-builder:<sha> .
#
# This is the ONE image that compiles OpenSSL — SQLCipher (the device library)
# links a vendored, statically-built OpenSSL. That needs perl + make at build
# time, and makes this the slowest build; cargo-chef caches it so only
# dependency changes pay the cost. No runtime libssl is needed (it's static).

# --- chef: toolchain + the extra build tools OpenSSL needs -------------------
FROM rust:1-slim-bookworm AS chef
RUN apt-get update \
    && apt-get install -y --no-install-recommends perl make \
    && rm -rf /var/lib/apt/lists/*
RUN cargo install cargo-chef --locked
WORKDIR /app

# --- planner ----------------------------------------------------------------
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# --- builder ----------------------------------------------------------------
FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
# Pulls bvault-export -> core[device-library] -> rusqlite (vendored SQLCipher
# + OpenSSL). This cook compiles OpenSSL from C source — slow, but cached.
RUN cargo chef cook --release -p bvault-export-builder --recipe-path recipe.json
COPY . .
RUN cargo build --release -p bvault-export-builder

# --- runtime ----------------------------------------------------------------
FROM gcr.io/distroless/cc-debian12:nonroot AS runtime
# OpenSSL is statically linked into the binary; distroless/cc's glibc is all
# that's needed at runtime.
COPY --from=builder /app/target/release/export-builder /usr/local/bin/export-builder
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/export-builder"]