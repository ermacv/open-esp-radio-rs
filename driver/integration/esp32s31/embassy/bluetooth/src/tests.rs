use open_esp_radio_esp32s31_bluetooth::{
    BluetoothDtmDefaultTxPowerDbm, BluetoothDtmRuntimeConfig,
    BluetoothLegacyAdvertisingDefaultTxPowerDbm, BluetoothPassiveScanRuntimeConfig,
    BluetoothPeripheralConnectionRuntimeConfig,
};
use open_esp_radio_esp32s31_bluetooth_memory::{
    BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH, BluetoothBlePhyEngineBindError,
    BluetoothBlePhyEngineModelAddress, BluetoothBlePhyEngineStorage,
    BluetoothDirectionFindingWorkspaceModelAddress, BluetoothDtmMemoryGraphBindError,
    BluetoothDtmMemoryGraphModelAddress, BluetoothDtmMemoryGraphStorage,
    BluetoothDtmSchedulerAllocationConfig, BluetoothLegacyAdvertisingMemoryGraphModelAddress,
    BluetoothLegacyConnectableAdvertisingMemoryGraphModelAddress,
    BluetoothNonScanningRxMemoryModelAddress, BluetoothPassiveScanDefaultTxPowerDbm,
    BluetoothPassiveScanMemoryGraphModelAddress, BluetoothPassiveScanSchedulerAllocationConfig,
    BluetoothPeripheralConnectionDefaultTxPowerDbm,
    BluetoothPeripheralConnectionMemoryGraphModelAddress,
};

use super::{
    Esp32s31BluetoothBlePhyMemory, Esp32s31BluetoothBlePhyMemoryClaimError,
    Esp32s31BluetoothDirectionFindingMemory, Esp32s31BluetoothDirectionFindingMemoryClaimError,
    Esp32s31BluetoothDtmMemory, Esp32s31BluetoothDtmMemoryClaimError,
    Esp32s31BluetoothLegacyAdvertisingMemory, Esp32s31BluetoothLegacyAdvertisingMemoryClaimError,
    Esp32s31BluetoothLegacyConnectableAdvertisingMemory,
    Esp32s31BluetoothLegacyConnectableAdvertisingMemoryClaimError,
    Esp32s31BluetoothPassiveScanMemory, Esp32s31BluetoothPassiveScanMemoryClaimError,
    Esp32s31BluetoothPeripheralConnectionMemory,
    Esp32s31BluetoothPeripheralConnectionMemoryClaimError,
};

const fn runtime_config() -> BluetoothDtmRuntimeConfig {
    BluetoothDtmRuntimeConfig::new(
        BluetoothDtmSchedulerAllocationConfig::new(2, 3, 4),
        BluetoothDtmDefaultTxPowerDbm::new(6),
    )
}

#[test]
fn model_ble_phy_arena_is_claimed_once_as_one_bound_graph() {
    static MEMORY: Esp32s31BluetoothBlePhyMemory = Esp32s31BluetoothBlePhyMemory::new();

    let base =
        BluetoothBlePhyEngineModelAddress::new(0x2f00_2000).expect("model base is encodable");
    let owner = MEMORY
        .claim_model(base)
        .expect("fresh model arena binds once");
    let (start, end) = owner.binding().range();
    assert_eq!(start, 0x2f00_2000);
    assert_eq!(
        end - start,
        size_of::<BluetoothBlePhyEngineStorage>() as u32
    );
    assert!(matches!(
        MEMORY.claim_model(base),
        Err(Esp32s31BluetoothBlePhyMemoryClaimError::InUse)
    ));
}

#[test]
fn model_direction_finding_workspace_is_claimed_once() {
    static MEMORY: Esp32s31BluetoothDirectionFindingMemory =
        Esp32s31BluetoothDirectionFindingMemory::new();
    let base = BluetoothDirectionFindingWorkspaceModelAddress::new(0x2f00_3000)
        .expect("model base is encodable");

    let owner = MEMORY
        .claim_model(base)
        .expect("fresh workspace binds once");

    assert!(owner.is_disabled_baseline_initialized());
    assert!(matches!(
        MEMORY.claim_model(base),
        Err(Esp32s31BluetoothDirectionFindingMemoryClaimError::InUse)
    ));
}

#[test]
fn model_legacy_advertising_arena_is_claimed_once() {
    static MEMORY: Esp32s31BluetoothLegacyAdvertisingMemory =
        Esp32s31BluetoothLegacyAdvertisingMemory::new();
    let base = BluetoothLegacyAdvertisingMemoryGraphModelAddress::new(0x2f00_6000)
        .expect("model base is encodable");
    let runtime = MEMORY
        .claim_model(base, BluetoothLegacyAdvertisingDefaultTxPowerDbm::new(6))
        .expect("fresh advertising arena binds once");
    assert!(runtime.event_is_idle());
    assert_eq!(runtime.default_tx_power_dbm().dbm(), 6);
    assert!(matches!(
        MEMORY.claim_model(base, BluetoothLegacyAdvertisingDefaultTxPowerDbm::new(6)),
        Err(Esp32s31BluetoothLegacyAdvertisingMemoryClaimError::InUse)
    ));
}

#[test]
fn model_passive_scanner_arena_is_claimed_once() {
    static MEMORY: Esp32s31BluetoothPassiveScanMemory = Esp32s31BluetoothPassiveScanMemory::new();
    let base = BluetoothPassiveScanMemoryGraphModelAddress::new(0x2f00_8000)
        .expect("model base is encodable");
    let config = BluetoothPassiveScanRuntimeConfig::new(
        BluetoothPassiveScanSchedulerAllocationConfig::new(2, 3)
            .expect("the product limits fit the scanner graph"),
        BluetoothPassiveScanDefaultTxPowerDbm::new(6),
    );
    let runtime = MEMORY
        .claim_model(base, config)
        .expect("fresh scanner arena binds once");
    assert!(runtime.event_is_idle());
    assert_eq!(runtime.config(), config);
    assert!(matches!(
        MEMORY.claim_model(base, config),
        Err(Esp32s31BluetoothPassiveScanMemoryClaimError::InUse)
    ));
}

#[test]
fn model_peripheral_connection_arena_is_claimed_once() {
    static MEMORY: Esp32s31BluetoothPeripheralConnectionMemory =
        Esp32s31BluetoothPeripheralConnectionMemory::new();
    let base = BluetoothPeripheralConnectionMemoryGraphModelAddress::new(0x2f00_a000)
        .expect("model base is encodable");
    let receive_base = BluetoothNonScanningRxMemoryModelAddress::new(0x2f00_b000)
        .expect("model receive base is encodable");
    let config = BluetoothPeripheralConnectionRuntimeConfig::new(
        BluetoothPeripheralConnectionDefaultTxPowerDbm::new(6),
    );
    let runtime = MEMORY
        .claim_model(base, receive_base, config)
        .expect("fresh connection arena binds once");

    assert!(runtime.allocation_is_idle());
    assert_eq!(runtime.config(), config);
    assert!(matches!(
        MEMORY.claim_model(base, receive_base, config),
        Err(Esp32s31BluetoothPeripheralConnectionMemoryClaimError::InUse)
    ));
}

#[test]
fn model_connectable_advertising_arena_is_claimed_once() {
    static MEMORY: Esp32s31BluetoothLegacyConnectableAdvertisingMemory =
        Esp32s31BluetoothLegacyConnectableAdvertisingMemory::new();
    let base = BluetoothLegacyConnectableAdvertisingMemoryGraphModelAddress::new(0x2f00_c000)
        .expect("model base is encodable");
    let runtime = MEMORY
        .claim_model(base, BluetoothLegacyAdvertisingDefaultTxPowerDbm::new(6))
        .expect("fresh connectable-advertising arena binds once");

    assert!(runtime.event_is_idle());
    assert_eq!(runtime.default_tx_power_dbm().dbm(), 6);
    assert!(matches!(
        MEMORY.claim_model(base, BluetoothLegacyAdvertisingDefaultTxPowerDbm::new(6)),
        Err(Esp32s31BluetoothLegacyConnectableAdvertisingMemoryClaimError::InUse)
    ));
}

#[test]
fn ble_phy_placement_failure_is_sticky_and_retains_the_allocation() {
    static MEMORY: Esp32s31BluetoothBlePhyMemory = Esp32s31BluetoothBlePhyMemory::new();

    let crossing = BluetoothBlePhyEngineModelAddress::new(
        BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH - size_of::<BluetoothBlePhyEngineStorage>() as u32
            + 4,
    )
    .expect("crossing model base is still encodable");
    let failure = match MEMORY.claim_model(crossing) {
        Err(Esp32s31BluetoothBlePhyMemoryClaimError::Placement(failure)) => failure,
        Err(Esp32s31BluetoothBlePhyMemoryClaimError::InUse) => {
            panic!("fresh arena cannot already be in use")
        }
        Ok(_) => panic!("crossing placement must fail closed"),
    };
    assert_eq!(
        failure.error(),
        BluetoothBlePhyEngineBindError::ExtentOutsidePhysicalSram
    );
    let (_storage, error) = failure.into_parts();
    assert_eq!(
        error,
        BluetoothBlePhyEngineBindError::ExtentOutsidePhysicalSram
    );

    let valid = BluetoothBlePhyEngineModelAddress::new(0x2f00_2000)
        .expect("valid retry address is encodable");
    assert!(matches!(
        MEMORY.claim_model(valid),
        Err(Esp32s31BluetoothBlePhyMemoryClaimError::InUse)
    ));
}

#[test]
fn model_arena_is_claimed_once_as_one_bound_graph() {
    static MEMORY: Esp32s31BluetoothDtmMemory = Esp32s31BluetoothDtmMemory::new();

    let base =
        BluetoothDtmMemoryGraphModelAddress::new(0x2f00_1000).expect("model base is encodable");
    let runtime = MEMORY
        .claim_model(base, runtime_config())
        .expect("fresh model arena binds once");
    assert_eq!(runtime.config(), runtime_config());
    assert_eq!(runtime.default_tx_power_dbm().dbm(), 6);
    assert!(runtime.session_is_idle());
    assert!(matches!(
        MEMORY.claim_model(base, runtime_config()),
        Err(Esp32s31BluetoothDtmMemoryClaimError::InUse)
    ));
}

#[test]
fn placement_failure_is_sticky_and_retains_the_allocation() {
    static MEMORY: Esp32s31BluetoothDtmMemory = Esp32s31BluetoothDtmMemory::new();

    let crossing = BluetoothDtmMemoryGraphModelAddress::new(
        BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH
            - size_of::<BluetoothDtmMemoryGraphStorage>() as u32
            + 4,
    )
    .expect("crossing model base is still encodable");
    let failure = match MEMORY.claim_model(crossing, runtime_config()) {
        Err(Esp32s31BluetoothDtmMemoryClaimError::Placement(failure)) => failure,
        Err(Esp32s31BluetoothDtmMemoryClaimError::InUse) => {
            panic!("fresh arena cannot already be in use")
        }
        Ok(_) => panic!("crossing placement must fail closed"),
    };
    assert_eq!(
        failure.error(),
        BluetoothDtmMemoryGraphBindError::ExtentOutsidePhysicalSram
    );
    let (_storage, error) = failure.into_parts();
    assert_eq!(
        error,
        BluetoothDtmMemoryGraphBindError::ExtentOutsidePhysicalSram
    );

    let valid = BluetoothDtmMemoryGraphModelAddress::new(0x2f00_1000)
        .expect("valid retry address is encodable");
    assert!(matches!(
        MEMORY.claim_model(valid, runtime_config()),
        Err(Esp32s31BluetoothDtmMemoryClaimError::InUse)
    ));
}
