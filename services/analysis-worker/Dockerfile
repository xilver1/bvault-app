# syntax=docker/dockerfile:1
#
# Build from the workspace root:
#   docker build -f services/analysis-worker/Dockerfile -t <repo>/analysis-worker:<sha> .

# --- chef -------------------------------------------------------------------
FROM rust:1-slim-bookworm AS chef
RUN cargo install cargo-chef --locked
WORKDIR /app

# --- planner ----------------------------------------------------------------
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# --- builder ----------------------------------------------------------------
FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
# Pulls rekordbox-core + Symphonia + FFT, but the device-library feature stays
# OFF, so this image compiles no SQLCipher/OpenSSL.
RUN cargo chef cook --release -p bvault-analysis-worker --recipe-path recipe.json
COPY . .
RUN cargo build --release -p bvault-analysis-worker

# --- runtime (no HTTP port: this is a queue consumer) -----------------------
FROM gcr.io/distroless/cc-debian12:nonroot AS runtime
COPY --from=builder /app/target/release/analysis-worker /usr/local/bin/analysis-worker
ENTRYPOINT ["/usr/local/bin/analysis-worker"]