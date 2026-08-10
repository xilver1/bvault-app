# syntax=docker/dockerfile:1
#
# Build context MUST be the workspace root (shared crates live outside this dir):
#   docker build -f services/gateway/Dockerfile -t <repo>/gateway:<sha> .
#
# Pin `rust:1-slim-bookworm` to a concrete minor (e.g. rust:1.83-slim-bookworm)
# and cargo-chef to a fixed --version in CI for reproducible builds.

# --- chef: shared toolchain + cargo-chef ------------------------------------
FROM rust:1-slim-bookworm AS chef
RUN cargo install cargo-chef --locked
WORKDIR /app

# --- planner: capture the dependency recipe (no compilation) ----------------
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# --- builder: cook deps (cached), then build just this binary ----------------
FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
# `-p` scopes the cook to the gateway's dependency subtree only. The gateway
# doesn't depend on bvault-core, so this compiles no formats/audio code and
# no OpenSSL. Cached until Cargo.toml / Cargo.lock change.
RUN cargo chef cook --release -p bvault-gateway --recipe-path recipe.json
COPY . .
RUN cargo build --release -p bvault-gateway

# --- runtime: distroless, nonroot, no shell ---------------------------------
FROM gcr.io/distroless/cc-debian12:nonroot AS runtime
COPY --from=builder /app/target/release/gateway /usr/local/bin/gateway
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/gateway"]