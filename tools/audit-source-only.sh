#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
target_triple="riscv32imafc-unknown-none-elf"
audit_dir="$(mktemp -d)"

cd "$repo_root"

# HAL/MAC may select only PAC-described peripheral registers. Descriptor
# volatile access is intentionally excluded: those words live in owned DMA
# memory, not in the peripheral address space.
if rg -n \
    '0x(2010|2058|2070|2071|2080|2081)_[[:xdigit:]]{4}' \
    crates/open-esp-radio-hal-esp32s31/src \
    crates/open-esp-radio-mac-esp32s31/src
then
    echo "raw radio peripheral address escaped the PAC" >&2
    exit 1
fi

if rg -n \
    '(read_volatile|write_volatile|as \*(const|mut))' \
    crates/open-esp-radio-hal-esp32s31/src \
    crates/open-esp-radio-mac-esp32s31/src/registers.rs
then
    echo "raw MMIO access escaped the PAC" >&2
    exit 1
fi

RUSTUP_TOOLCHAIN=stable cargo build \
    -p open-esp-radio-phy-esp32s31 \
    --lib \
    --release \
    --target "$target_triple"

artifact="$(
    find "target/$target_triple/release/deps" \
        -maxdepth 1 \
        -name 'libopen_esp_radio_phy_esp32s31-*.rlib' \
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

# These are compiler/core requirements supplied by the final Rust image, not
# radio ROM or vendor archive ABI. Any other external symbol fails closed.
if rg -v \
    '^(_RNv.*core.*(panic.*|len_mismatch_fail.*)|__u?divdi3|mem(cmp|cpy|move|set))$' \
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
        -p open-esp-radio-phy-esp32s31 \
        --target "$target_triple" \
        --prefix none
)"
if printf '%s\n' "$dependency_tree" |
    rg -v '^(open-esp-radio-(phy|hal|pac)-esp32s31) v'
then
    echo "non-workspace dependency survived source-only build" >&2
    exit 1
fi

echo "source-only radio audit passed: $artifact"
