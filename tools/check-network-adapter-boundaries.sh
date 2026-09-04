#!/usr/bin/env bash
set -euo pipefail

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${repository_root}"

if ! command -v jq >/dev/null 2>&1; then
    echo "jq is required to validate adapter dependency sources" >&2
    exit 1
fi

cargo check --manifest-path driver/adapters/embassy-net-compat/Cargo.toml --all-targets
cargo check -p open-esp-radio-esp32s31-wifi-embassy-compat --all-targets
cargo check --manifest-path driver/adapters/embassy-net/Cargo.toml --all-targets
cargo check -p open-esp-radio-wifi-datapath --all-targets
cargo check -p open-esp-radio-research-datapath --all-targets
cargo check -p open-esp-radio-esp32s31-wifi-embassy --all-targets
cargo check -p open-esp-radio-esp32s31-wifi-embassy --no-default-features --all-targets

product_manifest="driver/integration/esp32s31/embassy-wifi/Cargo.toml"
target_triple="riscv32imafc-unknown-none-elf"
cargo check \
    --manifest-path "${product_manifest}" \
    --target "${target_triple}"
cargo check \
    --manifest-path "${product_manifest}" \
    --target "${target_triple}" \
    --no-default-features \
    --features compat-network

metadata="$(cargo metadata --format-version 1 --no-deps)"

neutral_network_dependencies="$({
    jq -r '
        .packages[]
        | select(.name == "open-esp-radio-network")
        | .dependencies[]
        | select(.kind != "dev")
        | .name
    ' <<<"${metadata}"
} || true)"
if [[ -n "${neutral_network_dependencies}" ]]; then
    echo "adapter-neutral network values acquired a production dependency:" >&2
    echo "${neutral_network_dependencies}" >&2
    exit 1
fi

compatibility_forbidden_dependency="$({
    jq -r '
        .packages[]
        | select(.name == "open-esp-radio-embassy-net-compat")
        | .dependencies[]
        | select(.kind != "dev")
        | select((.source // "") | startswith("registry+") | not)
        | select(.name != "open-esp-radio-network")
        | .name
    ' <<<"${metadata}"
} || true)"
if [[ -n "${compatibility_forbidden_dependency}" ]]; then
    echo "compatibility adapter acquired a non-neutral local dependency:" >&2
    echo "${compatibility_forbidden_dependency}" >&2
    exit 1
fi

owned_official_driver="$({
    jq -r '
        .packages[]
        | select(.name == "open-esp-radio-embassy-net")
        | .dependencies[]
        | select(.name == "embassy-net-driver")
        | select((.source // "") | startswith("registry+"))
        | .name
    ' <<<"${metadata}"
} || true)"
if [[ -n "${owned_official_driver}" ]]; then
    echo "optimized owned adapter acquired the released Embassy driver contract" >&2
    exit 1
fi

if rg -q 'open-esp-radio-(dma|esp32s31)' driver/adapters/embassy-net/Cargo.toml; then
    echo "optimized owned adapter acquired a physical radio dependency" >&2
    exit 1
fi

research_dependencies="$(
    cargo tree \
        -p open-esp-radio-research-datapath \
        --edges normal \
        --prefix none
)"
if rg -q '^(embassy-|xarxa(-driver)?|open-esp-radio-embassy)' \
    <<<"${research_dependencies}"; then
    echo "research datapath acquired an Embassy or Xarxa dependency" >&2
    exit 1
fi

radio_datapath_dependencies="$(
    cargo tree \
        -p open-esp-radio-wifi-datapath \
        --edges normal \
        --prefix none
)"
if rg -q '^(embassy-|xarxa(-driver)?|open-esp-radio-embassy|open-esp-radio-esp32s31)' \
    <<<"${radio_datapath_dependencies}"; then
    echo "radio-native datapath contract acquired an adapter or chip dependency" >&2
    exit 1
fi

radio_core_dependencies="$(
    cargo tree \
        -p open-esp-radio-esp32s31-wifi-embassy \
        --no-default-features \
        --edges normal \
        --prefix none
)"
if rg -q '^(open-esp-radio-embassy-net|owned-embassy-net-driver|xarxa(-driver)?) ' \
    <<<"${radio_core_dependencies}"; then
    echo "radio core acquired an optimized Xarxa/Embassy network dependency" >&2
    exit 1
fi

compatibility_bridge_dependencies="$(
    cargo tree \
        -p open-esp-radio-esp32s31-wifi-embassy-compat \
        --edges normal \
        --prefix none
)"
if rg -q '^(open-esp-radio-embassy-net |owned-embassy-net-driver|xarxa(-driver)?) ' \
    <<<"${compatibility_bridge_dependencies}"; then
    echo "compatibility radio bridge acquired an optimized network dependency" >&2
    exit 1
fi
if ! rg -q '^embassy-net-driver v0\.2\.0$' <<<"${compatibility_bridge_dependencies}"; then
    echo "compatibility radio bridge does not resolve the released Embassy driver" >&2
    exit 1
fi

compatibility_product_metadata="$(
    cargo metadata \
        --manifest-path "${product_manifest}" \
        --format-version 1 \
        --no-default-features \
        --features compat-network
)"
compatibility_product_forbidden="$({
    jq -r '
        .packages[]
        | select(
            .name == "xarxa"
            or .name == "xarxa-driver"
            or .name == "open-esp-radio-embassy-net"
        )
        | .name
    ' <<<"${compatibility_product_metadata}"
} || true)"
if [[ -n "${compatibility_product_forbidden}" ]]; then
    echo "compatibility product acquired the optimized owned network graph:" >&2
    echo "${compatibility_product_forbidden}" >&2
    exit 1
fi
compatibility_product_nonrelease_embassy="$({
    jq -r '
        .packages[]
        | select(.name == "embassy-net" or .name == "embassy-net-driver")
        | select(((.source // "") | startswith("registry+")) | not)
        | .id
    ' <<<"${compatibility_product_metadata}"
} || true)"
if [[ -n "${compatibility_product_nonrelease_embassy}" ]]; then
    echo "compatibility product acquired a non-release Embassy network package:" >&2
    echo "${compatibility_product_nonrelease_embassy}" >&2
    exit 1
fi

radio_policy_root="driver/adapters/embassy/esp32s31-wifi/src/roles"
if rg -n \
    --glob '!**/tests.rs' \
    'OwnedNetworkTxFrame|DatapathTxConsumer' \
    "${radio_policy_root}"; then
    echo "radio policy acquired a concrete Xarxa/Embassy TX owner" >&2
    exit 1
fi

echo "network adapter compile and dependency boundaries are clean"
