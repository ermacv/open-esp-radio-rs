#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

# Unsafe is an implementation detail of these audited foundations. The raw PAC
# is generated; the closed PAC is safe and is audited like every other safe
# driver crate. The handwritten leaves deny unsafe by default and reopen it
# only around individually justified operations.
generated_unsafe_leaf="driver/chips/esp32s31/pac-raw"
audited_unsafe_leaves=(
    "driver/common/dma"
    "driver/chips/esp32s31/bluetooth"
    "driver/chips/esp32s31/pac"
    "driver/chips/esp32s31/wifi/dma"
    "driver/adapters/esp-hal/esp32s31-radio-platform"
    "driver/adapters/embassy/esp32s31-platform"
    "driver/integration/esp32s31/embassy-wifi"
)

is_audited_unsafe_leaf() {
    local candidate="$1"
    local leaf
    for leaf in "${audited_unsafe_leaves[@]}"; do
        if [[ "$candidate" == "$leaf" ]]; then
            return 0
        fi
    done
    return 1
}

mapfile -t manifests < <(find driver -name Cargo.toml -not -path '*/target/*' | sort)
for manifest in "${manifests[@]}"; do
    crate_dir="${manifest%/Cargo.toml}"
    crate_root="$crate_dir/src/lib.rs"
    if [[ ! -f "$crate_root" ]]; then
        echo "driver package has no auditable library root: $manifest" >&2
        exit 1
    fi

    if [[ "$crate_dir" == "$generated_unsafe_leaf" ]]; then
        continue
    fi
    if is_audited_unsafe_leaf "$crate_dir"; then
        if ! rg -q '^#!\[deny\(unsafe_code\)\]$' "$crate_root"; then
            echo "audited unsafe leaf must deny unsafe by default: $crate_root" >&2
            exit 1
        fi
    elif ! rg -q '^#!\[forbid\(unsafe_code\)\]$' "$crate_root"; then
        echo "safe driver crate must forbid unsafe: $crate_root" >&2
        exit 1
    fi
done

# The restricted PAC is an implementation dependency of chip hardware
# backends. Crates above those backends cannot bypass that boundary through
# either ordinary or dev dependencies.
for manifest in "${manifests[@]}"; do
    case "$manifest" in
        driver/chips/esp32s31/pac/Cargo.toml|driver/chips/esp32s31/pac-raw/Cargo.toml|driver/chips/esp32s31/hal/Cargo.toml|driver/chips/esp32s31/bluetooth/Cargo.toml)
            continue
            ;;
    esac
    if rg -q 'open-esp-radio-esp32s31-pac([[:space:]]|[[:punct:]])' "$manifest"; then
        echo "driver crate bypasses HAL with a PAC dependency: $manifest" >&2
        exit 1
    fi
done

if rg -n 'test-register-catalog' driver/chips/esp32s31/pac; then
    echo "restricted PAC restored the removed external test register catalog" >&2
    exit 1
fi

# A crate- or module-wide allow would bypass review of the individual
# invariant. Every exception must remain attached to the smallest item or
# expression that needs it.
if rg -n -U '#!\[allow\([^]]*\bunsafe_code\b[^]]*\)\]' driver --glob '*.rs'; then
    echo "driver source contains a broad unsafe_code allowance" >&2
    exit 1
fi

mapfile -t handwritten_sources < <(
    rg --files \
        "${audited_unsafe_leaves[@]}" \
        --glob '*.rs' \
        --glob '!target/**' |
        sort
)
if ! perl -0777 -ne '
    while (/#\[allow\((.*?)\)\]/sg) {
        my $body = $1;
        next unless $body =~ /\bunsafe_code\b/;
        if ($body !~ /\breason\s*=\s*"[^"]+"/) {
            print STDERR "unsafe_code allowance without a reason: $ARGV\n";
            $failed = 1;
        }
    }
    END { exit($failed ? 1 : 0) }
' "${handwritten_sources[@]}"; then
    exit 1
fi

# Only the audited handwritten leaves may reopen the lint. Safe crates use
# `forbid`, but this textual check makes the whitelist violation fail before a
# potentially expensive target build.
mapfile -t all_allowing_sources < <(
    rg -l -U '#\[allow\([^]]*\bunsafe_code\b[^]]*\)\]' \
        driver \
        --glob '*.rs' \
        --glob '!chips/esp32s31/pac/**' || true
)
for source in "${all_allowing_sources[@]}"; do
    allowed=false
    for leaf in "${audited_unsafe_leaves[@]}"; do
        if [[ "$source" == "$leaf/"* ]]; then
            allowed=true
            break
        fi
    done
    if [[ "$allowed" != true ]]; then
        echo "unsafe_code allowance outside the audited leaves: $source" >&2
        exit 1
    fi
done

# PAC leaf-owner types are implementation details of the generated/restricted
# PAC and the matching chip hardware backends. The protocol-neutral
# RadioHardware root may cross the composition boundary by value; runtime,
# protocol, integration, and application-facing crates receive only finite
# operations or opaque lifecycle owners.
if rg -n '\b(ColdRadioRegisters|RadioRegisters|WifiColdRegisters|WifiRadioRegisters|BluetoothColdRegisters|BluetoothTaskRegisters|BluetoothInterruptSetup|BluetoothInterruptRegisters)\b' \
    driver \
    --glob '*.rs' \
    --glob '!driver/chips/esp32s31/pac/**' \
    --glob '!driver/chips/esp32s31/pac-raw/**' \
    --glob '!driver/chips/esp32s31/hal/**' \
    --glob '!driver/chips/esp32s31/bluetooth/**'
then
    echo "PAC leaf owner escaped above a chip hardware implementation boundary" >&2
    exit 1
fi

if rg -n '\bPhyRegisterAccess\b|\bphy_parts_mut\b|\bregisters_mut\b' \
    driver/chips/esp32s31 \
    --glob '*.rs'
then
    echo "removed powered-PHY compatibility surface was reintroduced" >&2
    exit 1
fi

# The PHY capability is intentionally opaque. A Deref implementation would
# silently restore every public PAC method as an operation available to PHY.
if rg -n 'impl([[:space:]]*<[^>]+>)?[[:space:]]+(core::ops::)?Deref(Mut)?[[:space:]]+for[[:space:]]+PhyHal' \
    driver/chips/esp32s31/hal \
    --glob '*.rs'
then
    echo "PhyHal must not dereference to the PAC owner" >&2
    exit 1
fi

# Cold authority is deliberately stronger than running authority, but the
# widening must remain explicit inside HAL. An implicit Deref recreates every
# runtime PAC method on the cold owner and hides the ownership boundary from
# call sites.
if rg -n 'impl([[:space:]]*<[^>]+>)?[[:space:]]+(core::ops::)?Deref(Mut)?[[:space:]]+for[[:space:]]+(ColdRadioRegisters|WifiColdRegisters|BluetoothColdRegisters)' \
    driver/chips/esp32s31/pac \
    --glob '*.rs'
then
    echo "a cold PAC owner must not dereference to a runtime PAC owner" >&2
    exit 1
fi

# Internal HAL wrappers must also keep PAC widening explicit. A private
# `Deref` would make every future PAC method silently available throughout the
# Wi-Fi MAC facade and invalidate the finite-operation review boundary.
if rg -n 'impl([[:space:]]*<[^>]+>)?[[:space:]]+(core::ops::)?Deref(Mut)?[[:space:]]+for[[:space:]]+WifiMacRegisters' \
    driver/chips/esp32s31/hal \
    --glob '*.rs'
then
    echo "WifiMacRegisters must not dereference to the PAC owner" >&2
    exit 1
fi

# Removed migration surfaces must stay removed. The `SplitPinned*` names are
# the canonical resource API and do not match these former aliases.
if rg -n '\b(PinnedResources|PinnedDevice|PinnedRadioRunner)\b|register_arena|esp32s31::registers' \
    driver \
    --glob '*.rs'
then
    echo "removed driver compatibility surface was reintroduced" >&2
    exit 1
fi

# Value snapshots may be re-exported, but a crate above the PAC must never
# publicly forward the unique register owner under either its original name
# or a module alias.
if rg -n 'pub use .*open_esp_radio_esp32s31_pac.*(RadioRegisters|ColdRadioRegisters|WifiRadioRegisters|WifiColdRegisters|BluetoothColdRegisters|BluetoothTaskRegisters|BluetoothInterruptSetup|BluetoothInterruptRegisters| as registers)' \
    driver \
    --glob '*.rs'
then
    echo "public PAC owner re-export was introduced" >&2
    exit 1
fi

# Hiding the dependency is insufficient if a public chip-hardware signature
# asks callers to provide one leaf PAC owner. The neutral `RadioHardware`
# composition root is intentionally allowed to cross by value; protocol leaf
# owners remain private and production callers receive finite capabilities.
if rg -n 'pub (unsafe )?fn [^(]+\([^)]*(RadioRegisters|ColdRadioRegisters|WifiRadioRegisters|WifiColdRegisters|BluetoothColdRegisters|BluetoothTaskRegisters|BluetoothInterruptSetup|BluetoothInterruptRegisters)|pub fn new\([^)]*(RadioRegisters|ColdRadioRegisters|WifiRadioRegisters|WifiColdRegisters|BluetoothColdRegisters|BluetoothTaskRegisters|BluetoothInterruptSetup|BluetoothInterruptRegisters)' \
    driver/chips/esp32s31/hal/src \
    driver/chips/esp32s31/bluetooth/src \
    --glob '*.rs'
then
    echo "chip hardware public API exposes a PAC leaf-owner parameter" >&2
    exit 1
fi

# Rust has no cross-crate friend visibility. The finite BTBB transaction is an
# unsafe hidden PAC SPI used by exactly one production lifecycle bridge and
# one isolated compiled-production probe. Safe downstream code must not be
# able to bypass the post-common-PHY typestate.
if ! rg -q 'pub unsafe fn initialize_baseband_v2_arg_one\(' \
    driver/chips/esp32s31/pac/src/bluetooth_baseband.rs
then
    echo "Bluetooth baseband PAC prerequisite is no longer compiler-enforced" >&2
    exit 1
fi
if ! rg -q 'pub\(crate\) unsafe fn initialize_baseband_v2\(' \
    driver/chips/esp32s31/bluetooth/src/resources.rs
then
    echo "Bluetooth lifecycle bridge no longer preserves the PAC prerequisite" >&2
    exit 1
fi
if ! rg -q 'pub unsafe fn initialize_bluetooth_baseband_v2\(' \
    driver/chips/esp32s31/pac/src/validation.rs \
    || ! rg -q 'pub unsafe fn initialize_baseband_v2\(' \
        driver/chips/esp32s31/bluetooth/src/validation.rs
then
    echo "Bluetooth validation probe lost its explicit common-PHY prerequisite" >&2
    exit 1
fi
if rg -n 'task\.into_cold\(interrupts\)' \
    driver/chips/esp32s31/pac/src/validation.rs
then
    echo "Bluetooth validation probe reconstructed cold ownership before teardown" >&2
    exit 1
fi
if rg -n '\.initialize_baseband_v2_arg_one\(' \
    driver \
    --glob '*.rs' \
    --glob '!driver/chips/esp32s31/pac/src/validation.rs' \
    --glob '!driver/chips/esp32s31/bluetooth/src/resources.rs'
then
    echo "Bluetooth baseband PAC SPI bypassed its lifecycle owner" >&2
    exit 1
fi

# The memory-list selector transaction proves only an encoding and exact MMIO
# sequence. Until controller list backing, active-state ownership and teardown
# are proved, its unsafe PAC SPI may be reached only through the two
# feature-gated compiled-production validation bridges.
memory_list_spi=driver/chips/esp32s31/pac/src/bluetooth_memory_lists.rs
memory_list_pac_validation=driver/chips/esp32s31/pac/src/validation.rs
memory_list_bt_validation=driver/chips/esp32s31/bluetooth/src/validation.rs
if ! rg -q 'pub unsafe fn program_memory_list_pointer\(' "$memory_list_spi"
then
    echo "Bluetooth memory-list PAC prerequisite is no longer explicit" >&2
    exit 1
fi
mapfile -t memory_list_spi_callers < <(
    rg -l '[.]program_memory_list_pointer[[:space:]]*\(' driver --glob '*.rs' | sort
)
if test "${#memory_list_spi_callers[@]}" -ne 1 \
    || test "${memory_list_spi_callers[0]}" != "$memory_list_pac_validation"
then
    echo "Bluetooth memory-list SPI escaped isolated PAC validation before lifecycle/teardown proof" >&2
    exit 1
fi
if ! rg -q 'pub unsafe fn program_bluetooth_memory_list_pointer\(' \
    "$memory_list_pac_validation" \
    || ! rg -q 'pub unsafe fn program_memory_list_pointer\(' \
        "$memory_list_bt_validation"
then
    echo "Bluetooth memory-list validation path lost an unsafe prerequisite" >&2
    exit 1
fi
mapfile -t memory_list_validation_callers < <(
    rg -l 'validation::program_bluetooth_memory_list_pointer[[:space:]]*\(' \
        driver \
        --glob '*.rs' \
        | sort
)
if test "${#memory_list_validation_callers[@]}" -ne 1 \
    || test "${memory_list_validation_callers[0]}" != "$memory_list_bt_validation"
then
    echo "Bluetooth memory-list validation bridge escaped the Bluetooth validation boundary" >&2
    exit 1
fi

# The complete BLE PHY register body is known, but its prerequisite lifecycle
# and rollback are not. Its crate-private unsafe PAC edge may therefore be
# reached only by the two feature-gated compiled-production validation bridges.
ble_phy_init_spi=driver/chips/esp32s31/pac/src/bluetooth_phy_init.rs
ble_phy_init_pac_validation=driver/chips/esp32s31/pac/src/validation.rs
ble_phy_init_bt_validation=driver/chips/esp32s31/bluetooth/src/validation.rs
if ! rg -q 'pub\(crate\) unsafe fn initialize_ble_phy_registers\(' \
    "$ble_phy_init_spi"
then
    echo "Bluetooth PHY init PAC prerequisite is no longer crate-private and unsafe" >&2
    exit 1
fi
mapfile -t ble_phy_init_spi_callers < <(
    rg -l '[.]initialize_ble_phy_registers[[:space:]]*\(' driver --glob '*.rs' | sort
)
if test "${#ble_phy_init_spi_callers[@]}" -ne 1 \
    || test "${ble_phy_init_spi_callers[0]}" != "$ble_phy_init_pac_validation"
then
    echo "Bluetooth PHY init SPI escaped isolated PAC validation before lifecycle/teardown proof" >&2
    exit 1
fi
if ! rg -q 'pub unsafe fn initialize_bluetooth_phy_registers\(' \
    "$ble_phy_init_pac_validation" \
    || ! rg -q 'pub unsafe fn initialize_phy_registers\(' \
        "$ble_phy_init_bt_validation"
then
    echo "Bluetooth PHY init validation path lost an unsafe prerequisite" >&2
    exit 1
fi
mapfile -t ble_phy_init_validation_callers < <(
    rg -l 'validation::initialize_bluetooth_phy_registers[[:space:]]*\(' \
        driver \
        --glob '*.rs' \
        | sort
)
if test "${#ble_phy_init_validation_callers[@]}" -ne 1 \
    || test "${ble_phy_init_validation_callers[0]}" != "$ble_phy_init_bt_validation"
then
    echo "Bluetooth PHY init bridge escaped the Bluetooth validation boundary" >&2
    exit 1
fi

# Powered/partial Bluetooth PHY states are fail-stop until the complete
# last-owner teardown exists. Storing the ordinary platform owner directly, or
# extracting the armed owner without that transaction, would reintroduce an
# implicit clock-gating path through its Drop implementation.
if rg -n '(^|[[:space:]])_?platform:[[:space:]]*P,' \
    driver/chips/esp32s31/bluetooth/src/phy.rs \
    driver/chips/esp32s31/bluetooth/src/baseband.rs \
    driver/chips/esp32s31/bluetooth/src/scheduler.rs \
    || rg -n 'ManuallyDrop::(into_inner|take)' \
        driver/chips/esp32s31/bluetooth/src \
        --glob '*.rs'
then
    echo "powered Bluetooth state can implicitly release its platform owner" >&2
    exit 1
fi

# The pinned controller lifecycle performs task/controller initialization
# between clock setup and the common-PHY enable edge. The public API must stop
# at the fact-bounded scheduler prefix until those intervening stages exist;
# restoring the former direct clocks-to-PHY method would encode a false order.
if rg -n 'pub async fn initialize_common_phy' \
    driver/chips/esp32s31/bluetooth/src \
    --glob '*.rs'
then
    echo "Bluetooth public API bypasses incomplete controller init before common PHY" >&2
    exit 1
fi
if ! rg -q 'pub fn clear_scheduler_table_low_bits\(self\).*BluetoothSchedulerTableLowBitsCleared' \
    driver/chips/esp32s31/bluetooth/src/scheduler.rs
then
    echo "Bluetooth clocked owner lost its fact-bounded scheduler frontier" >&2
    exit 1
fi
if rg -n 'initialize_scheduler_table' \
    driver/chips/esp32s31/bluetooth/src \
    driver/chips/esp32s31/pac/src \
    --glob '*.rs'
then
    echo "overclaiming Bluetooth scheduler compatibility name was restored" >&2
    exit 1
fi

echo "driver unsafe boundary audit passed"
echo "generated_leaf=$generated_unsafe_leaf"
printf 'audited_leaf=%s\n' "${audited_unsafe_leaves[@]}"
