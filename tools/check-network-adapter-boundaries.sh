#!/usr/bin/env bash
set -euo pipefail

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${repository_root}"

if ! command -v jq >/dev/null 2>&1; then
    echo "jq is required to validate adapter dependency sources" >&2
    exit 1
fi

cargo check --manifest-path driver/adapters/embassy-net-compat/Cargo.toml --all-targets
cargo check --manifest-path driver/adapters/embassy-net/Cargo.toml --all-targets
cargo check -p open-esp-radio-esp32s31-wifi-embassy --all-targets

metadata="$(cargo metadata --format-version 1 --no-deps)"

compatibility_non_registry="$({
    jq -r '
        .packages[]
        | select(.name == "open-esp-radio-embassy-net-compat")
        | .dependencies[]
        | select(.kind != "dev")
        | select((.source // "") | startswith("registry+") | not)
        | .name
    ' <<<"${metadata}"
} || true)"
if [[ -n "${compatibility_non_registry}" ]]; then
    echo "compatibility adapter acquired non-registry dependencies:" >&2
    echo "${compatibility_non_registry}" >&2
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

echo "network adapter compile and dependency boundaries are clean"
