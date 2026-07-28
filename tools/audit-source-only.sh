#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
target_triple="riscv32imafc-unknown-none-elf"
audit_dir="$(mktemp -d)"

cd "$repo_root"

# The SVD is the editable clock/power/register source. Fail closed if the
# checked-in svd2rust PAC was edited directly or generation is no longer
# reproducible with the pinned Rust generator dependency.
cargo pac-gen --check

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

# The generated register singleton is owned only by the compatibility PAC.
# HAL/MAC/PHY receive the safe `RadioRegisters` capability and must not split
# it into independently stolen svd2rust peripheral blocks.
if rg -n \
    '(open_esp_radio_svd|::svd::|svd::Peripherals|RadioRegisters::steal)' \
    crates/open-esp-radio-mac-esp32s31/src \
    crates/open-esp-radio-phy-esp32s31/src
then
    echo "generated PAC ownership escaped RadioRegisters" >&2
    exit 1
fi

# These native generated-PAC slices have completed their target migration.
# Their HAL modules may sequence semantic operations but must not regress to
# the Register32 compatibility facade.
if rg -n \
    '(Register32|Field32|read32|write32|modify32|power::(phy_i2c|phy_pbus))' \
    crates/open-esp-radio-hal-esp32s31/src/phy_i2c.rs \
    crates/open-esp-radio-hal-esp32s31/src/pbus.rs \
    crates/open-esp-radio-hal-esp32s31/src/phy_agc.rs \
    crates/open-esp-radio-hal-esp32s31/src/phy_iq_estimator.rs \
    crates/open-esp-radio-hal-esp32s31/src/phy_baseband.rs \
    crates/open-esp-radio-hal-esp32s31/src/phy_frequency.rs \
    crates/open-esp-radio-hal-esp32s31/src/phy_memory.rs \
    crates/open-esp-radio-hal-esp32s31/src/phy_prelude.rs \
    crates/open-esp-radio-hal-esp32s31/src/phy_power_detector.rs \
    crates/open-esp-radio-hal-esp32s31/src/phy_rx_dco.rs
then
    echo "native PHY-I2C/PBus/prelude/AGC/IQ/baseband/frequency/memory/PWDET/RX-DCO compatibility MMIO returned to the HAL" >&2
    exit 1
fi

# RX and TX BlockAck hardware leaves are now direct generated-PAC methods.
# The MAC protocol modules retain validation and decoding but must not regain
# compatibility-register identities or generic raw register operations.
if rg -n \
    '(Register32|Field32|read32|write32|modify32|mac::rx_block_ack|TX_BLOCK_ACK_(CONTROL_SEQUENCE|BITMAP_(LOW|HIGH)))' \
    crates/open-esp-radio-mac-esp32s31/src/rx_ampdu_hw.rs \
    crates/open-esp-radio-mac-esp32s31/src/tx_ampdu.rs
then
    echo "BlockAck compatibility MMIO returned to the MAC protocol layer" >&2
    exit 1
fi

# The live RX ring receives only the semantic descriptor-walker capability.
# Raw register identities remain available to diagnostic HIL code, but may
# not re-enter the ownership and recycling implementation.
if rg -n \
    '(Register32|Field32|\bMmio\b|read32|write32|modify32)' \
    crates/open-esp-radio-mac-esp32s31/src/rx.rs
then
    echo "RX descriptor-walker compatibility MMIO returned to the live ring" >&2
    exit 1
fi

# PHY target bindings may perform I2C/PBus work only through a borrowed
# RadioRegisters capability. Keep the removed raw-owner leaves and unsafe
# wrapper API from quietly returning during later calibration work.
if rg -n \
    'try_(start|finish)_(read|write)_unowned|try_(start|finish)_phy_pbus_force_test|pub[[:space:]]+unsafe[[:space:]]+fn[[:space:]]+(start_target|observe_target_edge|sample_target_once)|start_phy_channel_frequency_switch|configure_phy_(channel_nrx_frequency|nrx_frequency|frequency_registers|frequency_i2c_number_addresses|bt_filter|bb_tx_power_tracking|i2c_tx_rate|power_detector_registers|tx_power_control_background|power_detector_enabled|power_detector_calibration_mode|txdc_pwdet_registers|txdc_pwdet_sar|baseband_watchdog|noise_floor_auto|dc_iq_estimator|temperature_sensor_read)|write_phy_frequency_memory|set_phy_(baseband_mode|wifi_enabled|dc_iq_estimator_enable)|enable_phy_(mac_baseband|iq_correction)|restore_phy_txdc_pwdet_registers|write_phy_power_detector_reference_control|trigger_phy_power_detector_sar|sample_phy_dc_iq_readiness|read_phy_(power_detector_(ready_status|sar_word)|dc_iq_accumulators|rxiq_total_power|rxiq_mismatch_accumulators|signal_power_accumulators|temperature_code)' \
    crates/open-esp-radio-phy-esp32s31/src
then
    echo "unowned PHY-I2C/PBus target access returned" >&2
    exit 1
fi

if rg -n \
    'capture_and_clear_phy_register_field|restore_phy_register_field|mask_phy_rx_dco_control_field|restore_phy_rx_dco_control_field|read_phy_pbus_(field|rx_dco_value)' \
    crates/open-esp-radio-phy-esp32s31/src
then
    echo "raw RX-DCO/PBus owner access returned" >&2
    exit 1
fi

if rg -n \
    'configure_phy_register_force_txrx|sample_phy_i2c_master_reset|pulse_phy_i2c_master_reset|phy_i2c_master_reset_busy|configure_phy_register_xtal_frequency|read_phy_sdm_cycle_counter|configure_phy_tx_clock|configure_phy_rx_clock|configure_phy_rxiq_root_status|configure_phy_rxiq_root_correction|with_phy_tx_clock|with_phy_rx_clock|with_phy_rxiq_root_correction_begin|with_phy_rxiq_root_aux_begin' \
    crates/open-esp-radio-phy-esp32s31/src
then
    echo "raw PHY prelude/deadline owner access returned" >&2
    exit 1
fi

# RX-gain DC calibration is now a generated-PAC operation. Reject both the
# physical literal and the former address arithmetic used to hide it behind
# the adjacent tone-selector identity.
if rg -n \
    '0x2010_0424|PHY_TONE_SELECTOR_CONTROL_ADDRESS[[:space:]]*-[[:space:]]*4|unsafe[[:space:]]+fn[[:space:]]+configure_phy_rx_gain_dc_registers' \
    crates/open-esp-radio-phy-esp32s31/src
then
    echo "raw RX-gain DC calibration access returned" >&2
    exit 1
fi

if rg -n \
    'unsafe[[:space:]]+fn[[:space:]]+configure_phy_(adc_rate|front_end_registers|front_end_update)' \
    crates/open-esp-radio-phy-esp32s31/src
then
    echo "raw ADC/front-end owner access returned" >&2
    exit 1
fi

# TX-DC actions now carry booleans rather than a raw address/mask/register
# image and all access is serialized by the generated PAC owner.
if rg -n \
    '0x2010_0418|PHY_TX_DC_READY_(ADDRESS|MASK|VALUE)|unsafe[[:space:]]+fn[[:space:]]+(trigger_phy_tx_dc_measurement|read_phy_tx_dc_(ready|comparator)_status|clear_phy_tx_dc_measurement)' \
    crates/open-esp-radio-phy-esp32s31/src
then
    echo "raw TX-DC measurement access returned" >&2
    exit 1
fi

# IQ mode and coefficient access now uses the existing generated PAC fields.
if rg -n \
    '0x2010_(0438|0c0c)|PHY_(TXIQ_CONTROL|RXIQ_CORRECTION|RXIQ_AUX)_ADDRESS|unsafe[[:space:]]+fn[[:space:]]+configure_phy_(txiq_correction|txiq_coefficient|rxiq_coefficient|rxiq_calibration_mode)' \
    crates/open-esp-radio-phy-esp32s31/src
then
    echo "raw IQ correction/coefficient access returned" >&2
    exit 1
fi

# Calibration-tone, DAC-scale and TX-gain-compensation MMIO is native PAC
# state. Reject every former physical identity and wrapper that could bypass
# the unique register owner.
if rg -n \
    '0x2010_(040c|0410|0414|041c|0420|0428|0c04)|PHY_(TONE_PATH[01]_CONTROL|TONE_STOP_CONTROL|TONE_SELECTOR_CONTROL|TX_GAIN_COMPENSATION_(CONTROL|AUX)|DAC_SCALE_CONTROL)_ADDRESS|unsafe[[:space:]]+fn[[:space:]]+(configure_phy_(calibration_tone|power_control_tone|calibration_tone_wide|txiq_mis_power)|read_phy_txiq_tone_control|restore_phy_txiq_tone_control|arm_phy_power_detector_tone|clear_phy_power_detector_tone_arm|stop_phy_power_detector_tone)' \
    crates/open-esp-radio-phy-esp32s31/src
then
    echo "raw calibration-tone access returned" >&2
    exit 1
fi

# Every live PHY target operation is now ownership-bound and safe. Peripheral
# writer unsafety belongs only inside the PAC, while DMA pointer unsafety
# remains a separate MAC concern.
if rg -n \
    '\bunsafe\b|read_volatile|write_volatile|as \*(const|mut)' \
    crates/open-esp-radio-phy-esp32s31/src
then
    echo "unsafe operation returned to the upper PHY crate" >&2
    exit 1
fi

# Complete frequency/channel, PBus mode, AGC, antenna, RX-compensation,
# DC-memory, BBPLL, 11b and post-init leaves are PAC/HAL-owned. These
# addresses have no remaining live raw consumer.
if rg -n \
    '0x(2010_(001c|0024|0028|002c|0030|0034|0038|003c|0434|0444|0448|044c|0450|0454|0458|045c|0460|0464|0468|046c|047c|0808|080c|0810|0814|0818|081c|086c|0870|0874|0884|088c|0890|0894|08bc|08d0|0c08|0c20|4400|448c|7018|702c|7030|703c|7044|7048|705c|7064|7068|7094|70a0|7104|7114|711c|7120|7124|7128|713c|7400|7428|743c|7454|7458|745c|7460|7808|7848|7890|78a4|78c8|78dc|78e4|790c|7980|7a28|7c00|7c30|7c3c|7c40|7c44|7c50|7c6c|7ca8|7cd0|7ce0|7ce4|7d4c|8004|8010|8018|801c|8020|8028|802c|8070|8078|9c18|d800|f028|f800|f804|f818|fc04)|2070_1068|2071_0030|2081_(8000|8018))' \
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
    rg -v '^(open-esp-radio-(phy|hal|pac|svd)-esp32s31|vcell) v'
then
    echo "non-workspace dependency survived source-only build" >&2
    exit 1
fi

echo "source-only radio audit passed: $artifact"
