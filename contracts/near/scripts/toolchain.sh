#!/usr/bin/env bash

# rustup proxies are not necessarily first on PATH (notably on NixOS).
if command -v rustup >/dev/null 2>&1; then
    near_rust_bin="$(dirname -- "$(rustup which --toolchain 1.99.0 rustc)")"
    export PATH="$near_rust_bin:$PATH"
fi
