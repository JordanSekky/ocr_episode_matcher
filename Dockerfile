# Build stage
FROM rust:1-trixie AS builder

# Install build dependencies for tesseract-rs
# tesseract-rs requires clang and leptonica/tesseract development headers
RUN apt-get update && apt-get install -y \
    clang \
    libclang-dev \
    libleptonica-dev \
    cmake \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /usr/src/app
COPY . .

# Build the application
RUN cargo build --release

# Runtime stage
FROM debian:trixie-slim

# Install runtime dependencies:
# - ffmpeg: for frame and subtitle extraction
# - ca-certificates: for HTTPS requests (TVDB API)
# - less: as the default pager
RUN apt-get update && apt-get install -y \
    ffmpeg \
    ca-certificates \
    less \
    && rm -rf /var/lib/apt/lists/*

# Copy the binary from builder
COPY --from=builder /usr/src/app/target/release/episode-matcher /usr/local/bin/episode-matcher

ENTRYPOINT ["episode-matcher"]
