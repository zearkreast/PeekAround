# syntax=docker/dockerfile:1.7
# PeekAround — self-hosted gallery server
# Build:  docker build -f docker/Dockerfile -t peekaround .
# Run:    see docker/docker-compose.yml

ARG NODE_VERSION=24
# Override if Docker Hub rate-limits you, e.g. --build-arg NODE_IMAGE=mirror.gcr.io/library/node
ARG NODE_IMAGE=node

# ───────────── build ─────────────
FROM ${NODE_IMAGE}:${NODE_VERSION}-bookworm-slim AS build
WORKDIR /src
ENV ELECTRON_SKIP_BINARY_DOWNLOAD=1 \
    npm_config_fund=false \
    npm_config_audit=false
COPY package.json package-lock.json ./
COPY packages/shared/package.json packages/shared/
COPY packages/server/package.json packages/server/
COPY packages/web/package.json packages/web/
COPY packages/desktop/package.json packages/desktop/
COPY packages/android/package.json packages/android/
# (optional) behind a TLS-intercepting proxy: --secret id=ca,src=/path/to/ca.pem
RUN --mount=type=secret,id=ca,required=false \
    if [ -f /run/secrets/ca ]; then export NODE_EXTRA_CA_CERTS=/run/secrets/ca; fi; \
    npm ci --workspace @peekaround/shared --workspace @peekaround/server --workspace @peekaround/web --include-workspace-root
COPY tsconfig.base.json ./
COPY packages/shared packages/shared
COPY packages/server packages/server
COPY packages/web packages/web
RUN npm run build -w @peekaround/web && npm run build -w @peekaround/server
# a production-only package.json for the runtime image
RUN node -e "const p=require('./packages/server/package.json'); require('fs').writeFileSync('/src/runtime-package.json', JSON.stringify({name:'peekaround-server', version:p.version, type:'module', private:true, dependencies:p.dependencies}, null, 2))"

# ───────────── runtime ─────────────
FROM ${NODE_IMAGE}:${NODE_VERSION}-bookworm-slim
LABEL org.opencontainers.image.title="PeekAround" \
      org.opencontainers.image.description="Modern, animated gallery for your folders" \
      org.opencontainers.image.licenses="MIT"
RUN apt-get update \
 && apt-get install -y --no-install-recommends ffmpeg tini gosu ca-certificates \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=build /src/runtime-package.json ./package.json
RUN --mount=type=secret,id=ca,required=false \
    if [ -f /run/secrets/ca ]; then export NODE_EXTRA_CA_CERTS=/run/secrets/ca; fi; \
    npm install --omit=dev --no-audit --no-fund && npm cache clean --force
COPY --from=build /src/packages/server/dist ./dist
COPY --from=build /src/packages/web/dist ./web
COPY docker/entrypoint.sh /usr/local/bin/peekaround-entrypoint
RUN chmod +x /usr/local/bin/peekaround-entrypoint && mkdir -p /data /media

ENV NODE_ENV=production \
    NODE_OPTIONS=--disable-warning=ExperimentalWarning \
    PEEK_PORT=8080 \
    PEEK_DATA_DIR=/data \
    PEEK_ROOTS=/media \
    PEEK_SPLIT_ROOTS=true \
    PEEK_BROWSE_ROOTS=/media \
    PEEK_FFMPEG=ffmpeg \
    PEEK_FFPROBE=ffprobe \
    PUID=0 \
    PGID=0

VOLUME ["/data"]
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
  CMD node -e "fetch('http://127.0.0.1:'+(process.env.PEEK_PORT||8080)+'/api/info').then(r=>process.exit(r.ok?0:1)).catch(()=>process.exit(1))"
ENTRYPOINT ["/usr/bin/tini", "--", "peekaround-entrypoint"]
CMD ["node", "/app/dist/cli.js"]
