# syntax=docker/dockerfile:1.4

# Stage 1: Build the application
FROM --platform=linux/amd64 rust:alpine3.22 AS builder

WORKDIR /app

# Install musl-dev, openssl-dev, perl, and now build-base
# build-base provides essential build tools like 'make' and 'gcc'
RUN apk update && apk add --no-cache musl-dev openssl-dev perl build-base

COPY . .

# Build the final release binary for the musl target
RUN cargo build --release --target=x86_64-unknown-linux-musl --features alpine-deploy

# Build tracker
RUN cargo build --release -p tracker --target=x86_64-unknown-linux-musl --features alpine-deploy

# Build migrate_persistence
RUN cargo build --release -p migrate_persistence --target=x86_64-unknown-linux-musl --features alpine-deploy

# Stage 2: Create the final, minimal runtime image
FROM --platform=linux/amd64 alpine:3.22

WORKDIR /app

RUN apk add --no-cache ca-certificates

COPY --from=builder /app/target/x86_64-unknown-linux-musl/release/index-maker ./
COPY --from=builder /app/target/x86_64-unknown-linux-musl/release/tracker ./
COPY --from=builder /app/target/x86_64-unknown-linux-musl/release/migrate_persistence ./
COPY --from=builder /app/configs ./configs

CMD ["./index-maker", "-b", "0.0.0.0:3000", "-c", "configs", "quote-server"]

EXPOSE 3000