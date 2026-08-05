#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
target_triple="riscv32imafc-unknown-none-elf"
audit_dir="$(mktemp -d)"
trap 'rm -rf -- "$audit_dir"' EXIT

cd "$repo_root"

tools/audit-driver-safety.sh

# Verify generated code from its canonical input instead of inspecting Rust
# source text for particular identifiers or function spellings.
cargo pac-gen --check

cargo build \
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

# The final artifact may refer to its source-only HAL/register dependencies and to
# compiler/core support only. Radio ROM or vendor archive symbols fail closed.
if rg -v \
    '^(_R.*open_esp_radio_esp32s31_(hal|registers|pac).*|_ZN.*open_esp_radio_esp32s31_(hal|registers|pac).*|_RNv.*core.*(panic.*|len_mismatch_fail.*)|_ZN.*core.*(panic.*|len_mismatch_fail.*)|__u?divdi3|mem(cmp|cpy|move|set))$' \
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
    cargo tree \
        -p open-esp-radio-esp32s31-phy \
        --target "$target_triple" \
        --prefix none
)"
if printf '%s\n' "$dependency_tree" |
    rg -v '^(open-esp-radio-dma|open-esp-radio-esp32s31-(phy|hal|registers|pac)|critical-section|vcell) v'
then
    echo "non-workspace dependency survived source-only build" >&2
    exit 1
fi

# Generated references and qualification harnesses are development oracles,
# never dependencies of a production crate. Audit each public production root
# separately so a future optional feature cannot smuggle a tool/HIL crate into
# the ordinary workspace graph while the PHY-only allowlist above still passes.
production_packages=(
    open-esp-radio
    open-esp-radio-dma
    open-esp-radio-embassy-net
    open-esp-radio-esp32s31-hal
    open-esp-radio-esp32s31-registers
    open-esp-radio-esp32s31-phy
    open-esp-radio-esp32s31-pac
    open-esp-radio-esp32s31-wifi-embassy
    open-esp-radio-esp32s31-wifi-dma
    open-esp-radio-esp32s31-wifi-esp-hal
    open-esp-radio-esp32s31-wifi-lmac
    open-esp-radio-esp32s31-wifi-sta
    open-esp-radio-ieee80211
    open-esp-radio-wifi-lmac
    open-esp-radio-wifi-sta
    open-esp-radio-wpa2
)
for package in "${production_packages[@]}"; do
    cargo tree \
        --package "$package" \
        --target "$target_triple" \
        --edges normal,build \
        --prefix none >"$audit_dir/dependencies-$package"
    if rg \
        '^(vendor-code-validator|open-radio-vendor-code-validator|open-esp-radio-(pac-gen|hil-runner|hil-(protocol|.*telemetry)|.*trace-probes|.*vendor-oracle))' \
        "$audit_dir/dependencies-$package"
    then
        echo "qualification dependency survived in production package $package" >&2
        exit 1
    fi
done

# Station policy and chip-specific station composition are deliberately below
# every executor/network adapter. Keep the rule executable as those crates
# absorb more code from the former monolithic Embassy composition.
for package in open-esp-radio-wifi-sta open-esp-radio-esp32s31-wifi-sta; do
    cargo tree \
        --package "$package" \
        --target "$target_triple" \
        --edges normal,build \
        --prefix none >"$audit_dir/dependencies-$package-layer"
    if rg \
        '^(embassy-|esp-hal |open-esp-radio-(embassy-net|esp32s31-wifi-embassy|hil-))' \
        "$audit_dir/dependencies-$package-layer"
    then
        echo "runtime or HIL dependency survived in station layer $package" >&2
        exit 1
    fi
done

if rg -n 'core::hint::spin_loop|spin_loop\(' driver --glob '*.rs'
then
    echo "production source contains a CPU spin loop" >&2
    exit 1
fi

# Production documentation names stable evidence IDs and public symbols. Local
# oracle paths, artifact digests and paths into an earlier private firmware
# tree are qualification policy and must stay under validation/HIL.
if rg -n '(?i)(_oracles/|sha-?256|[0-9a-f]{64}|esp32s31_rust/|firmware/esp32s31/)' \
    driver --glob '*.rs' --glob '*.md' --glob '*.toml'
then
    echo "qualification artifact identity survived in production source" >&2
    exit 1
fi

# Build the exact final image from the locked dependencies. A sibling esp-hal
# checkout is a useful HIL development override, but using it here would both
# mutate the embedded lockfile and make this policy gate machine-dependent.
env -u ESP_HAL_ROOT cargo hil build radio

runtime_elf="target/hil/esp32s31/psram-code-psram-data-open-radio-hil/cargo/runtime/$target_triple/release/open-esp-radio-hil-esp32s31-runtime"
test -f "$runtime_elf"

# The linker script exposes absolute ESP32-S31 ECO0 ROM symbols even when no
# call references them, so symbol-table matching is insufficient. Decode all
# executable sections and reject statically resolved jumps/calls into the
# pinned radio API table or the contiguous radio implementation body. System
# ROM outside these ranges (for example ets_printf) remains permitted.
cargo vendor-code-validator image audit-targets \
    --target-spec validation/esp32s31/target.spec \
    --artifact "$runtime_elf" \
    --forbid 'esp32s31-eco0-radio-api=0x2f800bf0..0x2f8016bc' \
    --forbid 'esp32s31-eco0-radio-body=0x2f823c12..0x2f83e6d0'

echo "source-only radio audit passed: rlib=$artifact runtime=$runtime_elf"
