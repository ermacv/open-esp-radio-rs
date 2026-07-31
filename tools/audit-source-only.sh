#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
target_triple="riscv32imafc-unknown-none-elf"
audit_dir="$(mktemp -d)"
trap 'rm -rf -- "$audit_dir"' EXIT

cd "$repo_root"

# Verify generated code from its canonical input instead of inspecting Rust
# source text for particular identifiers or function spellings.
cargo pac-gen --check

RUSTUP_TOOLCHAIN=stable cargo build \
    -p open-esp-radio-esp32s31-phy \
    --lib \
    --release \
    --target "$target_triple"

artifact="$(
    find "target/$target_triple/release/deps" \
        -maxdepth 1 \
        -name 'libopen_esp_radio_esp32s31_phy-*.rlib' \
        -printf '%T@ %p\n' |
        sort -nr |
        head -n 1 |
        cut -d' ' -f2-
)"
test -n "$artifact"

llvm-nm --undefined-only --format=posix "$artifact" 2>/dev/null |
    awk '{print $1}' |
    sort -u >"$audit_dir/undefined"
llvm-nm --defined-only --format=posix "$artifact" 2>/dev/null |
    awk '{print $1}' |
    sort -u >"$audit_dir/defined"
comm -23 "$audit_dir/undefined" "$audit_dir/defined" >"$audit_dir/external"

# The final artifact may refer to its source-only HAL/PAC dependencies and to
# compiler/core support only. Radio ROM or vendor archive symbols fail closed.
if rg -v \
    '^(_R.*open_esp_radio_esp32s31_(hal|pac).*|_RNv.*core.*(panic.*|len_mismatch_fail.*)|__u?divdi3|mem(cmp|cpy|move|set))$' \
    "$audit_dir/external"
then
    echo "unexpected external symbol in source-only radio rlib" >&2
    exit 1
fi

if llvm-nm --format=posix "$artifact" 2>/dev/null |
    rg '(^| )(phy_wifi_get_tx_gain|register_chipv7_phy|g_phyFuns|phy_param|esp_wifi_|pp_|net80211_)($| )'
then
    echo "radio ROM/vendor ABI symbol survived source-only build" >&2
    exit 1
fi

dependency_tree="$(
    RUSTUP_TOOLCHAIN=stable cargo tree \
        -p open-esp-radio-esp32s31-phy \
        --target "$target_triple" \
        --prefix none
)"
if printf '%s\n' "$dependency_tree" |
    rg -v '^(open-esp-radio-esp32s31-(phy|hal|pac|svd)|vcell) v'
then
    echo "non-workspace dependency survived source-only build" >&2
    exit 1
fi

echo "source-only radio audit passed: $artifact"
