# syntax=docker/dockerfile:1
FROM rust:1.99.1-slim-bookworm@sha256:f2c004e3d2d8b547e8cf7ffbc29205385f4e71d23e53e4e383065db5e9f822cd AS build
RUN apt-get update \
    && apt-get install --yes --no-install-recommends build-essential pkg-config \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app/apps
COPY apps/Cargo.toml apps/Cargo.lock ./
COPY apps/backend ./backend
COPY contracts/solana /app/contracts/solana

FROM build AS development-build
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/apps/target \
    cargo build --locked -p apodect-backend \
    && cp target/debug/apodect-backend target/debug/apodect-worker target/debug/apodect /usr/local/bin/

FROM build AS production-build
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/apps/target \
    cargo build --locked --release -p apodect-backend \
    && cp target/release/apodect-backend target/release/apodect-worker target/release/apodect /usr/local/bin/

FROM debian:bookworm-slim@sha256:734a72b3d6142a71993b4bc580ca01d8ca687f77a2c422d96a961d3376b179bc AS runtime
RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 1000 app \
    && useradd --uid 1000 --gid app --no-create-home app
USER app
WORKDIR /app
EXPOSE 3001
CMD ["apodect-backend"]

FROM runtime AS development
COPY --from=development-build /usr/local/bin/apodect* /usr/local/bin/

FROM runtime AS production
COPY --from=production-build /usr/local/bin/apodect* /usr/local/bin/
