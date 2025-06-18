# Build stage
FROM rust:1.75 as builder

# Install protobuf compiler and libraries
RUN apt-get update && apt-get install -y \
    protobuf-compiler \
    libprotobuf-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy cargo files first for better caching
COPY Cargo.toml Cargo.lock ./
COPY .cargo .cargo

# Copy source code
COPY src ./src

# Build the application
RUN cargo build --release

# Runtime stage
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy the binary from builder
COPY --from=builder /app/target/release/actix_server /app/

# Expose the port
EXPOSE 8081

# Run the binary
CMD ["./actix_server"]