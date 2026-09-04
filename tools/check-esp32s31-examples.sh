#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
target_triple="riscv32imafc-unknown-none-elf"

for example in \
    esp32s31-station \
    esp32s31-access-point \
    esp32s31-monitor \
    esp32s31-bluetooth-controller
do
    cargo check \
        --locked \
        --release \
        --target "$target_triple" \
        --manifest-path "$repo_root/examples/$example/Cargo.toml"
done

# The same application source must also compose through the released
# Embassy/Xarxa-compatible leaf. This profile is intentionally alternative to
# the default owned-network leaf rather than an additive Cargo feature.
cargo check \
    --locked \
    --release \
    --target "$target_triple" \
    --manifest-path "$repo_root/examples/esp32s31-station/Cargo.toml" \
    --no-default-features \
    --features compat-network
