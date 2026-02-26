# ── Build stage ────────────────────────────────────────────
FROM --platform=linux/amd64 ubuntu:24.04 AS builder

ENV DEBIAN_FRONTEND=noninteractive

# System deps for the server-side build + Rust prerequisites
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates curl build-essential git openssh-client \
    pkg-config libwebkit2gtk-4.1-dev libgtk-3-dev \
    libayatana-appindicator3-dev libxdo-dev libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Install Rust + wasm32 target
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y && \
    . "$HOME/.cargo/env" && \
    rustup target add wasm32-unknown-unknown
ENV PATH="/root/.cargo/bin:${PATH}"
RUN curl -L "https://github.com/DioxusLabs/dioxus/releases/download/v0.6.3/dx-x86_64-unknown-linux-gnu-v0.6.3.tar.gz" \
    | tar xz && mv dx /usr/local/bin/

WORKDIR /app
COPY . .

# Clone private deps via SSH, then build
RUN --mount=type=ssh \
    mkdir -p ~/.ssh && ssh-keyscan github.com >> ~/.ssh/known_hosts && \
    cargo fetch
RUN dx build --release --platform web --fullstack

# ── Runtime stage ─────────────────────────────────────────
FROM --platform=linux/amd64 ubuntu:24.04

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates libssl3t64 \
    fonts-dejavu-core fontconfig \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /app/target/dx/blueprint-app/release/web/server ./server
COPY --from=builder /app/target/dx/blueprint-app/release/web/public ./public

EXPOSE 8080

ENV IP=0.0.0.0
ENV PORT=8080

CMD ["./server"]
