# syntax=docker/dockerfile:1
# The official NEAR sandbox requires glibc >= 2.39; keep both stages on Trixie.
FROM rust:1.99.0-trixie@sha256:81015672d6939b0265f09adf96d5de125482bb1330106076956b0518e74edc90 AS build

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates curl bzip2 xz-utils python3 python3-venv \
    clang libclang-dev cmake pkg-config libssl-dev libudev-dev protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*
RUN rustup target add wasm32-unknown-unknown

# Official release archives are pinned by version and SHA-256.
RUN curl -fsSL https://github.com/foundry-rs/foundry/releases/download/v1.7.1/foundry_v1.7.1_linux_amd64.tar.gz -o /tmp/foundry.tar.gz \
    && echo 'cf7e688ed0c4c48adffca788b496076e31060b67ac5afe1e43dbb5499c20c88b  /tmp/foundry.tar.gz' | sha256sum -c - \
    && tar -xzf /tmp/foundry.tar.gz -C /usr/local/bin \
    && rm /tmp/foundry.tar.gz
RUN curl -fsSL https://github.com/argotorg/solidity/releases/download/v0.8.33/solc-static-linux -o /usr/local/bin/solc \
    && echo '1274e5c4621ae478090c5a1f48466fd3c5f658ed9e14b15a0b213dc806215468  /usr/local/bin/solc' | sha256sum -c - \
    && chmod 0755 /usr/local/bin/solc
RUN curl -fsSL https://github.com/anza-xyz/agave/releases/download/v4.0.3/solana-release-x86_64-unknown-linux-gnu.tar.bz2 -o /tmp/solana.tar.bz2 \
    && echo '50a6ed0474c958e1ce3fba298f45fc1cd3117db5c5df253cc0c7d8c827e813a8  /tmp/solana.tar.bz2' | sha256sum -c - \
    && tar -xjf /tmp/solana.tar.bz2 -C /opt \
    && rm /tmp/solana.tar.bz2
RUN curl -fsSL https://github.com/anza-xyz/platform-tools/releases/download/v1.53/platform-tools-linux-x86_64.tar.bz2 -o /tmp/platform-tools.tar.bz2 \
    && echo '876b5c294a38d41d40bed4592911091e92cc3f399dd17a0f9cac87b1df9ab120  /tmp/platform-tools.tar.bz2' | sha256sum -c - \
    && mkdir -p /root/.cache/solana/v1.53/platform-tools \
    && tar -xjf /tmp/platform-tools.tar.bz2 -C /root/.cache/solana/v1.53/platform-tools \
    && rm /tmp/platform-tools.tar.bz2
RUN curl -fsSL https://github.com/astral-sh/uv/releases/download/0.11.21/uv-x86_64-unknown-linux-gnu.tar.gz -o /tmp/uv.tar.gz \
    && echo '8c88519b0ef0af9801fcdee419bbb12116bd9e6b18e162ae093c932d8b264050  /tmp/uv.tar.gz' | sha256sum -c - \
    && tar -xzf /tmp/uv.tar.gz -C /usr/local/bin --strip-components=1 \
    && rm /tmp/uv.tar.gz
RUN curl -fsSL https://s3-us-west-1.amazonaws.com/build.nearprotocol.com/nearcore/Linux-x86_64/2.14.0/near-sandbox.tar.gz -o /tmp/near.tar.gz \
    && echo 'a0ba004218b218bd74351795e7ce2eda4a2b6af5cd3e252bd5e8bfe3c8f2e691  /tmp/near.tar.gz' | sha256sum -c - \
    && tar -xzf /tmp/near.tar.gz -C /usr/local/bin --strip-components=1 \
    && rm /tmp/near.tar.gz

ENV PATH="/opt/solana-release/bin:${PATH}" \
    FOUNDRY_SOLC=/usr/local/bin/solc \
    NEAR_SANDBOX_BIN_PATH=/usr/local/bin/near-sandbox \
    UV_PROJECT_ENVIRONMENT=/opt/apodect/venv \
    CARGO_BUILD_JOBS=2
RUN anvil --version && solc --version \
    && solana-test-validator --version && cargo-build-sbf --version \
    && near-sandbox --version
WORKDIR /app
COPY scripts/crosschain/pyproject.toml scripts/crosschain/uv.lock scripts/crosschain/
RUN uv sync --project scripts/crosschain --locked --no-dev --python /usr/bin/python3
COPY contracts/ethereum contracts/ethereum
RUN cd contracts/ethereum \
    && forge soldeer install --config-location soldeer \
    && forge build script/LocalUSDC.s.sol src/WorkspaceRegistry.sol
COPY contracts/solana contracts/solana
RUN --mount=type=cache,id=apodect-cargo-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=apodect-solana-build,target=/cache/solana \
    mkdir -p /opt/apodect/bin \
    && cd contracts/solana \
    && CARGO_TARGET_DIR=/cache/solana/sbf cargo build-sbf --tools-version v1.53 --sbf-out-dir /opt/apodect -- --locked \
    && CARGO_TARGET_DIR=/cache/solana/host cargo build --locked --features no-entrypoint --example localnet \
    && cp /cache/solana/host/debug/examples/localnet /opt/apodect/bin/solana-client
COPY contracts/near contracts/near
RUN --mount=type=cache,id=apodect-cargo-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=apodect-near-build,target=/cache/near \
    cd contracts/near \
    && CARGO_TARGET_DIR=/cache/near ./scripts/build.sh \
    && CARGO_TARGET_DIR=/cache/near cargo build --locked -p apodect-near-localnet \
    && cp /cache/near/debug/apodect-near-localnet /opt/apodect/bin/near-client

FROM debian:trixie-slim@sha256:a72b4433de1cf7d6bae0c4cee74d0547f58601a643b3375f070c51600d63b6b2
RUN apt-get update && apt-get install -y --no-install-recommends \
    bash ca-certificates curl python3 libssl3t64 libudev1 libatomic1 libstdc++6 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --uid 1000 localnet \
    && mkdir -p /app/.localnet \
    && chown localnet:localnet /app/.localnet
COPY --from=build /usr/local/bin/anvil /usr/local/bin/cast /usr/local/bin/forge /usr/local/bin/near-sandbox /usr/local/bin/
COPY --from=build /opt/solana-release/bin/solana-test-validator /usr/local/bin/
COPY --from=build /opt/apodect /opt/apodect
WORKDIR /app
COPY --chown=localnet:localnet contracts/ethereum/scripts contracts/ethereum/scripts
COPY --from=build --chown=localnet:localnet /app/contracts/ethereum/out contracts/ethereum/out
COPY --chown=localnet:localnet contracts/solana/scripts/localnet.sh contracts/solana/scripts/localnet.sh
COPY --chown=localnet:localnet contracts/near/scripts/localnet.sh contracts/near/scripts/toolchain.sh contracts/near/scripts/
COPY --from=build --chown=localnet:localnet /app/contracts/near/artifacts contracts/near/artifacts
COPY --chown=localnet:localnet scripts/localnet.py scripts/localnet.sh scripts/
COPY --chown=localnet:localnet scripts/crosschain scripts/crosschain
ENV APODECT_CONTAINER=1 \
    NEAR_SANDBOX_BIN_PATH=/usr/local/bin/near-sandbox \
    PYTHONDONTWRITEBYTECODE=1 \
    PYTHONUNBUFFERED=1
USER localnet
RUN anvil --version && cast --version && solana-test-validator --version \
    && near-sandbox --version && ./scripts/localnet.sh --help \
    && test -r contracts/near/artifacts/apodect_near_project.wasm \
    && test -r /opt/apodect/apodect_solana.so
CMD ["./scripts/localnet.sh", "serve"]
