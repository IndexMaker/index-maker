# Stage 1: Builder - This stage compiles your Rust application.
# We use a Rust image that is based on Debian (or similar, rust:latest is usually Debian-based).
# This provides a more convenient environment for compilation and linking,
# especially when dealing with various C dependencies, before targeting musl.
FROM rust:latest AS builder

# Set working directory inside the builder container
WORKDIR /app

# Install necessary build-time dependencies for common Rust crates
# For example, if your project uses OpenSSL for HTTPS (via `native-tls`), you'll need its dev packages.
# If you use `rustls`, you often won't need these.
# pkg-config is often needed for various C bindings.
RUN apt-get update && apt-get install -y --no-install-recommends \
    musl-tools \
    pkg-config \
    libssl-dev \
    # Add any other build dependencies for your specific C-linked crates here, e.g.:
    # libpq-dev (for PostgreSQL)
    # libsqlite3-dev (for SQLite)
    # libmysqlclient-dev (for MySQL/MariaDB)
    # libxml2-dev libxslt1-dev (if using xml/xslt parsing libraries)
    && rm -rf /var/lib/apt/lists/*

# Add the x86_64-unknown-linux-musl target for static compilation
# Replace x86_64 with aarch64 or other if targeting a different architecture
RUN rustup target add x86_64-unknown-linux-musl

# Copy only Cargo.toml and Cargo.lock first to leverage Docker's layer caching.
# This ensures that if your source code changes, but dependencies don't,
# Cargo won't re-download and recompile all dependencies.
COPY Cargo.toml Cargo.lock ./

# Create a dummy src/main.rs and build to cache dependencies.
# This is a common optimization: if Cargo.toml/Cargo.lock don't change,
# this layer and its subsequent dependency compilation will be cached.
# If your project has a build.rs, you might need to copy more files here before this step.
# RUN mkdir src && echo "fn main() {println!(\"Caching...\");}" > src/main.rs \
#     && cargo build --release --target=x86_64-unknown-linux-musl \
#     && rm src/main.rs

# Copy the rest of your application source code
COPY . .

# Set RUSTFLAGS for static linking against musl.
# `+crt-static` links the C runtime statically.
# `link-self-contained=yes` ensures Rust's standard library is also linked statically.
ENV RUSTFLAGS="-C target-feature=+crt-static -C link-self-contained=yes"

# Build your Rust application in release mode for the musl target.
# Replace `your_app_name` with the actual name of your binary if it's different from the package name in Cargo.toml.
# Cargo will usually name the binary after the `[package].name` in Cargo.toml by default.
RUN cargo build --release --target=x86_64-unknown-linux-musl

# ---
# Stage 2: Final Alpine Image - This stage creates the smallest possible image.
# We start from a minimal Alpine base.
FROM alpine:latest

# Install ca-certificates if your application makes HTTPS requests.
# Most well-built musl Rust binaries are fully static and only need this for HTTPS.
RUN apk add --no-cache ca-certificates

# Set the working directory for the final application
WORKDIR /app

# Copy the compiled binary from the builder stage into the final Alpine image.
# Replace `your_app_name` with the actual name of the compiled binary.
COPY --from=builder /app/target/x86_64-unknown-linux-musl/release/index-maker ./

# Define the command to run your application when the container starts.
ENTRYPOINT ["./index-maker quote-server"]

# Optional: Expose ports if your application is a web server or listens on a port.
# EXPOSE 8080