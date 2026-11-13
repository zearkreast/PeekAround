# syntax=docker/dockerfile:1.7
# PeekAround — self-hosted gallery server: the Rust server (pa-cli) and the browser UI, the same
# Slint interface as the desktop app compiled to WebAssembly.
# Build:  docker build -f docker/Dockerfile -t peekaround .
# Run:    see docker/docker-compose.yml

# Override if Docker Hub rate-limits you, e.g. --build-arg RUST_IMAGE=mirror.gcr.io/library/rust:1-trixie
ARG RUST_IMAGE=rust:1-trixie
ARG DEBIAN_IMAGE=debian:trixie-slim

# ───────────── browser UI ─────────────
# WebAssembly is the same for every platform: built once, on the machine that runs the build
FROM --platform=$BUILDPLATFORM ${RUST_IMAGE} AS web
RUN rustup target add wasm32-unknown-unknown \
 && apt-get update \
 && apt-get install -y --no-install-recommends brotli \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/src/target,sharing=locked,id=peekaround-web \
    --mount=type=cache,target=/opt/tools,sharing=locked \
    CARGO_INSTALL_ROOT=/opt/tools PATH="/opt/tools/bin:$PATH" sh crates/pa-web/build.sh /out/web

# ───────────── server ─────────────
FROM ${RUST_IMAGE} AS server
ARG TARGETARCH
RUN apt-get update \
 && apt-get install -y --no-install-recommends pkg-config libheif-dev \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY . .
# after the browser UI (the mount waits for it) rather than beside it: the two builds at once
# need more memory than a small machine has
RUN --mount=type=bind,from=web,source=/out/web,target=/mnt/web \
    --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/src/target,sharing=locked,id=peekaround-server-${TARGETARCH} \
    cargo build --release --locked -p pa-cli \
 && mkdir -p /out && cp target/release/peekaround /out/peekaround

# ───────────── runtime ─────────────
FROM ${DEBIAN_IMAGE}
LABEL org.opencontainers.image.title="PeekAround" \
      org.opencontainers.image.description="Modern, animated gallery for your folders" \
      org.opencontainers.image.source="https://github.com/zearkreast/PeekAround" \
      org.opencontainers.image.licenses="GPL-3.0-only"
# ffmpeg: video thumbnails, transcodes, exports; libheif and its codecs: HEIC / AVIF
RUN apt-get update \
 && apt-get install -y --no-install-recommends ffmpeg tini gosu ca-certificates \
      libheif1 libheif-plugin-libde265 libheif-plugin-dav1d libheif-plugin-aomenc libheif-plugin-x265 \
 && rm -rf /var/lib/apt/lists/*
COPY --from=server /out/peekaround /usr/local/bin/peekaround
COPY --from=web /out/web /app/web
COPY LICENSE /app/LICENSE
COPY docker/entrypoint.sh /usr/local/bin/peekaround-entrypoint
RUN chmod +x /usr/local/bin/peekaround-entrypoint && mkdir -p /data /media

ENV PEEK_PORT=8080 \
    PEEK_DATA_DIR=/data \
    PEEK_ROOTS=/media \
    PEEK_SPLIT_ROOTS=true \
    PEEK_BROWSE_ROOTS=/media \
    PEEK_WEB_DIR=/app/web \
    PEEK_FFMPEG=ffmpeg \
    PEEK_FFPROBE=ffprobe \
    PUID=0 \
    PGID=0

VOLUME ["/data"]
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
  CMD ["peekaround", "--healthcheck"]
ENTRYPOINT ["/usr/bin/tini", "--", "peekaround-entrypoint"]
CMD ["peekaround"]
