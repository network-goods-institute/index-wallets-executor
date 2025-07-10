FROM rust:1.83

# Install dependencies
RUN apt-get update && apt-get install -y \
    protobuf-compiler \
    libprotobuf-dev \
    clang \
    libclang-dev \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Set working directory
WORKDIR /app

# Copy everything
COPY . .

# Set the cargo registry
ENV CARGO_REGISTRIES_DELTA_INDEX=sparse+https://crates.repyhlabs.dev/api/v1/crates/

# Configure cargo to use the token from environment
ARG CARGO_REGISTRIES_DELTA_TOKEN
RUN mkdir -p $HOME/.cargo && \
    echo '[registries.delta]' > $HOME/.cargo/config.toml && \
    echo 'index = "sparse+https://crates.repyhlabs.dev/api/v1/crates/"' >> $HOME/.cargo/config.toml && \
    echo "[registries.delta]" >> $HOME/.cargo/credentials.toml && \
    echo "token = \"${CARGO_REGISTRIES_DELTA_TOKEN}\"" >> $HOME/.cargo/credentials.toml

# Build the application
RUN cargo build --release

# Expose port
EXPOSE 8081

# Run the binary
CMD ["./target/release/actix_server"]