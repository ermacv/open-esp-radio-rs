#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
target_triple="riscv32imafc-unknown-none-elf"
audit_dir="$(mktemp -d)"

cd "$repo_root"

# The SVD is the editable clock/power register source. Fail closed if the
# checked-in PAC was edited directly or generation is no longer reproducible.
tools/generate-esp32s31-radio-pac.py --check

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

# PHY target bindings may perform I2C/PBus work only through a borrowed
# RadioRegisters capability. Keep the removed raw-owner leaves and unsafe
# wrapper API from quietly returning during later calibration work.
if rg -n \
    'try_(start|finish)_(read|write)_unowned|try_(start|finish)_phy_pbus_force_test|pub[[:space:]]+unsafe[[:space:]]+fn[[:space:]]+(start_target|observe_target_edge|sample_target_once)' \
    crates/open-esp-radio-phy-esp32s31/src
then
    echo "unowned PHY-I2C/PBus target access returned" >&2
    exit 1
fi

# Complete PBus mode, AGC, antenna, RX-compensation, 11b and post-init leaves
# are PAC/HAL-owned. These addresses have no remaining live raw consumer.
if rg -n \
    '0x2010_(0884|088c|08bc|702c|7030|7044|7048|705c|7064|7068|7094|70a0|7104|7114|711c|7120|7124|7128|713c|78a4|78c8|7d4c|8004|8010|8018|801c|8020|8028|802c|8070|8078)' \
    crates/open-esp-radio-phy-esp32s31/src
then
    echo "raw PHY AGC/11b address escaped the PAC/HAL boundary" >&2
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

# HAL/PAC symbols are source-only workspace dependencies verified again by
# the dependency-tree gate below. The remaining entries are compiler/core
# requirements supplied by the final Rust image, not radio ROM or vendor
# archive ABI. Any other external symbol fails closed.
if rg -v \
    '^(_R.*open_esp_radio_(hal|pac)_esp32s31.*|_RNv.*core.*(panic.*|len_mismatch_fail.*)|__u?divdi3|mem(cmp|cpy|move|set))$' \
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
