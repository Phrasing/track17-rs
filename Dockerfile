# ============================================
# Stage 1: Builder - Compile Rust + V8
# ============================================
FROM rust:1.92-bookworm AS builder

# Install build dependencies for V8 and OpenSSL
RUN apt-get update && apt-get install -y \
    build-essential \
    cmake \
    pkg-config \
    libssl-dev \
    libclang-dev \
    git \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Copy dependency manifests first for layer caching
COPY Cargo.toml Cargo.lock ./

# Create dummy source to cache dependencies
RUN mkdir src && \
    echo "fn main() {}" > src/main.rs && \
    mkdir -p src/bin && \
    echo "fn main() {}" > src/bin/server.rs

# Build dependencies only (this layer will be cached)
RUN cargo build --release --bin server && \
    rm -rf src target/release/deps/track17*

# Copy actual source code
COPY src ./src

# Build the actual application
RUN cargo build --release --bin server

# Strip debug symbols to reduce binary size
RUN strip /build/target/release/server

# ============================================
# Stage 2: Runtime - Minimal production image
# ============================================
FROM debian:bookworm-slim

# Install runtime dependencies (curl for health checks)
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user
RUN useradd -m -u 1000 -s /bin/bash appuser

# Copy binary from builder
COPY --from=builder /build/target/release/server /usr/local/bin/server

# Set ownership
RUN chown appuser:appuser /usr/local/bin/server

# Switch to non-root user
USER appuser

# Environment defaults
ENV PORT=3000 \
    RUST_LOG=info

# Expose port
EXPOSE 3000

# Health check using the /health endpoint
HEALTHCHECK --interval=30s --timeout=3s --start-period=10s --retries=3 \
    CMD curl -f http://localhost:3000/health || exit 1

# Run the server
CMD ["/usr/local/bin/server"]
