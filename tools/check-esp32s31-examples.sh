#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
target_triple="riscv32imafc-unknown-none-elf"

for example in esp32s31-station esp32s31-access-point esp32s31-monitor; do
    cargo check \
        --locked \
        --release \
        --target "$target_triple" \
        --manifest-path "$repo_root/examples/$example/Cargo.toml"
done
