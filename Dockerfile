# syntax=docker/dockerfile:1.4

# Stage 1: Build the application
# We use a glibc-based image for the builder to handle proc-macros,
# even though we target musl for the final binary.
# FROM rust:latest AS builder
FROM --platform=linux/amd64 rustlang/rust:nightly AS builder

# Set the working directory inside the container
WORKDIR /app

# Install build dependencies for OpenSSL and musl-tools
# build-essential includes gcc/g++, pkg-config, etc.
# musl-tools are necessary for the x86_64-unknown-linux-musl target
# libssl-dev is for OpenSSL development headers
RUN apt-get update && apt-get install -y  --no-install-recommends \
    build-essential \
    pkg-config \
    libssl-dev \
    ca-certificates \
    musl-tools \
    && rm -rf /var/lib/apt/lists/*

# Add the musl target to rustup
RUN rustup target add x86_64-unknown-linux-musl

# Copy Cargo.toml and Cargo.lock first to leverage Docker cache
# This ensures dependencies are only recompiled if Cargo.toml/Cargo.lock change
COPY . .

# Build the final release binary for the musl target
# Use the same RUSTFLAGS and OPENSSL_STATIC for consistency
RUN RUSTFLAGS="-C target-feature=+crt-static -C link-self-contained=yes" \
    OPENSSL_STATIC=1 \
    cargo build --target=x86_64-unknown-linux-musl --release --features alpine-deploy

# Stage 2: Create the final, minimal runtime image
# Use alpine as it's very small and compatible with musl binaries
FROM alpine:latest

# Install ca-certificates for HTTPS/TLS, crucial for network communication
RUN apk add --no-cache ca-certificates

# Set the working directory
WORKDIR /app

# Copy the built binary from the builder stage
# Replace `your_app_name` with the actual name of your binary (usually from Cargo.toml's [package] name)
COPY --from=builder /app/target/x86_64-unknown-linux-musl/release/index-maker ./

# Set the default command to run your application
CMD ["./index-maker"]

# Optional: If your application listens on a port, expose it (e.g., for a web server)
EXPOSE 3000
