use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothDtmMemoryGraphCpuOwned, BluetoothDtmMemoryGraphModelAddress,
    BluetoothDtmMemoryGraphStorage, BluetoothDtmSchedulerAllocationConfig,
};

use super::{
    BluetoothDtmRuntimeConfig, BluetoothDtmRuntimeResources, BluetoothDtmRuntimeSessionBeginError,
    BluetoothDtmSessionIdle,
};
use crate::dtm_event_prepare::BluetoothDtmQuiescedCpuOwned;
use crate::{
    BluetoothDtmDefaultTxPowerDbm, BluetoothDtmPayloadLength, BluetoothDtmPayloadPattern,
    BluetoothDtmTxGraphPrepare,
};

const fn runtime_config() -> BluetoothDtmRuntimeConfig {
    BluetoothDtmRuntimeConfig::new(
        BluetoothDtmSchedulerAllocationConfig::new(1, 2, 1),
        BluetoothDtmDefaultTxPowerDbm::new(7),
    )
}

fn graph_at(address: u32) -> BluetoothDtmMemoryGraphCpuOwned {
    let storage =
        std::boxed::Box::leak(std::boxed::Box::new(BluetoothDtmMemoryGraphStorage::new()));
    let base = BluetoothDtmMemoryGraphModelAddress::new(address)
        .expect("model address uses the controller SRAM syntax");
    let config = BluetoothDtmSchedulerAllocationConfig::new(1, 2, 1);
    BluetoothDtmMemoryGraphStorage::pin_static_model(storage, base, config)
        .expect("model graph fits in controller SRAM")
}

fn graph() -> BluetoothDtmMemoryGraphCpuOwned {
    graph_at(0x2f00_6800)
}

#[test]
fn idle_reinitializes_the_same_graph_for_each_cpu_owned_epoch() {
    let prepared = BluetoothDtmSessionIdle::new(graph())
        .begin_epoch()
        .into_graph()
        .prepare_dtm_tx_packet(
            BluetoothDtmPayloadPattern::Prbs15,
            BluetoothDtmPayloadLength::from_hci_image(5),
        );

    assert_eq!(prepared.pattern(), BluetoothDtmPayloadPattern::Prbs15);
    assert_eq!(prepared.length().hci_image(), 5);

    let prepared = BluetoothDtmSessionIdle::new(prepared.discard())
        .begin_epoch()
        .into_graph()
        .prepare_dtm_tx_packet(
            BluetoothDtmPayloadPattern::Repeated00001111,
            BluetoothDtmPayloadLength::from_hci_image(2),
        );

    assert_eq!(
        prepared.pattern(),
        BluetoothDtmPayloadPattern::Repeated00001111
    );
    assert_eq!(prepared.length().hci_image(), 2);

    let _idle = BluetoothDtmSessionIdle::new(prepared.discard());
}

#[test]
fn cancelling_graph_ready_returns_the_graph_to_idle_retention() {
    let idle = BluetoothDtmSessionIdle::new(graph()).begin_epoch().cancel();
    let _ready = idle.begin_epoch();
}

#[test]
fn terminal_neutral_quiesced_graph_restores_the_exact_runtime_slot() {
    let mut runtime = BluetoothDtmRuntimeResources::from_claimed_graph(
        runtime_config().default_tx_power_dbm(),
        graph(),
    );
    let active = runtime
        .begin_session_epoch()
        .expect("the retained idle graph begins one epoch")
        .into_graph();
    assert!(!runtime.session_is_idle());

    let quiesced = BluetoothDtmQuiescedCpuOwned::from_cpu_owned_for_test(active);
    let idle = BluetoothDtmSessionIdle::from_quiesced(quiesced);
    assert!(
        runtime.restore_idle(idle).is_ok(),
        "the neutral reclaimed owner restores its exact vacant slot"
    );

    assert!(runtime.session_is_idle());
    let _next_epoch = runtime
        .begin_session_epoch()
        .expect("the restored graph is reusable without terminal command policy");
}

#[test]
fn runtime_checkout_is_exclusive_and_cancelled_session_restores_it() {
    let mut runtime = BluetoothDtmRuntimeResources::from_claimed_graph(
        runtime_config().default_tx_power_dbm(),
        graph(),
    );
    assert!(runtime.session_is_idle());
    assert_eq!(runtime.config(), runtime_config());
    assert_eq!(runtime.default_tx_power_dbm().dbm(), 7);

    let ready = runtime
        .begin_session_epoch()
        .expect("the retained idle graph begins one epoch");
    assert!(!runtime.session_is_idle());
    assert!(matches!(
        runtime.begin_session_epoch(),
        Err(BluetoothDtmRuntimeSessionBeginError::SessionActive)
    ));

    runtime
        .restore_idle(ready.cancel())
        .unwrap_or_else(|_| panic!("the vacant runtime slot accepts its cancelled session"));
    assert!(runtime.session_is_idle());
    let _next = runtime
        .begin_session_epoch()
        .expect("the restored graph begins another epoch");
}

#[test]
fn vacant_runtime_rejects_a_foreign_graph_without_losing_either_owner() {
    let mut runtime = BluetoothDtmRuntimeResources::from_claimed_graph(
        runtime_config().default_tx_power_dbm(),
        graph_at(0x2f00_6800),
    );
    let own = runtime
        .begin_session_epoch()
        .expect("the retained graph leaves its runtime slot vacant");

    let mut foreign_runtime = BluetoothDtmRuntimeResources::from_claimed_graph(
        runtime_config().default_tx_power_dbm(),
        graph_at(0x2f00_7800),
    );
    let foreign = foreign_runtime
        .begin_session_epoch()
        .expect("the foreign graph also leaves its own slot vacant")
        .cancel();

    let foreign = runtime
        .restore_idle(foreign)
        .expect_err("a vacant runtime still rejects another bound graph");
    assert!(!runtime.session_is_idle());
    runtime
        .restore_idle(own.cancel())
        .unwrap_or_else(|_| panic!("the exact graph returns to its vacant runtime"));
    foreign_runtime
        .restore_idle(foreign)
        .unwrap_or_else(|_| panic!("the rejected graph remains returnable to its owner"));
    assert!(runtime.session_is_idle());
    assert!(foreign_runtime.session_is_idle());
}

#[test]
fn vacant_runtime_rejects_distinct_storage_with_the_same_model_binding() {
    let mut runtime = BluetoothDtmRuntimeResources::from_claimed_graph(
        runtime_config().default_tx_power_dbm(),
        graph_at(0x2f00_6800),
    );
    let own = runtime
        .begin_session_epoch()
        .expect("the retained graph leaves its runtime slot vacant");

    let mut foreign_runtime = BluetoothDtmRuntimeResources::from_claimed_graph(
        runtime_config().default_tx_power_dbm(),
        graph_at(0x2f00_6800),
    );
    let foreign = foreign_runtime
        .begin_session_epoch()
        .expect("the distinct storage has the same modeled range and configuration")
        .cancel();

    let foreign = runtime
        .restore_idle(foreign)
        .expect_err("modeled range equality cannot substitute for exact storage identity");
    assert!(!runtime.session_is_idle());
    runtime
        .restore_idle(own.cancel())
        .unwrap_or_else(|_| panic!("the exact graph returns to its vacant runtime"));
    foreign_runtime
        .restore_idle(foreign)
        .unwrap_or_else(|_| panic!("the rejected graph remains returnable to its owner"));
    assert!(runtime.session_is_idle());
    assert!(foreign_runtime.session_is_idle());
}
