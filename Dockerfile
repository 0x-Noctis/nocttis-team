# syntax=docker/dockerfile:1
# Image produksi Noctis Team: satu proses Axum yang melayani API dan WebApp statis.

# ---- 1. WebApp (SvelteKit adapter-static) ----
FROM node:22-bookworm-slim AS web
WORKDIR /web
COPY web/package.json web/package-lock.json ./
RUN npm ci --no-audit --no-fund
COPY web/ ./
RUN npm run build

# ---- 2. Backend Rust ----
FROM rust:1-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
# Cache dependensi: bangun dengan sumber kosong dulu supaya layer ini hanya berubah bila Cargo.* berubah.
RUN mkdir src && echo 'fn main() {}' > src/main.rs && : > src/lib.rs \
 && cargo build --release --locked --bin ai-team
COPY migrations ./migrations
COPY src ./src
RUN touch src/main.rs src/lib.rs && cargo build --release --locked --bin ai-team

# ---- 3. Runtime ----
FROM debian:bookworm-slim AS runtime
# git: worktree dan integrasi. docker CLI: runner verifikasi menjalankan container lewat daemon host (opsional, lihat
# docs/deployment.md; tanpa socket Docker, task tidak dapat diverifikasi tetapi API/UI tetap berjalan).
RUN apt-get update \
 && apt-get install -y --no-install-recommends git ca-certificates \
 && apt-get clean
COPY --from=docker:27-cli /usr/local/bin/docker /usr/local/bin/docker
RUN groupadd --system --gid 10001 noctis \
 && useradd --system --uid 10001 --gid noctis --no-create-home --home-dir /tmp --shell /usr/sbin/nologin noctis \
 && mkdir -p /data && chown noctis:noctis /data
COPY --from=build /src/target/release/ai-team /usr/local/bin/ai-team
COPY --from=web /web/build /app/web

# Di dalam container server mendengarkan semua antarmuka; pembatasan ke localhost dilakukan saat port DIPUBLIKASIKAN
# (compose.yaml: 127.0.0.1:7410:7410). Jangan publikasikan ke 0.0.0.0 tanpa proxy yang menambah autentikasi.
ENV NOCTIS__SERVER__BIND=0.0.0.0:7410 \
    NOCTIS_ALLOW_NON_LOOPBACK=1 \
    NOCTIS__SERVER__WEB_ROOT=/app/web \
    NOCTIS__GIT__WORKTREE_ROOT=/data/worktrees \
    NOCTIS__ARTIFACTS__ROOT=/data/artifacts \
    HOME=/tmp \
    RUST_LOG=info
USER 10001:10001
WORKDIR /data
VOLUME /data
EXPOSE 7410
HEALTHCHECK --interval=15s --timeout=5s --start-period=30s --retries=3 CMD ["ai-team", "healthcheck"]
ENTRYPOINT ["ai-team"]
