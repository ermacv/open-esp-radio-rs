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
    'try_(start|finish)_(read|write)_unowned|try_(start|finish)_phy_pbus_force_test|pub[[:space:]]+unsafe[[:space:]]+fn[[:space:]]+(start_target|observe_target_edge|sample_target_once)|start_phy_channel_frequency_switch|configure_phy_(channel_nrx_frequency|nrx_frequency|frequency_registers|frequency_i2c_number_addresses|bt_filter|bb_tx_power_tracking|i2c_tx_rate|power_detector_registers|tx_power_control_background|power_detector_enabled|power_detector_calibration_mode|txdc_pwdet_registers|txdc_pwdet_sar|baseband_watchdog|noise_floor_auto|dc_iq_estimator)|write_phy_frequency_memory|set_phy_(baseband_mode|wifi_enabled|dc_iq_estimator_enable)|enable_phy_(mac_baseband|iq_correction)|restore_phy_txdc_pwdet_registers|write_phy_power_detector_reference_control|trigger_phy_power_detector_sar|sample_phy_dc_iq_readiness|read_phy_(power_detector_(ready_status|sar_word)|dc_iq_accumulators|rxiq_total_power|rxiq_mismatch_accumulators|signal_power_accumulators)' \
    crates/open-esp-radio-phy-esp32s31/src
then
    echo "unowned PHY-I2C/PBus target access returned" >&2
    exit 1
fi

# Complete frequency/channel, PBus mode, AGC, antenna, RX-compensation,
# DC-memory, BBPLL, 11b and post-init leaves are PAC/HAL-owned. These
# addresses have no remaining live raw consumer.
if rg -n \
    '0x(2010_(001c|0024|0028|002c|0030|0034|0038|003c|044c|0450|0454|0458|045c|0460|0464|0468|046c|047c|0808|080c|0810|0814|0818|081c|0870|0874|0884|088c|08bc|08d0|4400|448c|7018|702c|7030|703c|7044|7048|705c|7064|7068|7094|70a0|7104|7114|711c|7120|7124|7128|713c|7400|7428|743c|7454|7458|745c|7460|7808|7848|7890|78a4|78c8|78dc|78e4|790c|7980|7a28|7c00|7c30|7c3c|7c40|7c44|7c50|7c6c|7ca8|7cd0|7ce0|7ce4|7d4c|8004|8010|8018|801c|8020|8028|802c|8070|8078|9c18|f818|fc04)|2070_1068)' \
    crates/open-esp-radio-phy-esp32s31/src
then
    echo "raw recovered PHY address escaped the PAC/HAL boundary" >&2
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
