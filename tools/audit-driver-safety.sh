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
    "driver/chips/esp32s31/hal"
    "driver/chips/esp32s31/pac"
    "driver/chips/esp32s31/phy"
    "driver/chips/esp32s31/ieee802154/dma"
    "driver/chips/esp32s31/ieee802154/runtime"
    "driver/chips/esp32s31/wifi/dma"
    "driver/adapters/esp-hal/esp32s31-radio-platform"
    "driver/adapters/embassy/esp32s31-platform"
    "driver/integration/esp32s31/bluetooth"
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

# The restricted PAC is an implementation dependency of exact chip hardware
# backends. Crates above those backends cannot bypass that boundary through
# either ordinary or dev dependencies.
for manifest in "${manifests[@]}"; do
    case "$manifest" in
        driver/chips/esp32s31/pac/Cargo.toml|driver/chips/esp32s31/pac-raw/Cargo.toml|driver/chips/esp32s31/hal/Cargo.toml|driver/chips/esp32s31/bluetooth/Cargo.toml|driver/chips/esp32s31/ieee802154/irq/Cargo.toml|driver/chips/esp32s31/ieee802154/runtime/Cargo.toml|driver/adapters/esp-hal/esp32s31-ieee802154/Cargo.toml)
            continue
            ;;
    esac
    if rg -q 'open-esp-radio-esp32s31-pac([[:space:]]|[[:punct:]])' "$manifest"; then
        echo "driver crate bypasses an exact chip hardware boundary with a PAC dependency: $manifest" >&2
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

# Rust has no cross-crate friend visibility. The affine runtime is the one
# composition point which owns both the accepted terminal actor transition and
# the exact retained DMA resources, so it mints the DMA terminal proof through
# one audited unsafe SPI. Keep both the unsafe block and its call graph finite.
terminal_evidence_spi="driver/chips/esp32s31/ieee802154/dma/src/terminal.rs"
terminal_evidence_runtime="driver/chips/esp32s31/ieee802154/runtime/src/lib.rs"
if ! rg -q 'pub unsafe fn from_accepted_terminal_batch\(' "$terminal_evidence_spi"
then
    echo "IEEE 802.15.4 DMA terminal proof SPI is no longer explicitly unsafe" >&2
    exit 1
fi
mapfile -t terminal_evidence_callers < <(
    rg -l 'DmaTerminalEvidence::from_accepted_terminal_batch[[:space:]]*\(' \
        driver \
        --glob '*.rs' \
        | sort
)
if test "${#terminal_evidence_callers[@]}" -ne 1 \
    || test "${terminal_evidence_callers[0]}" != "$terminal_evidence_runtime"
then
    echo "IEEE 802.15.4 DMA terminal proof escaped its affine runtime boundary" >&2
    exit 1
fi
mapfile -t runtime_unsafe_blocks < <(
    rg -n 'unsafe[[:space:]]*\{' "$terminal_evidence_runtime" || true
)
if test "${#runtime_unsafe_blocks[@]}" -ne 1 \
    || [[ "${runtime_unsafe_blocks[0]}" != *'DmaTerminalEvidence::from_accepted_terminal_batch()'* ]]
then
    echo "IEEE 802.15.4 runtime unsafe surface is not the single terminal proof mint" >&2
    exit 1
fi

# Terminal common-PHY registration and the retained IEEE hardware owner meet
# at one affine PHY boundary. Audit the corresponding PAC prerequisite mint in
# the same way as the terminal DMA proof above.
ieee_timing_spi="driver/chips/esp32s31/pac/src/ieee802154_timing.rs"
ieee_timing_boundary="driver/chips/esp32s31/phy/src/ieee802154_timing_boundary.rs"
if ! rg -q 'pub unsafe fn from_terminal_common_phy\(' "$ieee_timing_spi"
then
    echo "IEEE 802.15.4 timing prerequisite SPI is no longer explicitly unsafe" >&2
    exit 1
fi
mapfile -t ieee_timing_callers < <(
    rg -l 'Ieee802154TimingPrerequisite::from_terminal_common_phy[[:space:]]*\(' \
        driver \
        --glob '*.rs' \
        --glob '!driver/chips/esp32s31/pac/**' \
        | sort
)
if test "${#ieee_timing_callers[@]}" -ne 1 \
    || test "${ieee_timing_callers[0]}" != "$ieee_timing_boundary"
then
    echo "IEEE 802.15.4 timing prerequisite escaped its registered-PHY boundary" >&2
    exit 1
fi
mapfile -t phy_unsafe_blocks < <(
    rg -n 'unsafe[[:space:]]*\{' driver/chips/esp32s31/phy --glob '*.rs' || true
)
if test "${#phy_unsafe_blocks[@]}" -ne 1 \
    || [[ "${phy_unsafe_blocks[0]}" != *'Ieee802154TimingPrerequisite::from_terminal_common_phy('* ]]
then
    echo "ESP32-S31 PHY unsafe surface is not the single IEEE timing proof mint" >&2
    exit 1
fi

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

# The Bluetooth controller has one Wi-Fi-style cold aggregate. Platform and
# radio ownership may be separated only inside the lifecycle crate; rollback
# and reversible shutdown must return the same aggregate, never a tuple.
bluetooth_resources=driver/chips/esp32s31/bluetooth/src/resources.rs
bluetooth_clock=driver/chips/esp32s31/bluetooth/src/clock.rs
bluetooth_controller_root=driver/chips/esp32s31/bluetooth/src/lib.rs
if ! rg -q 'pub struct BluetoothStopped<P>' "$bluetooth_resources" \
    || ! rg -q 'pub fn from_hardware\(platform: P, hardware: RadioHardware\)' "$bluetooth_resources" \
    || ! rg -q 'pub fn release\(self\) -> \(P, RadioHardware\)' "$bluetooth_resources" \
    || ! rg -q 'pub fn disable_clocks\(mut self\) -> BluetoothStopped<P>' "$bluetooth_clock" \
    || ! rg -q 'pub fn into_stopped\(self\) -> BluetoothStopped<P>' "$bluetooth_clock"
then
    echo "Bluetooth stopped ownership frontier is incomplete" >&2
    exit 1
fi
if rg -q 'BluetoothPhysicalResources|^[[:space:]]*pub fn from_radio_hardware|^[[:space:]]*pub fn into_parts|pub fn enable_clocks<P:' \
        "$bluetooth_resources" "$bluetooth_clock"
then
    echo "legacy split Bluetooth ownership API was reintroduced" >&2
    exit 1
fi
if rg -q 'pub use (baseband|common_phy_state|phy)::|Bluetooth(Baseband|Phy)Initialized' \
        "$bluetooth_controller_root"
then
    echo "disconnected Bluetooth PHY/baseband state was re-exported as production API" >&2
    exit 1
fi

# PAC and HAL own affine register capabilities and finite transactions only.
# Cross-layer PHY/controller/route facts must not be represented by public
# `assume_satisfied` tokens: those values were forgeable and inverted the
# dependency direction. The controller lifecycle and isolated validation
# probes retain the real prerequisite owners at explicit unsafe boundaries.
baseband_spi=driver/chips/esp32s31/pac/src/bluetooth_baseband.rs
controller_init_spi=driver/chips/esp32s31/pac/src/bluetooth_controller_hal_init.rs
interrupt_spi=driver/chips/esp32s31/pac/src/bluetooth_interrupt.rs
modem_lp_timer_spi=driver/chips/esp32s31/pac/src/bluetooth_modem_lp_timer.rs
modem_lp_timer_queue=driver/chips/esp32s31/bluetooth/src/modem_lp_timer_queue.rs
nrt_interrupt=driver/chips/esp32s31/bluetooth/src/nrt_interrupt.rs
scheduler_disable_spi=driver/chips/esp32s31/pac/src/bluetooth_scheduler_stop.rs
bluetooth_hal=driver/chips/esp32s31/hal/src/bluetooth.rs
bluetooth_lifecycle=driver/chips/esp32s31/bluetooth/src/resources.rs
bluetooth_runtime_resources=driver/chips/esp32s31/bluetooth/src/runtime_resources.rs
bluetooth_scheduler=driver/chips/esp32s31/bluetooth/src/scheduler.rs
bluetooth_embassy=driver/adapters/embassy/esp32s31-bluetooth/src/lib.rs
bluetooth_validation=driver/chips/esp32s31/bluetooth/src/validation.rs
pac_validation=driver/chips/esp32s31/pac/src/validation.rs

if rg -q 'Bluetooth(BasebandInitialization|ControllerHalInit|InterruptOutputPreparation|ModemLpTimerInitialization|SchedulerDisable)Prerequisite|BluetoothControllerSchedulerDisablePrerequisite|assume_satisfied' \
        "$baseband_spi" "$controller_init_spi" "$interrupt_spi" \
        "$modem_lp_timer_spi" "$scheduler_disable_spi" "$bluetooth_hal" \
        "$bluetooth_lifecycle" "$bluetooth_validation" "$pac_validation"
then
    echo "forgeable Bluetooth cross-layer prerequisite token was reintroduced" >&2
    exit 1
fi
if ! rg -q 'pub struct BluetoothControllerRuntimeResources' "$bluetooth_runtime_resources" \
    || ! rg -q 'pub struct BluetoothControllerInterruptRuntime' "$bluetooth_runtime_resources" \
    || ! rg -q 'pub struct BluetoothControllerTaskRuntime' "$bluetooth_runtime_resources" \
    || ! rg -q 'pub fn split' "$bluetooth_runtime_resources" \
    || ! rg -q 'pub struct BluetoothSchedulerRuntimeResourcesBound' "$bluetooth_scheduler" \
    || ! rg -q 'pub fn bind_runtime_resources' "$bluetooth_scheduler" \
    || ! rg -q 'pub fn split_runtime' "$bluetooth_scheduler" \
    || rg -q 'derive.*(Copy|Clone)' "$bluetooth_runtime_resources"
then
    echo "Bluetooth no-RTOS runtime aggregate is not an affine scheduler-prefix owner" >&2
    exit 1
fi
if ! rg -q 'pub struct EmbassyBluetoothRuntimeWakers' "$bluetooth_embassy" \
    || rg -q 'EmbassyBluetoothWakeResources|Bluetooth(SchedulerWakeCell|SchedulerLockModifyEventCell|ModemLpTimerEventCell)' "$bluetooth_embassy" \
    || rg -q 'worker: &mut BluetoothSchedulerLockModifyWorker' "$bluetooth_embassy"
then
    echo "Bluetooth Embassy adapter reintroduced duplicate epoch state or an external worker" >&2
    exit 1
fi
mapfile -t bluetooth_hal_unsafe_entries < <(
    rg -o 'pub unsafe fn [a-z0-9_]+' "$bluetooth_hal" | sort
)
if test "${#bluetooth_hal_unsafe_entries[@]}" -ne 5 \
    || test "${bluetooth_hal_unsafe_entries[0]}" != "pub unsafe fn begin" \
    || test "${bluetooth_hal_unsafe_entries[1]}" != "pub unsafe fn initialize_baseband_v2_arg_one" \
    || test "${bluetooth_hal_unsafe_entries[2]}" != "pub unsafe fn initialize_controller_hal_transaction" \
    || test "${bluetooth_hal_unsafe_entries[3]}" != "pub unsafe fn prepare_controller_output" \
    || test "${bluetooth_hal_unsafe_entries[4]}" != "pub unsafe fn prepare_modem_lp_timer_registers" \
    || rg -q 'unsafe[[:space:]]*\{' "$bluetooth_hal"
then
    echo "Bluetooth HAL unsafe surface is not the closed lifecycle-boundary set" >&2
    exit 1
fi

if ! rg -q 'pub fn initialize_baseband_v2_arg_one\(&mut self, gain_parameter: u8\)' "$baseband_spi" \
    || ! rg -q 'pub unsafe fn initialize_baseband_v2_arg_one\(&mut self, gain_parameter: u8\)' "$bluetooth_hal" \
    || ! rg -q 'pub\(crate\) unsafe fn initialize_baseband_v2\(' "$bluetooth_lifecycle"
then
    echo "Bluetooth baseband PAC/HAL/lifecycle boundary is incomplete" >&2
    exit 1
fi
mapfile -t baseband_transaction_callers < <(
    rg -l '[.]initialize_baseband_v2_arg_one[[:space:]]*\(' driver --glob '*.rs' | sort
)
if test "${#baseband_transaction_callers[@]}" -ne 3 \
    || test "${baseband_transaction_callers[0]}" != "$bluetooth_lifecycle" \
    || test "${baseband_transaction_callers[1]}" != "$bluetooth_hal" \
    || test "${baseband_transaction_callers[2]}" != "$pac_validation"
then
    echo "Bluetooth baseband transaction escaped HAL or isolated PAC validation" >&2
    exit 1
fi
if ! rg -q 'pub unsafe fn initialize_bluetooth_baseband_v2\(' "$pac_validation" \
    || ! rg -q 'pub unsafe fn initialize_baseband_v2\(' "$bluetooth_validation"
then
    echo "Bluetooth validation probe lost its explicit common-PHY prerequisite" >&2
    exit 1
fi

if ! rg -q 'pub fn initialize_controller_hal\(&mut self, config: BluetoothControllerHalInitConfig\)' "$controller_init_spi" \
    || ! rg -q 'pub unsafe fn initialize_controller_hal_transaction\(' "$bluetooth_hal" \
    || ! rg -q 'pub\(crate\) unsafe fn initialize_controller_hal\(' "$bluetooth_lifecycle"
then
    echo "Bluetooth controller HAL-init PAC/HAL/lifecycle boundary is incomplete" >&2
    exit 1
fi

if ! rg -q 'pub fn prepare_controller_output\(self\)' "$interrupt_spi" \
    || ! rg -q 'pub unsafe fn prepare_controller_output\(self\)' "$bluetooth_hal" \
    || ! rg -q 'pub unsafe fn prepare_primary_interrupt_output\(' "$bluetooth_validation"
then
    echo "Bluetooth interrupt-output PAC/HAL/lifecycle boundary is incomplete" >&2
    exit 1
fi
mapfile -t interrupt_prepare_transaction_callers < <(
    rg -l '[.]prepare_controller_output[[:space:]]*\(' driver --glob '*.rs' | sort
)
if test "${#interrupt_prepare_transaction_callers[@]}" -ne 2 \
    || test "${interrupt_prepare_transaction_callers[0]}" != "$bluetooth_validation" \
    || test "${interrupt_prepare_transaction_callers[1]}" != "$bluetooth_hal"
then
    echo "Bluetooth interrupt-output transaction escaped HAL or isolated validation" >&2
    exit 1
fi
if ! rg -q 'pub fn capture_nrt_and_acknowledge\(' "$interrupt_spi" \
    || ! rg -q 'pub fn capture_nrt_and_acknowledge\(' "$bluetooth_hal" \
    || ! rg -q 'pub fn step_nrt_default_interrupt\(' "$nrt_interrupt" \
    || ! rg -q 'pub struct BluetoothNrtDefaultInterruptEpoch' "$nrt_interrupt"
then
    echo "Bluetooth default-profile NRT PAC/HAL/controller path is incomplete" >&2
    exit 1
fi
mapfile -t nrt_capture_callers < <(
    rg -l '[.]capture_nrt_and_acknowledge[[:space:]]*\(' driver --glob '*.rs' | sort
)
if test "${#nrt_capture_callers[@]}" -ne 3 \
    || test "${nrt_capture_callers[0]}" != "$nrt_interrupt" \
    || test "${nrt_capture_callers[1]}" != "$bluetooth_validation" \
    || test "${nrt_capture_callers[2]}" != "$bluetooth_hal"
then
    echo "Bluetooth NRT PAC transaction escaped HAL/controller or isolated validation" >&2
    exit 1
fi

if ! rg -q 'pub fn prepare_modem_lp_timer_registers\(self\)' "$modem_lp_timer_spi" \
    || ! rg -q 'pub unsafe fn prepare_modem_lp_timer_registers\(' "$bluetooth_hal" \
    || ! rg -q 'pub struct BluetoothModemLpTimerInterruptReady' "$modem_lp_timer_spi" \
    || ! rg -q 'pub struct BluetoothModemLpTimerHandlerPending' "$modem_lp_timer_spi" \
    || ! rg -q 'pub struct BluetoothModemLpTimerHandlerRegisterObservation' "$modem_lp_timer_spi" \
    || ! rg -q 'pub struct BluetoothModemLpTimerSoftwarePending' "$modem_lp_timer_spi" \
    || ! rg -q 'pub struct BluetoothModemLpTimerEpoch' "$modem_lp_timer_spi" \
    || ! rg -q 'pub struct BluetoothModemLpTimerCounterObservation' "$modem_lp_timer_spi" \
    || ! rg -q 'pub enum BluetoothModemLpTimerCompareDisposition' "$modem_lp_timer_spi" \
    || ! rg -q 'pub fn step_registers\(' "$modem_lp_timer_spi" \
    || ! rg -q 'pub fn sample_counter\(' "$modem_lp_timer_spi" \
    || ! rg -q 'pub fn program_compare\(' "$modem_lp_timer_spi" \
    || ! rg -q 'pub fn complete_software\(' "$modem_lp_timer_spi" \
    || ! rg -q 'pub fn stage_for_interrupt\(' "$modem_lp_timer_spi" \
    || ! rg -q 'pub struct BluetoothModemLpTimerInterruptReadyOwner' "$bluetooth_hal" \
    || ! rg -q 'pub struct BluetoothModemLpTimerHandlerPendingOwner' "$bluetooth_hal" \
    || ! rg -q 'pub struct BluetoothModemLpTimerSoftwarePendingOwner' "$bluetooth_hal" \
    || ! rg -q 'pub fn step_registers\(' "$bluetooth_hal" \
    || ! rg -q 'pub fn sample_counter\(' "$bluetooth_hal" \
    || ! rg -q 'pub fn program_compare\(' "$bluetooth_hal" \
    || ! rg -q 'pub fn complete_software\(' "$bluetooth_hal" \
    || ! rg -q 'pub fn stage_for_interrupt\(' "$bluetooth_hal" \
    || ! rg -q 'pub unsafe fn prepare_modem_lp_timer_registers\(' "$bluetooth_validation"
then
    echo "Bluetooth modem LP-timer affine prepare/ISR path is incomplete" >&2
    exit 1
fi
mapfile -t modem_lp_timer_transaction_callers < <(
    rg -l '[.]prepare_modem_lp_timer_registers[[:space:]]*\(' driver --glob '*.rs' | sort
)
if test "${#modem_lp_timer_transaction_callers[@]}" -ne 2 \
    || test "${modem_lp_timer_transaction_callers[0]}" != "$bluetooth_validation" \
    || test "${modem_lp_timer_transaction_callers[1]}" != "$bluetooth_hal"
then
    echo "Bluetooth modem LP-timer transaction escaped HAL or isolated validation" >&2
    exit 1
fi
if rg -q 'BluetoothModemLpTimerInterruptEvent|assume_pending' \
    "$modem_lp_timer_spi" "$bluetooth_hal" "$modem_lp_timer_queue"
then
    echo "Bluetooth modem LP-timer retained a forgeable ISR event token" >&2
    exit 1
fi
if rg -q 'pub fn clear_scheduler_reference\(' "$bluetooth_hal" \
    || rg -q 'BluetoothSchedulerReferenceCleared' "$bluetooth_hal"
then
    echo "Bluetooth HAL exposed scheduler-reference clear before selector-6 ownership exists" >&2
    exit 1
fi
mapfile -t modem_lp_timer_completion_callers < <(
    rg -l '[.]complete_software[[:space:]]*\(' driver --glob '*.rs' | sort
)
if ! rg -q 'pub struct BluetoothModemLpTimerSoftwareWork' "$modem_lp_timer_queue" \
    || ! rg -q 'pub struct BluetoothModemLpTimerExpirationPending' "$modem_lp_timer_queue" \
    || ! rg -q 'pub struct BluetoothModemLpTimerEventCell' "$modem_lp_timer_queue" \
    || ! rg -q 'pub fn publish' "$modem_lp_timer_queue" \
    || test "${#modem_lp_timer_completion_callers[@]}" -ne 2 \
    || test "${modem_lp_timer_completion_callers[0]}" != "$modem_lp_timer_queue" \
    || test "${modem_lp_timer_completion_callers[1]}" != "$bluetooth_hal"
then
    echo "Bluetooth modem LP-timer final rearm escaped its publication-gated controller owner" >&2
    exit 1
fi

if ! rg -q 'pub fn begin_scheduler_disable\(' "$scheduler_disable_spi" \
    || ! rg -q 'pub unsafe fn begin\(' "$bluetooth_hal"
then
    echo "Bluetooth scheduler-disable PAC/HAL boundary is incomplete" >&2
    exit 1
fi
mapfile -t scheduler_disable_transaction_callers < <(
    rg -l '[.]begin_scheduler_disable[[:space:]]*\(' driver --glob '*.rs' | sort
)
if test "${#scheduler_disable_transaction_callers[@]}" -ne 3 \
    || test "${scheduler_disable_transaction_callers[0]}" != "$bluetooth_hal" \
    || test "${scheduler_disable_transaction_callers[1]}" != "$scheduler_disable_spi" \
    || test "${scheduler_disable_transaction_callers[2]}" != "$pac_validation"
then
    echo "Bluetooth scheduler-disable transaction escaped HAL or isolated validation" >&2
    exit 1
fi
if ! rg -q 'pub unsafe fn disable_bluetooth_scheduler_and_sample_once\(' "$pac_validation" \
    || ! rg -q 'pub unsafe fn disable_scheduler_and_sample_once\(' "$bluetooth_validation"
then
    echo "Bluetooth scheduler-disable validation boundary is incomplete" >&2
    exit 1
fi

if rg -n 'task\.into_cold\(interrupts\)' "$pac_validation"
then
    echo "Bluetooth validation probe reconstructed cold ownership before teardown" >&2
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
