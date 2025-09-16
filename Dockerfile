FROM rust:1.82-slim as builder

RUN apt-get update && apt-get install -y \
    pkg-config libssl-dev libpq-dev git ca-certificates \
    openssh-client build-essential uuid-dev wget libclang-dev libbsd-dev \
    && rm -rf /var/lib/apt/lists/*

# Install newer CMake (3.30+ required by rusteron crates)
RUN wget -O - https://apt.kitware.com/keys/kitware-archive-latest.asc 2>/dev/null | gpg --dearmor - | tee /etc/apt/trusted.gpg.d/kitware.gpg >/dev/null && \
    echo 'deb https://apt.kitware.com/ubuntu/ jammy main' > /etc/apt/sources.list.d/kitware.list && \
    apt-get update && apt-get install -y cmake && \
    rm -rf /var/lib/apt/lists/*

RUN rustup toolchain install nightly && rustup default nightly && rustup component add rustfmt --toolchain nightly

RUN git config --global url."git@github.com:".insteadOf "https://github.com/"

WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY crates/ ./crates/
RUN --mount=type=ssh cargo build --release --bin bin

FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
    libssl3 libpq5 curl ca-certificates libbsd0 \
    && rm -rf /var/lib/apt/lists/*

# Copy Aeron client libraries for the application
COPY aeron-binaries/lib/ /usr/local/lib/
RUN ldconfig

# Set library path for runtime
ENV LD_LIBRARY_PATH=/usr/local/lib

WORKDIR /app
COPY --from=builder /app/target/release/bin /app/vex-gateway

RUN useradd -r -s /bin/false appuser && mkdir -p /app/keys && chown -R appuser:appuser /app
USER appuser

EXPOSE 8080 8081
HEALTHCHECK --interval=30s --timeout=10s --start-period=5s --retries=3 CMD curl -f http://localhost:8080/health || exit 1
CMD ["./vex-gateway"]