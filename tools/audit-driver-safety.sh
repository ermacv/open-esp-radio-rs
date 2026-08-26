#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
target_triple="riscv32imafc-unknown-none-elf"
audit_dir="$(mktemp -d)"
trap 'rm -rf -- "$audit_dir"' EXIT

cd "$repo_root"
command -v jq >/dev/null

# Unsafe is an implementation detail of these audited foundations. Clippy
# enforces the boundary on compiled Rust; handwritten leaves may reopen unsafe
# only at an explicitly allowed operation. The raw PAC is generated and is
# validated by its generator/publisher pipeline, so this audit only compiles it.
generated_unsafe_package="open-esp-radio-esp32s31-pac-raw"
audited_unsafe_packages=(
    open-esp-radio-dma
    open-esp-radio-esp32s31-bluetooth
    open-esp-radio-esp32s31-hal
    open-esp-radio-esp32s31-pac
    open-esp-radio-esp32s31-phy
    open-esp-radio-esp32s31-ieee802154-dma
    open-esp-radio-esp32s31-ieee802154-runtime
    open-esp-radio-esp32s31-wifi-dma
    open-esp-radio-esp32s31-radio-platform-esp-hal
    open-esp-radio-esp32s31-embassy-runtime
    open-esp-radio-esp32s31-bluetooth-integration
    open-esp-radio-esp32s31-embassy-wifi
)

pac_dependency_allowed_packages=(
    open-esp-radio-esp32s31-pac-raw
    open-esp-radio-esp32s31-pac
    open-esp-radio-esp32s31-hal
    open-esp-radio-esp32s31-bluetooth
    open-esp-radio-esp32s31-ieee802154-irq
    open-esp-radio-esp32s31-ieee802154-runtime
    open-esp-radio-esp32s31-ieee802154-esp-hal
)

contains_exactly() {
    local candidate="$1"
    shift

    local item
    for item in "$@"; do
        if [[ "$candidate" == "$item" ]]; then
            return 0
        fi
    done
    return 1
}

metadata_for_manifest() {
    local manifest="$1"
    local output="$2"

    cargo metadata \
        --format-version 1 \
        --locked \
        --offline \
        --no-deps \
        --manifest-path "$manifest" >"$output"
}

package_name_for_manifest() {
    local metadata="$1"
    local manifest
    manifest="$(realpath "$2")"

    jq -er --arg manifest "$manifest" '
        [.packages[] | select(.manifest_path == $manifest) | .name]
        | if length == 1 then .[0] else error("manifest does not identify exactly one package") end
    ' "$metadata"
}

package_has_library_target() {
    local metadata="$1"
    local manifest
    manifest="$(realpath "$2")"

    jq -e --arg manifest "$manifest" '
        any(.packages[] | select(.manifest_path == $manifest) | .targets[];
            any(.kind[]; . == "lib" or . == "rlib"))
    ' "$metadata" >/dev/null
}

package_has_direct_dependency() {
    local metadata="$1"
    local manifest="$2"
    local dependency="$3"
    manifest="$(realpath "$manifest")"

    jq -e \
        --arg manifest "$manifest" \
        --arg dependency "$dependency" '
            any(.packages[] | select(.manifest_path == $manifest) | .dependencies[];
                .name == $dependency)
        ' "$metadata" >/dev/null
}

mapfile -t manifests < <(find driver -name Cargo.toml -not -path '*/target/*' -print | sort)
test "${#manifests[@]}" -gt 0

safe_package_count=0
unsafe_package_count=0
for manifest in "${manifests[@]}"; do
    metadata="$audit_dir/package-$(basename "${manifest%/Cargo.toml}").json"
    metadata_for_manifest "$manifest" "$metadata"
    package="$(package_name_for_manifest "$metadata" "$manifest")"

    if ! package_has_library_target "$metadata" "$manifest"; then
        echo "driver package has no library target: $package" >&2
        exit 1
    fi

    if package_has_direct_dependency \
        "$metadata" \
        "$manifest" \
        open-esp-radio-esp32s31-pac \
        && ! contains_exactly "$package" "${pac_dependency_allowed_packages[@]}"
    then
        echo "package crosses the closed-PAC ownership boundary: $package" >&2
        exit 1
    fi

    common_clippy_arguments=(
        --quiet
        --locked
        --offline
        --manifest-path "$manifest"
        --package "$package"
        --target "$target_triple"
        --lib
        --all-features
        --no-deps
    )

    if [[ "$package" == "$generated_unsafe_package" ]]; then
        cargo check \
            --quiet \
            --locked \
            --offline \
            --manifest-path "$manifest" \
            --package "$package" \
            --target "$target_triple" \
            --lib \
            --all-features
    elif contains_exactly "$package" "${audited_unsafe_packages[@]}"; then
        cargo clippy "${common_clippy_arguments[@]}" -- \
            -D unsafe-code \
            -D unsafe-op-in-unsafe-fn
        unsafe_package_count=$((unsafe_package_count + 1))
    else
        cargo clippy "${common_clippy_arguments[@]}" -- -F unsafe-code
        safe_package_count=$((safe_package_count + 1))
    fi
done

# Execute behavioral tests for the ownership foundations. Test discovery and
# selection remain Cargo/Rust responsibilities; the audit does not inspect or
# assert source-level test names.
test_packages=(
    open-esp-radio-dma
    open-esp-radio-esp32s31-pac
    open-esp-radio-esp32s31-hal
    open-esp-radio-esp32s31-phy
    open-esp-radio-esp32s31-bluetooth
    open-esp-radio-esp32s31-ieee802154-dma
    open-esp-radio-esp32s31-ieee802154-runtime
    open-esp-radio-esp32s31-wifi-dma
)
test_arguments=(test --quiet --locked --offline)
for package in "${test_packages[@]}"; do
    test_arguments+=(--package "$package")
done
cargo "${test_arguments[@]}"

echo "driver safety audit passed ($safe_package_count safe packages, $unsafe_package_count audited unsafe packages)"
