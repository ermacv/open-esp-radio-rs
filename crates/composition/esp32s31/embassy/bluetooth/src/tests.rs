use oer_esp32s31_bluetooth::le::{
    advertising::LegacyAdvertisingDefaultTxPowerDbm,
    dtm::{DtmDefaultTxPowerDbm, DtmRuntimeConfig},
    peripheral::PeripheralConnectionRuntimeConfig,
    scanning::PassiveScanRuntimeConfig,
};

use oer_esp32s31_bluetooth_memory::{
    BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH, BlePhyEngineBindError, BlePhyEngineModelAddress,
    BlePhyEngineStorage, DirectionFindingWorkspaceModelAddress, DtmMemoryGraphBindError,
    DtmMemoryGraphModelAddress, DtmMemoryGraphStorage, DtmSchedulerAllocationConfig,
    LegacyAdvertisingMemoryGraphModelAddress, LegacyConnectableAdvertisingMemoryGraphModelAddress,
    NonScanningRxMemoryModelAddress, PassiveScanDefaultTxPowerDbm,
    PassiveScanMemoryGraphModelAddress, PassiveScanSchedulerAllocationConfig,
    PeripheralConnectionDefaultTxPowerDbm, PeripheralConnectionMemoryGraphModelAddress,
};

use super::{
    BluetoothBlePhyMemory, BluetoothBlePhyMemoryClaimError, BluetoothDirectionFindingMemory,
    BluetoothDirectionFindingMemoryClaimError, BluetoothDtmMemory, BluetoothDtmMemoryClaimError,
    BluetoothLegacyAdvertisingMemory, BluetoothLegacyAdvertisingMemoryClaimError,
    BluetoothLegacyConnectableAdvertisingMemory,
    BluetoothLegacyConnectableAdvertisingMemoryClaimError, BluetoothPassiveScanMemory,
    BluetoothPassiveScanMemoryClaimError, BluetoothPeripheralConnectionMemory,
    BluetoothPeripheralConnectionMemoryClaimError,
};

const fn runtime_config() -> DtmRuntimeConfig {
    DtmRuntimeConfig::new(
        DtmSchedulerAllocationConfig::new(2, 3, 4),
        DtmDefaultTxPowerDbm::new(6),
    )
}

#[test]
fn model_ble_phy_arena_is_claimed_once_as_one_bound_graph() {
    static MEMORY: BluetoothBlePhyMemory = BluetoothBlePhyMemory::new();

    let base = BlePhyEngineModelAddress::new(0x2f00_2000).expect("model base is encodable");
    let owner = MEMORY
        .claim_model(base)
        .expect("fresh model arena binds once");
    let (start, end) = owner.binding().range();
    assert_eq!(start, 0x2f00_2000);
    assert_eq!(end - start, size_of::<BlePhyEngineStorage>() as u32);
    assert!(matches!(
        MEMORY.claim_model(base),
        Err(BluetoothBlePhyMemoryClaimError::InUse)
    ));
}

#[test]
fn model_direction_finding_workspace_is_claimed_once() {
    static MEMORY: BluetoothDirectionFindingMemory = BluetoothDirectionFindingMemory::new();
    let base =
        DirectionFindingWorkspaceModelAddress::new(0x2f00_3000).expect("model base is encodable");

    let owner = MEMORY
        .claim_model(base)
        .expect("fresh workspace binds once");

    assert!(owner.is_disabled_baseline_initialized());
    assert!(matches!(
        MEMORY.claim_model(base),
        Err(BluetoothDirectionFindingMemoryClaimError::InUse)
    ));
}

#[test]
fn model_legacy_advertising_arena_is_claimed_once() {
    static MEMORY: BluetoothLegacyAdvertisingMemory = BluetoothLegacyAdvertisingMemory::new();
    let base = LegacyAdvertisingMemoryGraphModelAddress::new(0x2f00_6000)
        .expect("model base is encodable");
    let runtime = MEMORY
        .claim_model(base, LegacyAdvertisingDefaultTxPowerDbm::new(6))
        .expect("fresh advertising arena binds once");
    assert!(runtime.event_is_idle());
    assert_eq!(runtime.default_tx_power_dbm().dbm(), 6);
    assert!(matches!(
        MEMORY.claim_model(base, LegacyAdvertisingDefaultTxPowerDbm::new(6)),
        Err(BluetoothLegacyAdvertisingMemoryClaimError::InUse)
    ));
}

#[test]
fn model_passive_scanner_arena_is_claimed_once() {
    static MEMORY: BluetoothPassiveScanMemory = BluetoothPassiveScanMemory::new();
    let base =
        PassiveScanMemoryGraphModelAddress::new(0x2f00_8000).expect("model base is encodable");
    let config = PassiveScanRuntimeConfig::new(
        PassiveScanSchedulerAllocationConfig::new(2, 3)
            .expect("the product limits fit the scanner graph"),
        PassiveScanDefaultTxPowerDbm::new(6),
    );
    let runtime = MEMORY
        .claim_model(base, config)
        .expect("fresh scanner arena binds once");
    assert!(runtime.event_is_idle());
    assert_eq!(runtime.config(), config);
    assert!(matches!(
        MEMORY.claim_model(base, config),
        Err(BluetoothPassiveScanMemoryClaimError::InUse)
    ));
}

#[test]
fn model_peripheral_connection_arena_is_claimed_once() {
    static MEMORY: BluetoothPeripheralConnectionMemory = BluetoothPeripheralConnectionMemory::new();
    let base = PeripheralConnectionMemoryGraphModelAddress::new(0x2f00_a000)
        .expect("model base is encodable");
    let receive_base =
        NonScanningRxMemoryModelAddress::new(0x2f00_b000).expect("model receive base is encodable");
    let config =
        PeripheralConnectionRuntimeConfig::new(PeripheralConnectionDefaultTxPowerDbm::new(6));
    let runtime = MEMORY
        .claim_model(base, receive_base, config)
        .expect("fresh connection arena binds once");

    assert!(runtime.allocation_is_idle());
    assert_eq!(runtime.config(), config);
    assert!(matches!(
        MEMORY.claim_model(base, receive_base, config),
        Err(BluetoothPeripheralConnectionMemoryClaimError::InUse)
    ));
}

#[test]
fn model_connectable_advertising_arena_is_claimed_once() {
    static MEMORY: BluetoothLegacyConnectableAdvertisingMemory =
        BluetoothLegacyConnectableAdvertisingMemory::new();
    let base = LegacyConnectableAdvertisingMemoryGraphModelAddress::new(0x2f00_c000)
        .expect("model base is encodable");
    let runtime = MEMORY
        .claim_model(base, LegacyAdvertisingDefaultTxPowerDbm::new(6))
        .expect("fresh connectable-advertising arena binds once");

    assert!(runtime.event_is_idle());
    assert_eq!(runtime.default_tx_power_dbm().dbm(), 6);
    assert!(matches!(
        MEMORY.claim_model(base, LegacyAdvertisingDefaultTxPowerDbm::new(6)),
        Err(BluetoothLegacyConnectableAdvertisingMemoryClaimError::InUse)
    ));
}

#[test]
fn ble_phy_placement_failure_is_sticky_and_retains_the_allocation() {
    static MEMORY: BluetoothBlePhyMemory = BluetoothBlePhyMemory::new();

    let crossing = BlePhyEngineModelAddress::new(
        BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH - size_of::<BlePhyEngineStorage>() as u32 + 4,
    )
    .expect("crossing model base is still encodable");
    let failure = match MEMORY.claim_model(crossing) {
        Err(BluetoothBlePhyMemoryClaimError::Placement(failure)) => failure,
        Err(BluetoothBlePhyMemoryClaimError::InUse) => {
            panic!("fresh arena cannot already be in use")
        }
        Ok(_) => panic!("crossing placement must fail closed"),
    };
    assert_eq!(
        failure.error(),
        BlePhyEngineBindError::ExtentOutsidePhysicalSram
    );
    let (_storage, error) = failure.into_parts();
    assert_eq!(error, BlePhyEngineBindError::ExtentOutsidePhysicalSram);

    let valid =
        BlePhyEngineModelAddress::new(0x2f00_2000).expect("valid retry address is encodable");
    assert!(matches!(
        MEMORY.claim_model(valid),
        Err(BluetoothBlePhyMemoryClaimError::InUse)
    ));
}

#[test]
fn model_arena_is_claimed_once_as_one_bound_graph() {
    static MEMORY: BluetoothDtmMemory = BluetoothDtmMemory::new();

    let base = DtmMemoryGraphModelAddress::new(0x2f00_1000).expect("model base is encodable");
    let runtime = MEMORY
        .claim_model(base, runtime_config())
        .expect("fresh model arena binds once");
    assert_eq!(runtime.config(), runtime_config());
    assert_eq!(runtime.default_tx_power_dbm().dbm(), 6);
    assert!(runtime.session_is_idle());
    assert!(matches!(
        MEMORY.claim_model(base, runtime_config()),
        Err(BluetoothDtmMemoryClaimError::InUse)
    ));
}

#[test]
fn placement_failure_is_sticky_and_retains_the_allocation() {
    static MEMORY: BluetoothDtmMemory = BluetoothDtmMemory::new();

    let crossing = DtmMemoryGraphModelAddress::new(
        BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH - size_of::<DtmMemoryGraphStorage>() as u32 + 4,
    )
    .expect("crossing model base is still encodable");
    let failure = match MEMORY.claim_model(crossing, runtime_config()) {
        Err(BluetoothDtmMemoryClaimError::Placement(failure)) => failure,
        Err(BluetoothDtmMemoryClaimError::InUse) => {
            panic!("fresh arena cannot already be in use")
        }
        Ok(_) => panic!("crossing placement must fail closed"),
    };
    assert_eq!(
        failure.error(),
        DtmMemoryGraphBindError::ExtentOutsidePhysicalSram
    );
    let (_storage, error) = failure.into_parts();
    assert_eq!(error, DtmMemoryGraphBindError::ExtentOutsidePhysicalSram);

    let valid =
        DtmMemoryGraphModelAddress::new(0x2f00_1000).expect("valid retry address is encodable");
    assert!(matches!(
        MEMORY.claim_model(valid, runtime_config()),
        Err(BluetoothDtmMemoryClaimError::InUse)
    ));
}
