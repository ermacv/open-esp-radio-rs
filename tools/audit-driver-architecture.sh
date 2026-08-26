#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
target_triple="riscv32imafc-unknown-none-elf"
audit_dir="$(mktemp -d)"
trap 'rm -rf -- "$audit_dir"' EXIT

cd "$repo_root"

# Every Cargo package below driver/ ships or composes production behavior.
# Discover manifests instead of maintaining an allowlist that silently misses
# a new backend, adapter, or integration workspace.
mapfile -t production_manifests < <(find driver -name Cargo.toml -print | sort)
test "${#production_manifests[@]}" -gt 0

for manifest in "${production_manifests[@]}"; do
    package="$(sed -n 's/^name = "\([^"]*\)"/\1/p' "$manifest" | head -n 1)"
    test -n "$package"

    for mode in no-default-features default-features all-features; do
        arguments=(
            tree
            --locked
            --offline
            --manifest-path "$manifest"
            --package "$package"
            --target "$target_triple"
            --edges normal,build
            --prefix none
        )
        case "$mode" in
            no-default-features) arguments+=(--no-default-features) ;;
            default-features) ;;
            all-features) arguments+=(--all-features) ;;
        esac

        graph="$audit_dir/dependencies-$package-$mode"
        cargo "${arguments[@]}" >"$graph"
        if rg \
            '^(blobray($|-)|vendor-code-validator|open-radio-vendor-|open-esp-radio-(hil-runner|hil-(protocol|.*telemetry)|verification-.*-probes|.*vendor-oracle))' \
            "$graph"
        then
            echo "research, HIL, or qualification policy dependency survived in $package ($mode)" >&2
            exit 1
        fi

        check_arguments=(
            check
            --quiet
            --locked
            --offline
            --manifest-path "$manifest"
            --package "$package"
            --target "$target_triple"
        )
        case "$mode" in
            no-default-features) check_arguments+=(--no-default-features) ;;
            default-features) ;;
            all-features) check_arguments+=(--all-features) ;;
        esac
        cargo "${check_arguments[@]}"
    done
done

# Protocol and chip policy cannot depend upwards on executors, board
# composition, or HIL. Audit the complete transitive normal/build graph.
for package in \
    open-esp-radio-wifi-ap \
    open-esp-radio-wifi-sta \
    open-esp-radio-wifi-softmac \
    open-esp-radio-esp32s31-wifi-ap \
    open-esp-radio-esp32s31-wifi-sta
do
    cargo tree \
        --locked \
        --offline \
        --package "$package" \
        --target "$target_triple" \
        --edges normal,build \
        --prefix none >"$audit_dir/layer-$package"
    if rg \
        '^(embassy-|esp-hal |open-esp-radio-(embassy-net|esp32s31-embassy-wifi|esp32s31-wifi-embassy|hil-))' \
        "$audit_dir/layer-$package"
    then
        echo "runtime, adapter, integration, or HIL dependency survived below the adapter layer: $package" >&2
        exit 1
    fi
done

# Qualification is external policy. Production may expose diagnostics and
# observations, but it must never compile behavior under a qualification flag.
if rg -n \
    '#\[cfg(_attr)?\([^]]*feature[[:space:]]*=[[:space:]]*"qualification"' \
    driver --glob '*.rs'
then
    echo "behavioral qualification cfg survived in production source" >&2
    exit 1
fi

# The adapter layout is executable architecture, not a documentation
# convention. Network capabilities have one canonical module; the former
# `datapath::*` forwarding surface must not return.
adapter_source="driver/adapters/embassy/esp32s31-wifi/src"
for required in \
    datapath/network.rs \
    datapath/scheduler.rs \
    datapath/irq.rs \
    datapath/tx/mod.rs \
    datapath/rx/mod.rs \
    roles/station.rs \
    roles/access_point.rs \
    roles/concurrent.rs \
    roles/scan.rs \
    roles/monitor.rs \
    diagnostics.rs \
    composition.rs
do
    test -f "$adapter_source/$required" || {
        echo "required adapter ownership module is missing: $required" >&2
        exit 1
    }
done
if rg -n \
    'pub use (crate::)?datapath::network|pub use (self::)?network::' \
    "$adapter_source/datapath" --glob '*.rs'
then
    echo "legacy datapath-level network forwarding surface survived" >&2
    exit 1
fi

# Role and integration cutovers have one path each. Reintroducing a flat
# connected facade or a generic `runtime` bucket would recreate parallel APIs
# and make ownership ambiguous again.
test -f "$adapter_source/roles/station/connected/mod.rs"
test -f "$adapter_source/roles/access_point/concurrent.rs"
if find "$adapter_source/roles/station" -maxdepth 1 \
    \( -name 'connected_*.rs' -o -name 'port.rs' -o -name port \) | rg .
then
    echo "flat/legacy connected-station path survived the station SPI cutover" >&2
    exit 1
fi
if test -e "$adapter_source/roles/access_point/runtime.rs"; then
    echo "ambiguous AP runtime module survived the concurrent-role cutover" >&2
    exit 1
fi

integration_source="driver/integration/esp32s31/embassy-wifi/src"
for required in \
    supervisor/mod.rs \
    supervisor/station.rs \
    supervisor/access_point.rs \
    supervisor/concurrent.rs \
    supervisor/station_epoch.rs \
    supervisor/role_dispatch.rs \
    status/mod.rs \
    diagnostics.rs
do
    test -f "$integration_source/$required" || {
        echo "required integration responsibility module is missing: $required" >&2
        exit 1
    }
done
if test "$(wc -l < "$integration_source/supervisor/mod.rs")" -gt 1000; then
    echo "integration supervisor facade became monolithic again" >&2
    exit 1
fi
for removed in connected.rs runtime.rs access_point_status.rs station_status.rs; do
    if test -e "$integration_source/$removed"; then
        echo "legacy integration monolith survived: $removed" >&2
        exit 1
    fi
done

runner_source="hil/host/runner/src"
for responsibility in device image transport traffic evidence qualification reporting; do
    test -f "$runner_source/$responsibility/mod.rs" || {
        echo "HIL runner responsibility module is missing: $responsibility" >&2
        exit 1
    }
done
if test "$(wc -l < "$runner_source/main.rs")" -gt 900; then
    echo "HIL runner entry point owns implementation responsibilities again" >&2
    exit 1
fi
if find "$runner_source" -maxdepth 1 -type f \
    \( -name '*traffic*.rs' -o -name '*qualification*.rs' -o -name '*fixture*.rs' \) | rg .
then
    echo "flat HIL transport/traffic/qualification module survived" >&2
    exit 1
fi

# HIL consumes stable decoded integration observations. Raw public-header
# parsing and chip MAC/STA/IEEE implementation dependencies belong below that
# boundary and must not return to the runtime image.
hil_runtime="hil/targets/esp32s31/runtime"
if rg -n \
    'open-esp-radio-(esp32s31-wifi-(mac|sta)|ieee80211)' \
    "$hil_runtime/Cargo.toml" \
    || rg -n 'PUBLIC_HEADER_SIZE|decode_rx_phy_info|ConnectedRxEvent|public_qos_sequence' \
        "$hil_runtime/src" --glob '*.rs'
then
    echo "raw or low-level RX decoding survived in HIL runtime" >&2
    exit 1
fi
if rg -n '^qualification[[:space:]]*=' driver --glob Cargo.toml
then
    echo "qualification feature survived in a production manifest" >&2
    exit 1
fi
if rg -n 'rx-delivery-observation' \
    driver/adapters/embassy/esp32s31-wifi \
    driver/integration/esp32s31/embassy-wifi
then
    echo "obsolete narrow diagnostics feature survived the adapter cutover" >&2
    exit 1
fi

# Ordinary production images must not retain observer pointers or hard-IRQ
# accounting. A disabled callback is still state and branching in the hot
# owner graph, so every observer field and the RX-post counter must be removed
# by cfg, not merely initialized to None/zero.
ordinary_diagnostics_sources=(
    "$adapter_source/datapath/irq/mac_runtime.rs"
    "$adapter_source/datapath/rx"
    "$adapter_source/roles"
)
if ! awk '
    function guarded() {
        return $0 ~ /#\[cfg\(/ || previous ~ /#\[cfg\(/ || before_previous ~ /#\[cfg\(/
    }
    {
        if (($0 ~ /(pipeline_observer|aggregate_tx_observer|terminal_observer|delivery_observer):[[:space:]]*Option</ ||
             $0 ~ /observer:[[:space:]]*Option<&.*(AggregateTxObserver|RxPipelineObserver)/ ||
             $0 ~ /rx_post_count:[[:space:]]*AtomicU32/) && !guarded()) {
            print FILENAME ":" FNR ": ordinary-build diagnostic state: " $0 > "/dev/stderr"
            failed = 1
        }
        before_previous = previous
        previous = $0
    }
    END { exit failed }
' $(find "${ordinary_diagnostics_sources[@]}" -type f -name '*.rs' -print | sort); then
    echo "observer or hard-IRQ diagnostic state survived the ordinary driver graph" >&2
    exit 1
fi
if rg -n '\.dropped_events\(\)|dropped:[[:space:]]*AtomicU32' \
    "$adapter_source/roles/station/control"* --glob '*.rs'
then
    echo "lossy station control-mailbox accounting survived the correctness cutover" >&2
    exit 1
fi

# Complexity exceptions must identify one concrete ownership-heavy module.
# Crate-wide exceptions hide unrelated growth and are therefore forbidden;
# module-local expectations remain visible beside the exact static owner graph
# which makes the lint inapplicable.
mapfile -t production_crate_roots < <(find driver -path '*/src/lib.rs' -o -path '*/src/main.rs' | sort)
if rg -n \
    '^#!\[(allow|expect)\([^]]*(clippy::too_many_arguments|clippy::type_complexity)' \
    "${production_crate_roots[@]}"
then
    echo "crate-wide ownership-complexity allowance survived" >&2
    exit 1
fi

# WDEV is a vendor symbol-family hint, not a production layer. Source notes
# may cite exact vendor names, but the Rust module and owner API cut over once.
if find \
    driver/adapters/embassy/esp32s31-wifi/src \
    driver/chips/esp32s31/wifi/src \
    \( -type d -name wdev -o -type f -name 'wdev.rs' \) | rg .
then
    echo "legacy wdev module survived the datapath cutover" >&2
    exit 1
fi
if rg -n '\bWdev[A-Z]' \
    driver/adapters/embassy/esp32s31-wifi/src \
    driver/chips/esp32s31/wifi/src \
    --glob '*.rs'
then
    echo "legacy Wdev Rust API survived the datapath cutover" >&2
    exit 1
fi

# Role-specific protocol/network policy may not migrate back into the
# role-neutral datapath tree after the structural cutover.
if rg -n \
    'open_esp_radio_(wifi_(sta|ap)|esp32s31_wifi_(sta|ap))|roles::(station|access_point)' \
    driver/adapters/embassy/esp32s31-wifi/src/datapath \
    --glob '*.rs' --glob '!tests.rs' --glob '!**/tests.rs'
then
    echo "role policy leaked into the role-neutral datapath" >&2
    exit 1
fi

# The generic application facade must not own or tunnel a chip backend.
# Chip startup and physical-role composition belong to integration crates.
if rg -n \
    'open-esp-radio-esp32s31|esp32s31-wifi|validation-raw-dma' \
    driver/radio/Cargo.toml driver/radio/src --glob '*.rs'
then
    echo "ESP32-S31 backend dependency or feature survived in the generic facade" >&2
    exit 1
fi
if test -e driver/radio/src/esp32s31; then
    echo "chip-specific composition survived in the generic facade" >&2
    exit 1
fi

# `new` is the sole free application root. Secondary owners expose associated
# constructors, so they cannot be confused with another hardware root.
if rg -n 'new_wifi_network|pub use wifi_network::\{[^}]*new' \
    driver/integration/esp32s31/embassy-wifi/src \
    examples hil --glob '*.rs'
then
    echo "secondary free integration constructor survived" >&2
    exit 1
fi

# Scheduler instrumentation changes the hot executor/network path and must be
# selected by a diagnostic HIL image, never by the integration diagnostics API.
integration_manifest="driver/integration/esp32s31/embassy-wifi/Cargo.toml"
for dependency in \
    open-esp-radio-esp32s31-hal \
    open-esp-radio-esp32s31-phy \
    open-esp-radio-esp32s31-wifi \
    open-esp-radio-esp32s31-wifi-mac \
    open-esp-radio-esp32s31-wifi-ap \
    open-esp-radio-esp32s31-wifi-sta
do
    if ! rg -q "^${dependency}[[:space:]]*=" "$integration_manifest"; then
        echo "integration lost direct backend dependency: $dependency" >&2
        exit 1
    fi
done
for mode in ordinary diagnostics; do
    arguments=(
        tree
        --locked
        --offline
        --manifest-path "$integration_manifest"
        --package open-esp-radio-esp32s31-embassy-wifi
        --target "$target_triple"
        --edges features
        --prefix none
        --no-default-features
    )
    if test "$mode" = diagnostics; then
        arguments+=(--features diagnostics)
    fi
    cargo "${arguments[@]}" >"$audit_dir/integration-$mode-features"
    if rg 'cooperative-scheduler-telemetry' "$audit_dir/integration-$mode-features"
    then
        echo "network scheduler telemetry survived in the $mode integration graph" >&2
        exit 1
    fi
done

# HIL image classes are intentionally different compiled graphs. Performance
# has no driver observers; correctness adds those observers but not intrusive
# task-poll or hard-IRQ instrumentation.
hil_manifest="hil/targets/esp32s31/Cargo.toml"
hil_common_features="open-radio-hil,psram-task-stack,code-psram,profile-psram-data"
for mode in performance correctness; do
    features="$hil_common_features"
    if test "$mode" = correctness; then
        features="$features,driver-observation"
    fi
    graph="$audit_dir/hil-$mode-integration-features"
    cargo tree \
        --locked \
        --offline \
        --manifest-path "$hil_manifest" \
        --package open-esp-radio-hil-esp32s31-runtime \
        --target "$target_triple" \
        --edges features \
        --no-default-features \
        --features "$features" \
        --invert open-esp-radio-esp32s31-embassy-wifi >"$graph"

    if rg 'cooperative-scheduler-telemetry|network-scheduler-observation|task-poll-telemetry|mac-irq-diagnostics' "$graph"
    then
        echo "intrusive scheduler, task-poll, or hard-IRQ telemetry survived in the $mode HIL image" >&2
        exit 1
    fi
    if test "$mode" = performance && rg 'feature "diagnostics"' "$graph"; then
        echo "driver observers survived in the performance HIL image" >&2
        exit 1
    fi
    if test "$mode" = correctness && ! rg -q 'feature "diagnostics"' "$graph"; then
        echo "correctness HIL image lost its driver observers" >&2
        exit 1
    fi
done

# The task-poll image is the only diagnostic graph allowed to time every
# protocol Future::poll. Its feature must be absent from correctness above and
# explicitly reach the integration crate here.
cargo tree \
    --locked \
    --offline \
    --manifest-path "$hil_manifest" \
    --package open-esp-radio-hil-esp32s31-runtime \
    --target "$target_triple" \
    --edges features \
    --no-default-features \
    --features \
    open-radio-hil,psram-task-stack,task-poll-telemetry,code-psram,profile-psram-data \
    --invert open-esp-radio-esp32s31-embassy-wifi >"$audit_dir/hil-task-poll-features"
if ! rg -q 'open-esp-radio-esp32s31-embassy-wifi feature "task-poll-telemetry"' \
    "$audit_dir/hil-task-poll-features"
then
    echo "diagnostic task-poll image lost its integration poll observer" >&2
    exit 1
fi

# Hard-IRQ instrumentation is an explicit diagnostic graph of its own. It may
# not be pulled transitively by the broad driver-observation bundle used for
# correctness evidence.
cargo tree \
    --locked \
    --offline \
    --manifest-path "$hil_manifest" \
    --package open-esp-radio-hil-esp32s31-runtime \
    --target "$target_triple" \
    --edges features \
    --no-default-features \
    --features \
    open-radio-hil,psram-task-stack,mac-irq-telemetry,code-psram,profile-psram-data \
    --invert open-esp-radio-esp32s31-embassy-wifi >"$audit_dir/hil-mac-irq-features"
if ! rg -q 'open-esp-radio-esp32s31-embassy-wifi feature "mac-irq-diagnostics"' \
    "$audit_dir/hil-mac-irq-features"
then
    echo "diagnostic MAC IRQ image lost its explicit integration observer" >&2
    exit 1
fi

# These tests are executable proofs of the single and concurrent affine owner
# graphs. Their names are stable architecture contracts, not incidental unit
# coverage.
cargo test \
    --package open-esp-radio-esp32s31-wifi-embassy \
    owner_graph_contract

# PHY-I2C knowledge and authority have one owner per layer. The PHY owns the
# reviewed block/host mapping and transaction order; HAL owns the pure S31
# ANA_CONF2 transform; only the two official ESP-HAL owners may touch the
# I2C_ANA_MST register block. This also prevents a validation bridge from
# becoming an ordinary public command-serialization bypass.
phy_i2c_hal="driver/chips/esp32s31/hal/src/phy_i2c.rs"
phy_i2c_driver="driver/chips/esp32s31/phy/src/phy_i2c.rs"
phy_i2c_validation="driver/chips/esp32s31/phy/src/validation.rs"
phy_i2c_adapter_bluetooth="driver/adapters/esp-hal/esp32s31-radio-platform/src/esp32s31.rs"
phy_i2c_adapter_wifi="driver/adapters/esp-hal/esp32s31-wifi/src/lib.rs"

mapfile -t phy_i2c_mmio_owners < <(rg -l 'I2C_ANA_MST::regs\(\)' driver --glob '*.rs' | sort)
if test "${#phy_i2c_mmio_owners[@]}" -ne 2 \
    || test "${phy_i2c_mmio_owners[0]}" != "$phy_i2c_adapter_bluetooth" \
    || test "${phy_i2c_mmio_owners[1]}" != "$phy_i2c_adapter_wifi"
then
    echo "PHY-I2C MMIO authority escaped the two official ESP-HAL owners" >&2
    exit 1
fi

mapfile -t phy_i2c_impl_owners < <(rg -l 'impl PhyI2cMasterControl for' driver --glob '*.rs' | sort)
if test "${#phy_i2c_impl_owners[@]}" -ne 2 \
    || test "${phy_i2c_impl_owners[0]}" != "$phy_i2c_adapter_bluetooth" \
    || test "${phy_i2c_impl_owners[1]}" != "$phy_i2c_adapter_wifi"
then
    echo "PHY-I2C platform implementation inventory changed without a serialization owner" >&2
    exit 1
fi

if rg -n '0x00fc_000f' driver --glob '*.rs'; then
    echo "obsolete PHY-I2C host-map mask that clears the upper byte survived" >&2
    exit 1
fi
mapfile -t phy_i2c_transform_owners < <(
    rg -l '0xfffc_000f|0x0003_fa00' driver --glob '*.rs' | sort
)
if test "${#phy_i2c_transform_owners[@]}" -ne 1 \
    || test "${phy_i2c_transform_owners[0]}" != "$phy_i2c_hal"
then
    echo "PHY-I2C ANA_CONF2 transform has more than one production owner" >&2
    exit 1
fi
mapfile -t phy_i2c_host_knowledge_owners < <(rg -l '0x0647' driver --glob '*.rs' | sort)
if test "${#phy_i2c_host_knowledge_owners[@]}" -ne 1 \
    || test "${phy_i2c_host_knowledge_owners[0]}" != "$phy_i2c_driver"
then
    echo "PHY-I2C host-map knowledge escaped its PHY owner" >&2
    exit 1
fi

if rg -n 'try_(start|finish)_(read|write)' "$phy_i2c_hal"; then
    echo "HAL regained ownership of PHY-I2C command sequencing" >&2
    exit 1
fi
for helper in \
    try_start_read \
    try_finish_read \
    try_start_write \
    try_finish_write \
    configure_and_select_phy_i2c_host
do
    if ! rg -q "pub\(crate\) fn ${helper}" "$phy_i2c_driver"; then
        echo "PHY-I2C serialization helper is not crate-private: $helper" >&2
        exit 1
    fi
done
if ! rg -q '#\[cfg\(feature = "validation-probes"\)\][[:space:]]*' \
    driver/chips/esp32s31/phy/src/lib.rs \
    || ! rg -q 'crate::phy_i2c::configure_and_select_phy_i2c_host' "$phy_i2c_validation"
then
    echo "PHY-I2C compiled-comparison bridge escaped its feature or production delegate" >&2
    exit 1
fi
for adapter in "$phy_i2c_adapter_bluetooth" "$phy_i2c_adapter_wifi"; do
    if ! rg -q 'configured_host_map_image' "$adapter" \
        || ! rg -q '_i2c_ana_mst: I2C_ANA_MST' "$adapter"
    then
        echo "official PHY-I2C adapter lost its affine owner or shared transform: $adapter" >&2
        exit 1
    fi
done

echo "driver architecture audit passed: ${#production_manifests[@]} production crates"
