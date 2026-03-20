# ── Build stage ────────────────────────────────────────────
FROM --platform=linux/amd64 ubuntu:24.04 AS builder

ENV DEBIAN_FRONTEND=noninteractive

# System deps for Rust + wasm build
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates curl build-essential git \
    pkg-config libssl-dev \
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

RUN cargo fetch
RUN dx build --release --platform web --package blueprint-app

# ── Runtime stage ─────────────────────────────────────────
FROM --platform=linux/amd64 nginx:alpine

COPY --from=builder /app/target/dx/blueprint-app/release/web/public /usr/share/nginx/html

# SPA fallback: serve index.html for all routes
RUN printf 'server {\n\
    listen 8080;\n\
    root /usr/share/nginx/html;\n\
    index index.html;\n\
    location / {\n\
        try_files $uri $uri/ /index.html;\n\
    }\n\
}\n' > /etc/nginx/conf.d/default.conf

EXPOSE 8080

CMD ["nginx", "-g", "daemon off;"]
