use crate::{
    ControllerSchedulerEpoch, ControllerTimeSample,
    scheduler::{SchedulerRawWindow, SchedulerSoftwareConfig},
};

use oer_bluetooth_ll::connection::{
    LEGACY_CONNECT_IND_LE_1M_AIRTIME_MICROS, LEGACY_CONNECT_IND_PAYLOAD_BYTES,
    LEGACY_CONNECT_IND_PDU_BYTES, LeLegacyConnectionRequest, LePeripheralConnection,
};

use oer_esp32s31_bluetooth_memory::{
    DirectionFindingWorkspaceModelAddress, DirectionFindingWorkspaceStorage,
    NonScanningRxMemoryModelAddress, NonScanningRxMemoryStorage,
    PeripheralConnectionDefaultTxPowerDbm, PeripheralConnectionMemoryGraphModelAddress,
    PeripheralConnectionMemoryGraphStorage, PeripheralConnectionReceiveTime,
    PeripheralConnectionSchedulerPriority,
};

use oer_esp32s31_pac::BluetoothControllerHalInitConfig;

use super::{
    Le1MPacketStartTiming, PeripheralConnectionRuntimeBeginError,
    PeripheralConnectionRuntimeConfig, PeripheralConnectionRuntimeResources,
};

fn runtime(graph_base: u32) -> PeripheralConnectionRuntimeResources {
    let storage = std::boxed::Box::leak(std::boxed::Box::new(
        PeripheralConnectionMemoryGraphStorage::new(),
    ));
    let receive_storage =
        std::boxed::Box::leak(std::boxed::Box::new(NonScanningRxMemoryStorage::new()));
    let base = PeripheralConnectionMemoryGraphModelAddress::new(graph_base)
        .expect("the model graph base is a controller SRAM address");
    let receive_base = NonScanningRxMemoryModelAddress::new(graph_base.wrapping_add(0x1000))
        .expect("the model receive base is a controller SRAM address");
    PeripheralConnectionRuntimeResources::claim_static_model(
        storage,
        base,
        receive_storage,
        receive_base,
        PeripheralConnectionRuntimeConfig::new(PeripheralConnectionDefaultTxPowerDbm::new(0)),
    )
    .expect("the model graph and receive pool fit controller SRAM")
}

#[test]
fn claimed_runtime_retains_the_idle_allocation() {
    let runtime = runtime(0x2f00_1000);

    assert!(runtime.allocation_is_idle());
}

#[test]
fn recurring_timing_requires_explicit_local_clock_policy() {
    let config =
        PeripheralConnectionRuntimeConfig::new(PeripheralConnectionDefaultTxPowerDbm::new(-3));
    assert!(config.recurring_timing_policy.is_none());

    let configured = config.with_software_recurring_timing(500).unwrap();
    assert_eq!(
        configured.default_tx_power_dbm(),
        config.default_tx_power_dbm()
    );
    assert_eq!(
        configured.recurring_timing_policy,
        Some(super::PeripheralConnectionRecurringTimingPolicy::new(
            super::PeripheralConnectionLocalSleepClockAccuracy::new(500),
            super::PeripheralConnectionWindowWideningMode::SoftwareZeroAccumulatedUncertainty,
        ))
    );
    assert!(config.with_software_recurring_timing(501).is_none());
}

#[test]
fn portable_event_can_prepare_identity_and_cancel_losslessly() {
    let mut runtime = runtime(0x2f00_2000);
    let request = LeLegacyConnectionRequest::decode(&connection_request()).unwrap();
    let connection = LePeripheralConnection::from_request(
        request,
        oer_bluetooth_ll::connection::LeChannelSelectionAlgorithm::AlgorithmTwo,
    );

    let prepared = runtime
        .begin_event()
        .expect("the sole allocation starts idle")
        .prepare_identity(connection.prepare_event());
    assert_eq!(
        runtime.begin_event().err(),
        Some(PeripheralConnectionRuntimeBeginError::EventActive)
    );
    assert_eq!(prepared.event_counter(), 0);
    assert!(prepared.channel().get() < 37);
    assert_eq!(prepared.timing().interval_micros(), 30_000);

    let (allocation, connection) = prepared.cancel();
    runtime
        .restore_idle(allocation)
        .unwrap_or_else(|_| panic!("the exact allocation returns to its runtime"));
    assert!(runtime.allocation_is_idle());
    assert_eq!(connection.event_counter(), 0);
}

#[test]
fn foreign_allocation_cannot_replace_the_checked_out_runtime_slot() {
    let mut first = runtime(0x2f00_9000);
    let mut second = runtime(0x2f00_b000);
    let first_allocation = first.begin_event().expect("the first runtime starts idle");
    let second_allocation = second
        .begin_event()
        .expect("the second runtime starts idle");

    let second_allocation = first
        .restore_idle(second_allocation)
        .expect_err("a foreign graph and RX pool must remain with their caller");
    second
        .restore_idle(second_allocation)
        .unwrap_or_else(|_| panic!("the second runtime accepts its exact allocation"));
    first
        .restore_idle(first_allocation)
        .unwrap_or_else(|_| panic!("the first runtime accepts its exact allocation"));

    assert!(first.allocation_is_idle());
    assert!(second.allocation_is_idle());
}

#[test]
fn retirement_admission_requires_both_identities_and_an_empty_slot() {
    let mut first = runtime(0x2f00_9000);
    let second = runtime(0x2f00_b000);
    assert!(!first.can_restore_allocation(first.graph_identity, first.receive_identity));
    let allocation = first.begin_event().unwrap();
    assert!(first.can_restore_allocation(first.graph_identity, first.receive_identity));
    assert!(!first.can_restore_allocation(second.graph_identity, first.receive_identity));
    assert!(!first.can_restore_allocation(first.graph_identity, second.receive_identity));
    first
        .restore_idle(allocation)
        .unwrap_or_else(|_| panic!("the exact allocation is accepted"));
    assert!(!first.can_restore_allocation(first.graph_identity, first.receive_identity));
}

#[test]
fn reserved_graph_and_exact_pool_restore_the_original_runtime() {
    let mut runtime = runtime(0x2f00_d000);
    let (reserved, receive_pool) = runtime
        .begin_event()
        .expect("the allocation starts idle")
        .reserve_graph();

    assert!(!runtime.allocation_is_idle());

    let allocation = match reserved.rejoin_receive_pool(receive_pool) {
        Ok(allocation) => allocation,
        Err(_) => panic!("the exact receive pool must restore its allocation"),
    };
    assert!(allocation.is_pristine());
    runtime
        .restore_idle(allocation)
        .unwrap_or_else(|_| panic!("the runtime accepts its reconstructed allocation"));
    assert!(runtime.allocation_is_idle());
}

#[test]
fn foreign_pool_rejection_preserves_both_original_pairs() {
    let mut first = runtime(0x2f00_d000);
    let mut second = runtime(0x2f01_0000);
    let (first_reserved, first_pool) = first
        .begin_event()
        .expect("the first allocation starts idle")
        .reserve_graph();
    let (second_reserved, second_pool) = second
        .begin_event()
        .expect("the second allocation starts idle")
        .reserve_graph();

    assert!(!first.allocation_is_idle());
    assert!(!second.allocation_is_idle());

    let first_failure = match first_reserved.rejoin_receive_pool(second_pool) {
        Ok(_) => panic!("a foreign receive pool cannot restore the first allocation"),
        Err(failure) => failure,
    };
    let (first_reserved, second_pool) = first_failure.into_parts();

    let second_failure = match second_reserved.rejoin_receive_pool(first_pool) {
        Ok(_) => panic!("a foreign receive pool cannot restore the second allocation"),
        Err(failure) => failure,
    };
    let (second_reserved, first_pool) = second_failure.into_parts();

    let first_allocation = match first_reserved.rejoin_receive_pool(first_pool) {
        Ok(allocation) => allocation,
        Err(_) => panic!("the exact first receive pool must restore its allocation"),
    };
    let second_allocation = match second_reserved.rejoin_receive_pool(second_pool) {
        Ok(allocation) => allocation,
        Err(_) => panic!("the exact second receive pool must restore its allocation"),
    };

    assert!(first_allocation.is_pristine());
    assert!(second_allocation.is_pristine());
    first
        .restore_idle(first_allocation)
        .unwrap_or_else(|_| panic!("the first runtime accepts its reconstructed allocation"));
    second
        .restore_idle(second_allocation)
        .unwrap_or_else(|_| panic!("the second runtime accepts its reconstructed allocation"));
    assert!(first.allocation_is_idle());
    assert!(second.allocation_is_idle());
}

#[test]
fn first_event_uses_the_received_packet_start_for_its_absolute_window() {
    let mut runtime = runtime(0x2f00_3000);
    let request = LeLegacyConnectionRequest::decode(&connection_request()).unwrap();
    let connection = LePeripheralConnection::from_request(
        request,
        oer_bluetooth_ll::connection::LeChannelSelectionAlgorithm::AlgorithmTwo,
    );

    let prepared = runtime
        .begin_event()
        .expect("the sole allocation starts idle")
        .prepare_first_event(
            connection,
            Le1MPacketStartTiming::from_scheduler_micros(u32::MAX - 100),
        );
    assert!(prepared.receive_pool_is_initialized());
    let window = prepared.first_window();
    assert_eq!(
        window.anchor().image(),
        (u32::MAX - 100)
            .wrapping_add(LEGACY_CONNECT_IND_LE_1M_AIRTIME_MICROS)
            .wrapping_add(request.timing().first_window_start_micros())
    );
    assert_eq!(
        window.end().image().wrapping_sub(window.anchor().image()),
        request
            .timing()
            .first_window_end_micros()
            .wrapping_sub(request.timing().first_window_start_micros())
    );

    let (allocation, connection) = prepared.cancel();
    runtime
        .restore_idle(allocation)
        .unwrap_or_else(|_| panic!("the exact allocation returns to its runtime"));
    assert!(runtime.allocation_is_idle());
    assert_eq!(connection.event_counter(), 0);
}

#[test]
fn first_event_projects_one_preparation_window_without_losing_ownership() {
    let mut runtime = runtime(0x2f00_5000);
    let request = LeLegacyConnectionRequest::decode(&connection_request()).unwrap();
    let expected_channel = LePeripheralConnection::from_request(
        request,
        oer_bluetooth_ll::connection::LeChannelSelectionAlgorithm::AlgorithmTwo,
    )
    .prepare_event()
    .channel();
    let packet_start_micros = 10_000;
    let prepared = runtime
        .begin_event()
        .expect("the sole allocation starts idle")
        .prepare_first_event(
            LePeripheralConnection::from_request(
                request,
                oer_bluetooth_ll::connection::LeChannelSelectionAlgorithm::AlgorithmTwo,
            ),
            Le1MPacketStartTiming::from_scheduler_micros(packet_start_micros),
        );
    let scale = BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale();
    let epoch =
        ControllerSchedulerEpoch::new(ControllerTimeSample::for_validation(100), 9_000, scale);
    let config = SchedulerSoftwareConfig::reviewed_standalone();
    let candidate = match prepared.project_scheduler_window(
        epoch,
        config,
        &ControllerTimeSample::for_validation(41_234),
    ) {
        Ok(candidate) => candidate,
        Err(_) => panic!("the accepted first window has a non-empty raw projection"),
    };
    let anchor = packet_start_micros
        .wrapping_add(LEGACY_CONNECT_IND_LE_1M_AIRTIME_MICROS)
        .wrapping_add(request.timing().first_window_start_micros());

    assert_eq!(candidate.event_counter(), 0);
    assert_eq!(candidate.channel(), expected_channel);
    assert_eq!(
        candidate.requested_window().start(),
        epoch.raw_ticks_for_micros(
            anchor
                .wrapping_sub(config.preparation_lead_micros())
                .wrapping_sub(super::LE_FIRST_EVENT_TIMING_GUARD_MICROS)
                .wrapping_sub(super::LE_FIRST_EVENT_BOUNDARY_GUARD_MICROS)
        )
    );
    assert_eq!(
        candidate.requested_window().end(),
        epoch.raw_ticks_for_micros(
            packet_start_micros
                .wrapping_add(LEGACY_CONNECT_IND_LE_1M_AIRTIME_MICROS)
                .wrapping_add(request.timing().first_window_end_micros())
                .wrapping_add(super::LE_1M_FIRST_EVENT_RESERVATION_MICROS)
                .wrapping_add(super::LE_FIRST_EVENT_BOUNDARY_GUARD_MICROS)
        )
    );

    let (allocation, connection) = candidate.cancel();
    runtime
        .restore_idle(allocation)
        .unwrap_or_else(|_| panic!("the exact allocation returns to its runtime"));
    assert!(runtime.allocation_is_idle());
    assert_eq!(connection.event_counter(), 0);
}

#[test]
fn prepublication_retry_keeps_the_causal_packet_window_and_exact_allocation() {
    let mut runtime = runtime(0x2f00_5800);
    let request = LeLegacyConnectionRequest::decode(&connection_request()).unwrap();
    let packet_start = u32::MAX - 4_000;
    let scale = BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale();
    let epoch = ControllerSchedulerEpoch::new(
        ControllerTimeSample::for_validation(700),
        u32::MAX - 8_000,
        scale,
    );
    let config = SchedulerSoftwareConfig::reviewed_standalone();

    let first = runtime
        .begin_event()
        .expect("the sole allocation starts idle")
        .prepare_first_event(
            LePeripheralConnection::from_request(
                request,
                oer_bluetooth_ll::connection::LeChannelSelectionAlgorithm::AlgorithmTwo,
            ),
            Le1MPacketStartTiming::from_scheduler_micros(packet_start),
        )
        .project_scheduler_window(epoch, config, &ControllerTimeSample::for_validation(41_234))
        .unwrap_or_else(|_| panic!("the accepted packet projects a first-event window"));
    let first_window = first.requested_window();
    let (allocation, connection) = first.cancel();

    let retry = allocation
        .prepare_first_event(
            connection,
            Le1MPacketStartTiming::from_scheduler_micros(packet_start),
        )
        .project_scheduler_window(epoch, config, &ControllerTimeSample::for_validation(41_234))
        .unwrap_or_else(|_| panic!("retrying the same packet keeps a valid window"));

    assert_eq!(retry.requested_window(), first_window);
    let (allocation, connection) = retry.cancel();
    runtime
        .restore_idle(allocation)
        .unwrap_or_else(|_| panic!("the retry returns the exact checked-out allocation"));
    assert!(runtime.allocation_is_idle());
    assert_eq!(connection.event_counter(), 0);
}

#[test]
fn resolved_connection_fields_remain_affine_and_cancel_losslessly() {
    let mut runtime = runtime(0x2f00_7000);
    let request = LeLegacyConnectionRequest::decode(&connection_request()).unwrap();
    let expected_channel = LePeripheralConnection::from_request(
        request,
        oer_bluetooth_ll::connection::LeChannelSelectionAlgorithm::AlgorithmTwo,
    )
    .prepare_event()
    .channel();
    let scale = BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale();
    let epoch =
        ControllerSchedulerEpoch::new(ControllerTimeSample::for_validation(300), 20_000, scale);
    let candidate = runtime
        .begin_event()
        .expect("the sole allocation starts idle")
        .prepare_first_event(
            LePeripheralConnection::from_request(
                request,
                oer_bluetooth_ll::connection::LeChannelSelectionAlgorithm::AlgorithmTwo,
            ),
            Le1MPacketStartTiming::from_scheduler_micros(21_000),
        )
        .project_scheduler_window(
            epoch,
            SchedulerSoftwareConfig::reviewed_standalone(),
            &ControllerTimeSample::for_validation(41_234),
        )
        .unwrap_or_else(|_| panic!("the first connection window projects"));
    let requested = candidate.requested_window();
    let resolved = SchedulerRawWindow::from_projected_scheduler_window(
        requested.end(),
        requested.end().wrapping_add(requested.duration()),
    )
    .expect("the displaced window retains the accepted duration");
    let default_tx_power = PeripheralConnectionDefaultTxPowerDbm::new(-4);
    let priority = PeripheralConnectionSchedulerPriority::FIRST_EVENT;

    let prepared = candidate
        .prepare_resolved_event_fields(resolved, 92, default_tx_power)
        .unwrap_or_else(|_| panic!("a resolved scheduler window remains non-empty"));

    assert_eq!(prepared.event_counter(), 0);
    assert_eq!(prepared.channel().index(), expected_channel.get());
    assert_eq!(prepared.default_tx_power(), default_tx_power);
    assert_eq!(prepared.priority(), priority);
    assert_eq!(prepared.requested_window(), requested);
    assert_eq!(prepared.resolved_window(), resolved);
    assert_eq!(
        prepared.receive_wait().transmit_window_micros(),
        request
            .timing()
            .first_window_end_micros()
            .wrapping_sub(request.timing().first_window_start_micros())
    );
    assert_eq!(
        prepared.receive_wait().timing_guard_micros(),
        super::LE_FIRST_EVENT_TIMING_GUARD_MICROS
    );
    assert_eq!(
        prepared.receive_time(),
        PeripheralConnectionReceiveTime::from_controller_ticks(41_234)
    );

    let workspace_storage =
        std::boxed::Box::leak(std::boxed::Box::new(DirectionFindingWorkspaceStorage::new()));
    let workspace_base = DirectionFindingWorkspaceModelAddress::new(0x2f00_6000)
        .expect("the model workspace base is a controller SRAM address");
    let workspace =
        DirectionFindingWorkspaceStorage::pin_static_model(workspace_storage, workspace_base)
            .expect("the complete workspace fits controller SRAM");
    let workspace_link = workspace.binding().link();
    let prepared = prepared.install_direction_finding_workspace(workspace_link);

    assert_eq!(prepared.event_counter(), 0);
    assert_eq!(prepared.channel().index(), expected_channel.get());
    assert_eq!(prepared.requested_window(), requested);
    assert_eq!(prepared.resolved_window(), resolved);
    assert_eq!(prepared.direction_finding_workspace(), workspace_link);

    let (allocation, connection) = prepared.cancel();
    runtime
        .restore_idle(allocation)
        .unwrap_or_else(|_| panic!("the exact allocation returns to its runtime"));
    assert!(runtime.allocation_is_idle());
    assert_eq!(connection.event_counter(), 0);
}

fn connection_request() -> [u8; LEGACY_CONNECT_IND_PDU_BYTES] {
    let mut pdu = [0; LEGACY_CONNECT_IND_PDU_BYTES];
    pdu[0] = 0x25;
    pdu[1] = LEGACY_CONNECT_IND_PAYLOAD_BYTES as u8;
    pdu[2..8].copy_from_slice(&[1, 2, 3, 4, 5, 6]);
    pdu[8..14].copy_from_slice(&[7, 8, 9, 10, 11, 12]);
    pdu[14..18].copy_from_slice(&0xa1b2_c3d4u32.to_le_bytes());
    pdu[18..21].copy_from_slice(&[0x33, 0x22, 0x11]);
    pdu[21] = 2;
    pdu[22..24].copy_from_slice(&1u16.to_le_bytes());
    pdu[24..26].copy_from_slice(&24u16.to_le_bytes());
    pdu[28..30].copy_from_slice(&200u16.to_le_bytes());
    pdu[30..35].copy_from_slice(&[0xff, 0xff, 0xff, 0xff, 0x1f]);
    pdu[35] = 5;
    pdu
}
