# Multi-stage build for optimized image size
# Build arguments for flexibility
ARG RUST_VERSION=1.89

#==============================================================================
# Builder Stage - Use Debian Bookworm for reliable apt mirrors (avoids trixie)
#==============================================================================
FROM rust:${RUST_VERSION}-slim-bookworm AS builder

# Configure apt: retries, long timeout, use regional mirror (ftp.us.debian.org)
RUN echo 'Acquire::Retries "5"; Acquire::http::Timeout "300";' > /etc/apt/apt.conf.d/80-retries && \
    rm -f /etc/apt/sources.list.d/*.list 2>/dev/null || true && \
    echo 'deb http://ftp.us.debian.org/debian bookworm main' > /etc/apt/sources.list && \
    echo 'deb http://ftp.us.debian.org/debian bookworm-updates main' >> /etc/apt/sources.list && \
    echo 'deb http://security.debian.org/debian-security bookworm-security main' >> /etc/apt/sources.list

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

# Use the stable toolchain provided by the base image
# (no rustup toolchain install; we rely on rust:${RUST_VERSION}-slim-bookworm)

# Create app user for security (UID 999 to match runtime stage)
RUN groupadd -g 999 clutch && \
    useradd -r -u 999 -g clutch -s /bin/sh clutch

WORKDIR /usr/src/clutch-node

# Copy dependency files for better caching
COPY Cargo.toml Cargo.lock ./

# Create dummy source and build dependencies
RUN mkdir src && \
    echo "fn main() {}" > src/main.rs && \
    cargo build --release && \
    rm -rf src

# Copy actual source code
COPY src ./src

# Build the final binary with stable cargo
RUN cargo build --release --bin clutch-node

# Strip the binary to reduce size further
RUN strip target/release/clutch-node

#==============================================================================
# Runtime Stage - Minimal Debian image (matching builder GLIBC version)
#==============================================================================
FROM debian:bookworm-slim

# Configure apt: retries, 5min timeout for slow networks
RUN echo 'Acquire::Retries "5"; Acquire::http::Timeout "300";' > /etc/apt/apt.conf.d/80-retries

# Use regional mirror (ftp.us.debian.org) - often more reliable in Docker
RUN rm -f /etc/apt/sources.list.d/*.list 2>/dev/null || true && \
    echo 'deb http://ftp.us.debian.org/debian bookworm main' > /etc/apt/sources.list && \
    echo 'deb http://ftp.us.debian.org/debian bookworm-updates main' >> /etc/apt/sources.list && \
    echo 'deb http://security.debian.org/debian-security bookworm-security main' >> /etc/apt/sources.list

# Install only essential runtime dependencies (skip upgrade to reduce fetch)
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
    ca-certificates \
    tzdata \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user (UID 999 to avoid SYS_UID_MAX warning)
RUN groupadd -g 999 clutch && \
    useradd -r -u 999 -g clutch -s /bin/sh clutch

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
