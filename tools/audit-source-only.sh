#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
target_triple="riscv32imafc-unknown-none-elf"
audit_dir="$(mktemp -d)"
trap 'rm -rf -- "$audit_dir"' EXIT

cd "$repo_root"

# Every tracked Cargo package must resolve through a checked-in lockfile at its
# actual workspace boundary. This also covers the independently buildable HIL,
# example, product-integration, probe, and oracle-firmware workspaces without
# reading local or private oracle inputs.
tools/audit-cargo-metadata.sh

# Research resumes only from a warning-free workspace. Keep this fail-closed:
# adding a new target or crate automatically subjects it to the same budget.
cargo clippy --workspace --all-targets -- -D warnings

tools/audit-driver-safety.sh
tools/audit-driver-architecture.sh

# Verify generated code from its canonical input instead of inspecting Rust
# source text for particular identifiers or function spellings.
cargo blobray project configure \
    --project verification/vendor/targets/esp32s31/vendor-project.toml \
    --check

# Publication also validates local analysis products and therefore belongs to
# artifact-backed CI.  The source-only gate checks each public register product
# directly so a clean checkout never needs generated/findings or local.toml.
for register_command in \
    validate \
    export-svd \
    generate-pac-raw \
    generate-bindings
do
    arguments=(registers "$register_command")
    if [[ "$register_command" != validate ]]; then
        arguments+=(--check)
    fi
    cargo blobray "${arguments[@]}" \
        --project verification/vendor/targets/esp32s31/vendor-project.toml
done

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
# compiler/core support only. Public pure-model `Debug` implementations retain
# `core::fmt` leaves in an rlib even when the final image does not call them.
# Radio ROM or vendor archive symbols still fail closed.
if rg -v \
    '^(_R.*open_esp_radio_esp32s31_(hal|registers|pac).*|_ZN.*open_esp_radio_esp32s31_(hal|registers|pac).*|_R.*4core3fmt.*|_ZN.*4core3fmt.*|_RNv.*core.*(panic.*|len_mismatch_fail.*)|_ZN.*core.*(panic.*|len_mismatch_fail.*)|__u?divdi3|mem(cmp|cpy|move|set))$' \
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
    rg -v '^(open-esp-radio-dma|open-esp-radio-esp32s31-(coex|phy|hal|registers|pac|pac-raw|ieee802154-irq)|critical-section|vcell) v'
then
    echo "non-workspace dependency survived source-only build" >&2
    exit 1
fi

if rg -n 'core::hint::spin_loop|spin_loop\(' driver --glob '*.rs'
then
    echo "production source contains a CPU spin loop" >&2
    exit 1
fi

# Production documentation names stable evidence IDs and public symbols. Local
# oracle paths, artifact digests and paths into an earlier private firmware
# tree are qualification policy and must stay under verification/HIL.
if rg -n '(?i)(_oracles/|sha-?256|[0-9a-f]{64}|esp32s31_rust/|firmware/esp32s31/)' \
    driver --glob '*.rs' --glob '*.md' --glob '*.toml'
then
    echo "qualification artifact identity survived in production source" >&2
    exit 1
fi

# Build the exact final image from the locked dependencies. A sibling esp-hal
# checkout is a useful HIL development override, but using it here would both
# mutate the embedded lockfile and make this policy gate machine-dependent.
env -u ESP_HAL_ROOT cargo hil image build performance

runtime_elf="target/hil/esp32s31/psram-code-psram-data-psram-stack-performance/cargo/runtime/$target_triple/release/open-esp-radio-hil-esp32s31-runtime"
test -f "$runtime_elf"

# The linker script exposes absolute ESP32-S31 ECO0 ROM symbols even when no
# call references them, so symbol-table matching is insufficient. Decode all
# executable sections and reject statically resolved jumps/calls into the
# pinned radio API table or the contiguous radio implementation body. System
# ROM outside these ranges (for example ets_printf) remains permitted.
cargo blobray advanced image audit-targets \
    --target-spec verification/vendor/targets/esp32s31/target.toml \
    --artifact "$runtime_elf" \
    --forbid 'esp32s31-eco0-radio-api=0x2f800bf0..0x2f8016bc' \
    --forbid 'esp32s31-eco0-radio-body=0x2f823c12..0x2f83e6d0'

echo "source-only radio audit passed: rlib=$artifact runtime=$runtime_elf"
