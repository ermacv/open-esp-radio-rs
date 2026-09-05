#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
target_triple="riscv32imafc-unknown-none-elf"
audit_dir="$(mktemp -d)"
trap 'rm -rf -- "$audit_dir"' EXIT

cd "$repo_root"
command -v jq >/dev/null

metadata_for() {
    local manifest="$1"
    local output="$2"
    shift 2

    cargo metadata \
        --format-version 1 \
        --locked \
        --offline \
        --manifest-path "$manifest" \
        --filter-platform "$target_triple" \
        "$@" >"$output"
}

metadata_without_dependencies_for() {
    local manifest="$1"
    local output="$2"

    cargo metadata \
        --format-version 1 \
        --locked \
        --offline \
        --no-deps \
        --manifest-path "$manifest" >"$output"
}

package_id_for_manifest() {
    local metadata="$1"
    local manifest
    manifest="$(realpath "$2")"

    jq -er --arg manifest "$manifest" '
        [.packages[] | select(.manifest_path == $manifest) | .id]
        | if length == 1 then .[0] else error("manifest does not identify exactly one package") end
    ' "$metadata"
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

package_id_for_name() {
    local metadata="$1"
    local package="$2"

    jq -er --arg package "$package" '
        [.packages[] | select(.name == $package) | .id]
        | if length == 1 then .[0] else error("name does not identify exactly one package") end
    ' "$metadata"
}

resolved_packages_with_forbidden_roots() {
    local metadata="$1"
    local root_id="$2"
    local forbidden
    shift 2
    forbidden="$(printf '%s\n' "$@" | jq -R . | jq -s .)"

    jq -r \
        --arg root "$root_id" \
        --argjson forbidden "$forbidden" '
        def closure($id; $edges):
            $id, (($edges[$id] // [])[] | closure(.; $edges));

        (.resolve.nodes
            | map({
                key: .id,
                value: [.deps[] | select(any(.dep_kinds[]; .kind != "dev")) | .pkg]
            })
            | from_entries) as $edges
        | (.packages | map({key: .id, value: .}) | from_entries) as $packages
        | [closure($root; $edges)] | unique[]
        | $packages[.]
        | select(. != null)
        | select(.manifest_path as $path | any($forbidden[]; . as $root | $path | startswith($root)))
        | .manifest_path
    ' "$metadata"
}

resolved_platform_runtime_packages() {
    local metadata="$1"
    local root_id="$2"

    jq -r \
        --arg root "$root_id" \
        '
        def closure($id; $edges):
            $id, (($edges[$id] // [])[] | closure(.; $edges));

        (.resolve.nodes
            | map({
                key: .id,
                value: [.deps[] | select(any(.dep_kinds[]; .kind != "dev")) | .pkg]
            })
            | from_entries) as $edges
        | (.packages | map({key: .id, value: .}) | from_entries) as $packages
        | [closure($root; $edges)] | unique[]
        | $packages[.]
        | select(. != null)
        | select(.name == "esp-hal" or (.name | startswith("embassy-")))
        | .name
    ' "$metadata"
}

graph_has_feature() {
    local metadata="$1"
    local feature="$2"

    jq -e --arg feature "$feature" '
        any(.resolve.nodes[]; any(.features[]; . == $feature))
    ' "$metadata" >/dev/null
}

package_has_feature() {
    local metadata="$1"
    local package="$2"
    local feature="$3"

    jq -e --arg package "$package" --arg feature "$feature" '
        ([.packages[] | select(.name == $package) | .id]
            | if length == 1 then .[0] else error("name does not identify exactly one package") end) as $id
        | any(.resolve.nodes[]; .id == $id and any(.features[]; . == $feature))
    ' "$metadata" >/dev/null
}

assert_graph_lacks_features() {
    local metadata="$1"
    shift

    local feature
    for feature in "$@"; do
        if graph_has_feature "$metadata" "$feature"; then
            echo "forbidden feature is enabled in resolved graph: $feature" >&2
            exit 1
        fi
    done
}

# Every Cargo package below driver/ ships or composes production behavior.
# Discover manifests so a new package is compiled in all supported feature
# modes. Every non-development local dependency declaration must stay under
# `driver/`; this feature-independent rule is stricter than inspecting only the
# dependencies selected by one resolved feature graph.
mapfile -t production_manifests < <(find driver -name target -type d -prune -o -name Cargo.toml -print | sort)
test "${#production_manifests[@]}" -gt 0

workspace_packages="$audit_dir/workspace-packages.json"
metadata_without_dependencies_for Cargo.toml "$workspace_packages"
driver_root="$(realpath driver)/"
isolated_profile_count=0

for manifest in "${production_manifests[@]}"; do
    manifest_absolute="$(realpath "$manifest")"
    package="$(jq -r --arg manifest "$manifest_absolute" '
        [.packages[] | select(.manifest_path == $manifest) | .name]
        | if length == 0 then "" elif length == 1 then .[0] else error("manifest identifies multiple packages") end
    ' "$workspace_packages")"
    package_metadata="$workspace_packages"
    if [[ -z "$package" ]]; then
        package_metadata="$audit_dir/package-$(basename "${manifest%/Cargo.toml}").json"
        metadata_without_dependencies_for "$manifest" "$package_metadata"
        package="$(package_name_for_manifest "$package_metadata" "$manifest")"
    fi

    violations="$(jq -r \
        --arg manifest "$manifest_absolute" \
        --arg driver_root "$driver_root" '
        .packages[]
        | select(.manifest_path == $manifest)
        | .dependencies[]
        | select(.kind != "dev" and .path != null)
        | select(((.path + "/") | startswith($driver_root)) | not)
        | .path
    ' "$package_metadata")"
    if [[ -n "$violations" ]]; then
        echo "production package declares a local dependency outside driver/: $package" >&2
        echo "$violations" >&2
        exit 1
    fi

    mapfile -t supported_feature_profiles < <(jq -r \
        --arg manifest "$manifest_absolute" '
        .packages[]
        | select(.manifest_path == $manifest)
        | .metadata["open-radio"]["supported-feature-profiles"][]?
    ' "$package_metadata")
    if ((${#supported_feature_profiles[@]} != 0)); then
        for profile in "${supported_feature_profiles[@]}"; do
            cargo check \
                --quiet \
                --locked \
                --offline \
                --manifest-path "$manifest" \
                --package "$package" \
                --target "$target_triple" \
                --no-default-features \
                --features "$profile"
            isolated_profile_count=$((isolated_profile_count + 1))
        done
    else
        for mode in no-default-features default-features all-features; do
            check_arguments=()
            case "$mode" in
                no-default-features) check_arguments+=(--no-default-features) ;;
                default-features) ;;
                all-features) check_arguments+=(--all-features) ;;
            esac
            cargo check \
                --quiet \
                --locked \
                --offline \
                --manifest-path "$manifest" \
                --package "$package" \
                --target "$target_triple" \
                "${check_arguments[@]}"
            isolated_profile_count=$((isolated_profile_count + 1))
        done
    fi
done
echo "driver architecture compilation: $isolated_profile_count isolated feature profiles"

workspace_graph="$audit_dir/workspace.json"
metadata_for Cargo.toml "$workspace_graph"

# Protocol and chip policy cannot depend upwards on executors, board
# composition, network backends, HIL, esp-hal, or Embassy.
for package in \
    open-esp-radio-wifi-ap \
    open-esp-radio-wifi-sta \
    open-esp-radio-wifi-softmac \
    open-esp-radio-esp32s31-wifi-ap \
    open-esp-radio-esp32s31-wifi-sta
do
    root_id="$(package_id_for_name "$workspace_graph" "$package")"
    violations="$(resolved_packages_with_forbidden_roots \
        "$workspace_graph" \
        "$root_id" \
        "$repo_root/driver/adapters/" \
        "$repo_root/driver/runtime/" \
        "$repo_root/driver/network/adapters/" \
        "$repo_root/driver/network/research/" \
        "$repo_root/driver/integration/" \
        "$repo_root/hil/")"
    if [[ -n "$violations" ]]; then
        echo "policy layer depends on an upper layer: $package" >&2
        echo "$violations" >&2
        exit 1
    fi

    violations="$(resolved_platform_runtime_packages "$workspace_graph" "$root_id")"
    if [[ -n "$violations" ]]; then
        echo "policy layer depends on a platform runtime: $package" >&2
        echo "$violations" >&2
        exit 1
    fi
done

# The generic radio facade must remain independent from chip implementations
# and platform adapters.
radio_manifest="driver/radio/Cargo.toml"
radio_graph="$audit_dir/radio.json"
metadata_for "$radio_manifest" "$radio_graph"
radio_root_id="$(package_id_for_manifest "$radio_graph" "$radio_manifest")"
violations="$(resolved_packages_with_forbidden_roots \
    "$radio_graph" \
    "$radio_root_id" \
    "$repo_root/driver/chips/esp32s31/" \
    "$repo_root/driver/adapters/esp-hal/" \
    "$repo_root/driver/runtime/embassy/esp32s31/" \
    "$repo_root/driver/integration/esp32s31/")"
if [[ -n "$violations" ]]; then
    echo "generic radio facade depends on a concrete platform:" >&2
    echo "$violations" >&2
    exit 1
fi

# Composition owns the complete ESP32-S31 Wi-Fi stack directly. This is a
# graph contract, not a source-spelling assertion.
integration_manifest="driver/integration/esp32s31/embassy/ieee80211/Cargo.toml"
integration_graph="$audit_dir/integration-direct.json"
metadata_for "$integration_manifest" "$integration_graph" --no-deps
integration_manifest_absolute="$(realpath "$integration_manifest")"
for dependency in \
    open-esp-radio-esp32s31-hal \
    open-esp-radio-esp32s31-phy \
    open-esp-radio-esp32s31-wifi \
    open-esp-radio-esp32s31-wifi-mac \
    open-esp-radio-esp32s31-wifi-ap \
    open-esp-radio-esp32s31-wifi-sta
do
    if ! jq -e \
        --arg manifest "$integration_manifest_absolute" \
        --arg dependency "$dependency" '
            any(.packages[] | select(.manifest_path == $manifest) | .dependencies[]; .name == $dependency)
        ' "$integration_graph" >/dev/null
    then
        echo "integration package lacks required direct dependency: $dependency" >&2
        exit 1
    fi
done

# Diagnostics must be opt-in. Inspect resolved Cargo feature sets rather than
# formatted cargo-tree output.
for mode in ordinary diagnostics; do
    graph="$audit_dir/integration-$mode.json"
    feature_arguments=(--no-default-features)
    if [[ "$mode" == diagnostics ]]; then
        feature_arguments+=(--features diagnostics)
    fi
    metadata_for "$integration_manifest" "$graph" "${feature_arguments[@]}"
    assert_graph_lacks_features "$graph" cooperative-scheduler-telemetry
done

hil_runtime_manifest="hil/targets/esp32s31/runtime/Cargo.toml"
integration_package="open-esp-radio-esp32s31-embassy-wifi"
common_hil_features="open-radio-hil,psram-task-stack,code-psram,profile-psram-data"
for profile in performance correctness; do
    features="$common_hil_features"
    if [[ "$profile" == correctness ]]; then
        features+=",driver-observation"
    fi

    graph="$audit_dir/hil-$profile.json"
    metadata_for "$hil_runtime_manifest" "$graph" \
        --no-default-features \
        --features "$features"
    assert_graph_lacks_features \
        "$graph" \
        cooperative-scheduler-telemetry \
        network-scheduler-observation \
        task-poll-telemetry \
        mac-irq-diagnostics

    if [[ "$profile" == performance ]] && package_has_feature "$graph" "$integration_package" diagnostics; then
        echo "performance HIL profile enabled integration diagnostics" >&2
        exit 1
    fi
    if [[ "$profile" == correctness ]] && ! package_has_feature "$graph" "$integration_package" diagnostics; then
        echo "correctness HIL profile did not enable integration diagnostics" >&2
        exit 1
    fi

    package_id_for_manifest "$graph" "$hil_runtime_manifest" >/dev/null
done

task_residence_graph="$audit_dir/hil-task-residence.json"
metadata_for "$hil_runtime_manifest" "$task_residence_graph" \
    --no-default-features \
    --features "$common_hil_features,task-residence-telemetry"
if package_has_feature "$task_residence_graph" "$integration_package" task-poll-telemetry; then
    echo "minimal task-residence HIL profile enabled intrusive integration telemetry" >&2
    exit 1
fi

core0_rx_cycle_graph="$audit_dir/hil-core0-rx-cycle.json"
metadata_for "$hil_runtime_manifest" "$core0_rx_cycle_graph" \
    --no-default-features \
    --features "$common_hil_features,core0-rx-cycle-telemetry"
if ! package_has_feature "$core0_rx_cycle_graph" "$integration_package" task-poll-telemetry; then
    echo "Core0 RX cycle HIL profile did not enable integration phase telemetry" >&2
    exit 1
fi

mac_irq_graph="$audit_dir/hil-mac-irq.json"
metadata_for "$hil_runtime_manifest" "$mac_irq_graph" \
    --no-default-features \
    --features "$common_hil_features,mac-irq-telemetry"
if ! package_has_feature "$mac_irq_graph" "$integration_package" mac-irq-diagnostics; then
    echo "MAC-IRQ HIL profile did not enable integration MAC diagnostics" >&2
    exit 1
fi

# Execute compiled ownership/state-machine tests. Test discovery belongs to
# Rust and Cargo; this audit intentionally does not assert test names.
cargo test \
    --quiet \
    --locked \
    --offline \
    --package open-esp-radio-esp32s31-wifi-embassy

# This integration is an excluded workspace because its firmware dependencies
# select a concrete chip. Its product resource ownership must still be exercised
# on the host, where no network backend or hardware binding is required.
cargo test \
    --quiet \
    --locked \
    --offline \
    --manifest-path "$integration_manifest" \
    --no-default-features

echo "driver architecture audit passed (${#production_manifests[@]} production packages)"
