# Multi-stage build for optimized image size
# Build arguments for flexibility
ARG RUST_VERSION=1.81

#==============================================================================
# Builder Stage - Use Debian slim for better compatibility with RocksDB
#==============================================================================
FROM rust:${RUST_VERSION}-slim AS builder

# Install build dependencies for static linking
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    clang \
    libclang-dev \
    build-essential \
    && rm -rf /var/lib/apt/lists/*

# Set build environment for optimized builds
ENV RUSTFLAGS="-C link-arg=-s"
ENV CC=clang
ENV CXX=clang++

# Install nightly toolchain so Cargo can parse newer manifest features (eg. edition2024)
RUN rustup toolchain install nightly && rustup default nightly && rustup show

# Create app user for security
RUN groupadd -g 1000 clutch && \
    useradd -r -u 1000 -g clutch -s /bin/sh clutch

WORKDIR /usr/src/clutch-node

# Copy dependency files for better caching
COPY Cargo.toml Cargo.lock ./

# Create dummy source and build dependencies
RUN mkdir src && \
    echo "fn main() {}" > src/main.rs && \
    cargo +nightly build --release && \
    rm -rf src

# Copy actual source code
COPY src ./src

# Build the final binary
RUN cargo +nightly build --release --bin clutch-node

# Strip the binary to reduce size further
RUN strip target/release/clutch-node

#==============================================================================
# Runtime Stage - Minimal Debian image
#==============================================================================
FROM debian:bookworm-slim

# Install only essential runtime dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    tzdata \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user
RUN groupadd -g 1000 clutch && \
    useradd -r -u 1000 -g clutch -s /bin/sh clutch

# Create directories with proper permissions
RUN mkdir -p /usr/local/bin /app/config && \
    chown -R clutch:clutch /app

# Copy the optimized binary
COPY --from=builder /usr/src/clutch-node/target/release/clutch-node /usr/local/bin/clutch-node

# Set permissions and switch to non-root user
RUN chmod +x /usr/local/bin/clutch-node
USER clutch

# Set working directory
WORKDIR /app

# Health check for container monitoring
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD clutch-node --version || exit 1

# Expose default port (configurable via environment)
EXPOSE 8081

# Set the entrypoint and default command
ENTRYPOINT ["clutch-node"]
CMD ["--env", "default"]
