# Build stage
FROM rust:latest AS builder
WORKDIR /build
COPY . .
RUN cargo build --release

# Runtime stage
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/azbridge /usr/local/bin/azbridge
ENTRYPOINT ["azbridge"]
