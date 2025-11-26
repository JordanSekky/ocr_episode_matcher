FROM rust:bullseye AS builder

# Install build dependencies
# libtesseract-dev and libleptonica-dev are needed for tesseract-rs
# clang and llvm-dev might be needed for bindgen
RUN apt-get update && apt-get install -y \
    build-essential \
    pkg-config \
    libssl-dev \
    libtesseract-dev \
    libleptonica-dev \
    cmake \
    clang \
    curl \
    wget \
    git

WORKDIR /app
COPY . .

# Build the release binary
RUN cargo build --release

# Final stage
FROM linuxserver/ffmpeg:latest

# Install runtime dependencies
# - libtesseract-dev: strictly we only need the shared libs, but ensuring matching version with builder
# - less: for the subtitle pager
# - ffmpeg: already in base image
RUN apt-get update && apt-get install -y \
    libtesseract-dev \
    less \
    && rm -rf /var/lib/apt/lists/*

# Copy the binary
COPY --from=builder /app/target/release/episode-matcher /usr/local/bin/episode-matcher

# Set entrypoint to the application
ENTRYPOINT ["episode-matcher"]
CMD ["--help"]
