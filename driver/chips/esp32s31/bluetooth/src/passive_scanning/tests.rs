use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothPassiveScanDefaultTxPowerDbm, BluetoothPassiveScanMemoryGraphModelAddress,
    BluetoothPassiveScanMemoryGraphStorage, BluetoothPassiveScanSchedulerAllocationConfig,
};

use super::{
    BluetoothPassiveScanRuntimeBeginError, BluetoothPassiveScanRuntimeConfig,
    BluetoothPassiveScanRuntimeResources,
};

fn runtime_at(base: u32) -> BluetoothPassiveScanRuntimeResources {
    let storage = std::boxed::Box::leak(std::boxed::Box::new(
        BluetoothPassiveScanMemoryGraphStorage::new(),
    ));
    BluetoothPassiveScanRuntimeResources::claim_static_model(
        storage,
        BluetoothPassiveScanMemoryGraphModelAddress::new(base)
            .expect("the model address lies in controller SRAM"),
        BluetoothPassiveScanRuntimeConfig::new(
            BluetoothPassiveScanSchedulerAllocationConfig::new(0, 0)
                .expect("the zero-capacity profile fits the graph"),
            BluetoothPassiveScanDefaultTxPowerDbm::new(0),
        ),
    )
    .expect("the model scanner graph fits controller SRAM")
}

#[test]
fn sole_graph_is_unavailable_until_exact_restore() {
    let mut runtime = runtime_at(0x2f00_1000);
    let graph = runtime.begin_event().expect("the graph starts idle");
    assert_eq!(
        runtime.begin_event().err(),
        Some(BluetoothPassiveScanRuntimeBeginError::EventActive)
    );
    runtime
        .restore_idle(graph)
        .unwrap_or_else(|_| panic!("the exact graph must return to its runtime"));
    assert!(runtime.event_is_idle());
}

#[test]
fn foreign_graph_cannot_replace_the_checked_out_allocation() {
    let mut first = runtime_at(0x2f00_3000);
    let mut second = runtime_at(0x2f00_5000);
    let first_graph = first.begin_event().expect("the first graph starts idle");
    let second_graph = second.begin_event().expect("the second graph starts idle");
    let second_graph = first
        .restore_idle(second_graph)
        .expect_err("another allocation must be rejected losslessly");
    first
        .restore_idle(first_graph)
        .unwrap_or_else(|_| panic!("the exact first graph must be accepted"));
    second
        .restore_idle(second_graph)
        .unwrap_or_else(|_| panic!("the exact second graph must be accepted"));
}
