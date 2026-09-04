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
cargo check -p open-esp-radio-esp32s31-wifi-embassy --all-targets
cargo check -p open-esp-radio-esp32s31-wifi-embassy --no-default-features --all-targets

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

radio_policy_root="driver/adapters/embassy/esp32s31-wifi/src/roles"
if rg -n \
    --glob '!**/tests.rs' \
    'OwnedNetworkTxFrame|DatapathTxConsumer' \
    "${radio_policy_root}"; then
    echo "radio policy acquired a concrete Xarxa/Embassy TX owner" >&2
    exit 1
fi

echo "network adapter compile and dependency boundaries are clean"
