use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothLegacyAdvertisingPrimaryChannel, BluetoothLegacyConnectableAdvIndPacketInput,
    BluetoothLegacyConnectableAdvertisingMemoryGraphModelAddress,
    BluetoothLegacyConnectableAdvertisingMemoryGraphStorage,
    BluetoothLegacyConnectableAdvertisingMemoryInput,
    BluetoothLegacyConnectableAdvertisingOwnAddress,
    BluetoothLegacyConnectableAdvertisingPostAnchorDuration,
    BluetoothLegacyConnectableScanResponsePacketInput, BluetoothNonScanningRxMemoryModelAddress,
    BluetoothNonScanningRxMemoryStorage,
};
use open_esp_radio_esp32s31_pac::BluetoothControllerHalInitConfig;

use super::{
    BluetoothLegacyAdvertisingEventWindow, BluetoothLegacyAdvertisingRecurringTimingObservation,
    BluetoothLegacyAdvertisingTimingObservation,
};
use crate::{
    BluetoothControllerSchedulerEpoch, BluetoothControllerTimeSample, BluetoothSchedulerInstant,
    BluetoothSchedulerSoftwareConfig,
};

const fn instant(image: u32) -> BluetoothSchedulerInstant {
    BluetoothSchedulerInstant::from_image(image)
}

fn connectable_post_anchor_duration() -> BluetoothLegacyConnectableAdvertisingPostAnchorDuration {
    const ADV_IND_PDU: [u8; 11] = [0x60, 9, 1, 2, 3, 4, 5, 6, 2, 1, 6];
    const SCAN_RESPONSE_PDU: [u8; 8] = [0x44, 6, 1, 2, 3, 4, 5, 6];

    let graph_storage = std::boxed::Box::leak(std::boxed::Box::new(
        BluetoothLegacyConnectableAdvertisingMemoryGraphStorage::new(),
    ));
    let graph_address =
        BluetoothLegacyConnectableAdvertisingMemoryGraphModelAddress::new(0x2f00_0100)
            .expect("the modeled graph address belongs to controller SRAM");
    let graph = BluetoothLegacyConnectableAdvertisingMemoryGraphStorage::pin_static_model(
        graph_storage,
        graph_address,
    )
    .expect("the connectable graph fits controller SRAM");

    let receive_storage = std::boxed::Box::leak(std::boxed::Box::new(
        BluetoothNonScanningRxMemoryStorage::new(),
    ));
    let receive_address = BluetoothNonScanningRxMemoryModelAddress::new(0x2f00_4000)
        .expect("the modeled RX address belongs to controller SRAM");
    let receive =
        BluetoothNonScanningRxMemoryStorage::pin_static_model(receive_storage, receive_address)
            .expect("the receive pool fits controller SRAM");

    let input = BluetoothLegacyConnectableAdvertisingMemoryInput::new(
        BluetoothLegacyConnectableAdvIndPacketInput::try_from_encoded_extent(&ADV_IND_PDU, 9)
            .expect("the ADV_IND fits the controller allocation"),
        BluetoothLegacyConnectableScanResponsePacketInput::try_from_encoded_extent(
            &SCAN_RESPONSE_PDU,
            6,
        )
        .expect("the SCAN_RSP fits the controller allocation"),
        BluetoothLegacyConnectableAdvertisingOwnAddress::Random([1, 2, 3, 4, 5, 6]),
        BluetoothLegacyAdvertisingPrimaryChannel::Channel37,
    );
    graph
        .prepare_response_capable_event(input, receive, 0)
        .expect("the disjoint response-capable graph is supported")
        .post_anchor_duration()
}

#[test]
fn first_event_retains_lll_delay_preparation_lead_and_le_1m_airtime() {
    let window = BluetoothLegacyAdvertisingEventWindow::first_le_1m(
        BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
        instant(10_000),
        instant(11_999),
        9,
    );

    assert_eq!(window.start().image(), 12_000);
    assert_eq!(window.anchor().image(), 12_107);
    assert_eq!(window.end().image(), 12_259);
}

#[test]
fn later_radio_ready_shifts_the_complete_window_without_changing_duration() {
    let window = BluetoothLegacyAdvertisingEventWindow::first_le_1m(
        BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
        instant(10_000),
        instant(12_050),
        9,
    );

    assert_eq!(window.anchor().image(), 12_107);
    assert_eq!(window.start().image(), 12_050);
    assert_eq!(window.end().image(), 12_309);
    assert_eq!(
        window.end().image().wrapping_sub(window.start().image()),
        259
    );
}

#[test]
fn connectable_first_event_reserves_preparation_and_memory_supplied_duration() {
    let scale = BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale();
    let epoch = BluetoothControllerSchedulerEpoch::new(
        BluetoothControllerTimeSample::for_validation(100),
        1_000,
        scale,
    );
    let observation = BluetoothLegacyAdvertisingTimingObservation {
        current: instant(10_000),
        radio_ready: instant(11_999),
        epoch,
    };
    let (window, raw) = observation
        .first_connectable_window(
            BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
            connectable_post_anchor_duration(),
        )
        .expect("the complete first connectable event fits one raw epoch");

    assert_eq!(window.start().image(), 12_000);
    assert_eq!(window.anchor().image(), 12_107);
    assert_eq!(window.end().image(), 12_263);
    assert_eq!(
        window.end().image().wrapping_sub(window.start().image()),
        263
    );
    assert_eq!(
        raw.start(),
        epoch.raw_ticks_for_micros(window.start().image())
    );
    assert_eq!(raw.end(), epoch.raw_ticks_for_micros(window.end().image()));
}

#[test]
fn connectable_recurrence_preserves_portable_phase_and_complete_graph_duration() {
    let config = BluetoothSchedulerSoftwareConfig::reviewed_standalone();
    let post_anchor_duration = connectable_post_anchor_duration();
    let scale = BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale();
    let epoch = BluetoothControllerSchedulerEpoch::new(
        BluetoothControllerTimeSample::for_validation(100),
        1_000,
        scale,
    );
    let first = BluetoothLegacyAdvertisingEventWindow::first_with_post_anchor_duration(
        config,
        instant(10_000),
        instant(11_000),
        post_anchor_duration.as_micros(),
    );
    let offset = 25_000;
    let (next, _) = BluetoothLegacyAdvertisingRecurringTimingObservation::new(epoch)
        .recurring_connectable_window(first.phase(), offset, config, post_anchor_duration)
        .expect("one selected-channel successor fits the retained epoch");

    assert_eq!(
        next.anchor().image().wrapping_sub(first.anchor().image()),
        offset as u32
    );
    assert_eq!(
        next.anchor().image().wrapping_sub(next.start().image()),
        config.preparation_lead_micros()
    );
    assert_eq!(
        next.end().image().wrapping_sub(next.anchor().image()),
        post_anchor_duration.as_micros()
    );
}

#[test]
fn late_connectable_first_event_shifts_both_endpoints_without_shortening() {
    let scale = BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale();
    let epoch = BluetoothControllerSchedulerEpoch::new(
        BluetoothControllerTimeSample::for_validation(100),
        1_000,
        scale,
    );
    let observation = BluetoothLegacyAdvertisingTimingObservation {
        current: instant(10_000),
        radio_ready: instant(12_050),
        epoch,
    };
    let (window, raw) = observation
        .first_connectable_window(
            BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
            connectable_post_anchor_duration(),
        )
        .expect("the shifted first connectable event fits one raw epoch");

    assert_eq!(window.anchor().image(), 12_107);
    assert_eq!(window.start().image(), 12_050);
    assert_eq!(window.end().image(), 12_313);
    assert_eq!(
        window.end().image().wrapping_sub(window.start().image()),
        263
    );
    assert_eq!(
        raw.start(),
        epoch.raw_ticks_for_micros(window.start().image())
    );
    assert_eq!(raw.end(), epoch.raw_ticks_for_micros(window.end().image()));
}

#[test]
fn connectable_radio_ready_at_nominal_start_keeps_the_full_preparation_lead() {
    let config = BluetoothSchedulerSoftwareConfig::reviewed_standalone();
    let post_anchor_duration = connectable_post_anchor_duration();
    let scale = BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale();
    let epoch = BluetoothControllerSchedulerEpoch::new(
        BluetoothControllerTimeSample::for_validation(100),
        1_000,
        scale,
    );
    let observation = BluetoothLegacyAdvertisingTimingObservation {
        current: instant(10_000),
        radio_ready: instant(12_000),
        epoch,
    };
    let (window, _raw) = observation
        .first_connectable_window(config, post_anchor_duration)
        .expect("the boundary-ready event fits one raw epoch");

    assert_eq!(window.start(), instant(12_000));
    assert_eq!(
        window.anchor().image().wrapping_sub(window.start().image()),
        config.preparation_lead_micros()
    );
    assert_eq!(
        window.end().image().wrapping_sub(window.anchor().image()),
        post_anchor_duration.as_micros()
    );
}

#[test]
fn connectable_first_event_preserves_its_duration_across_scheduler_wrap() {
    let config = BluetoothSchedulerSoftwareConfig::reviewed_standalone();
    let post_anchor_duration = connectable_post_anchor_duration();
    let scale = BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale();
    let epoch = BluetoothControllerSchedulerEpoch::new(
        BluetoothControllerTimeSample::for_validation(100),
        1_000,
        scale,
    );
    let radio_ready = instant(1_800);
    let observation = BluetoothLegacyAdvertisingTimingObservation {
        current: instant(0xffff_ff00),
        radio_ready,
        epoch,
    };
    let (window, raw) = observation
        .first_connectable_window(config, post_anchor_duration)
        .expect("the wrapping response-capable event fits one raw epoch");

    assert_eq!(window.start(), radio_ready);
    assert_eq!(
        window.end().image().wrapping_sub(window.start().image()),
        config
            .preparation_lead_micros()
            .wrapping_add(post_anchor_duration.as_micros())
    );
    assert_eq!(
        raw.duration(),
        epoch
            .raw_ticks_for_micros(window.end().image())
            .wrapping_sub(epoch.raw_ticks_for_micros(window.start().image()))
    );
}

#[test]
fn first_event_uses_signed_wrapping_order_and_live_epoch_projection() {
    let window = BluetoothLegacyAdvertisingEventWindow::first_le_1m(
        BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
        instant(0xffff_ff00),
        instant(1_800),
        6,
    );
    assert_eq!(window.start().image(), 1_800);
    assert_eq!(window.anchor().image(), 1_851);
    assert_eq!(window.end().image(), 2_035);

    let scale = BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale();
    let epoch = BluetoothControllerSchedulerEpoch::new(
        BluetoothControllerTimeSample::for_validation(100),
        1_000,
        scale,
    );
    let (raw, item_duration) = window
        .project_raw(epoch, 3)
        .expect("bounded three-channel event window");
    assert_eq!(item_duration, 58);
    assert_eq!(raw.duration(), 174);
}

#[test]
fn recurring_event_advances_nominal_phase_and_reserves_the_complete_chain() {
    let first = BluetoothLegacyAdvertisingEventWindow::first_le_1m(
        BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
        instant(10_000),
        instant(11_999),
        9,
    );
    let recurring = BluetoothLegacyAdvertisingEventWindow::recurring_le_1m(
        first.phase(),
        20_000,
        BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
        9,
    );
    assert_eq!(recurring.start().image(), 32_000);
    assert_eq!(recurring.anchor().image(), 32_107);
    assert_eq!(recurring.end().image(), 32_259);

    let scale = BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale();
    let epoch = BluetoothControllerSchedulerEpoch::new(
        BluetoothControllerTimeSample::for_validation(100),
        1_000,
        scale,
    );
    let (raw, item_duration) = recurring
        .project_raw(epoch, 3)
        .expect("the recurring chain fits one raw epoch");
    assert_eq!(raw.duration(), item_duration * 3);
}
