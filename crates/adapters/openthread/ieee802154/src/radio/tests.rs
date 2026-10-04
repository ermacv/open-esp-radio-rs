//! The OpenThread radio over the runtime and the engine's register model:
//! each test drives the trait as OpenThread's `SubMac` does and models the
//! MAC's interrupts in between.

use std::boxed::Box;

use embassy_futures::{block_on, join::join, yield_now};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use oer_espressif_ieee802154_engine::{
    engine::{Ieee802154Engine, Ieee802154EngineBuffers},
    ll::{Ieee802154LlCommand, model::Ieee802154LlModel},
    pib::Ieee802154PibDefaults,
    tx_power::Ieee802154TxPowerLevels,
    types::Ieee802154Event,
};
use oer_espressif_ieee802154_runtime::{
    Ieee802154Random, Ieee802154Runtime, Ieee802154RuntimeParts,
};
use oer_ieee802154::{Ieee802154RadioPort, Interface};
use openthread_radio::{
    Config, CslConfig, FrameCounterUpdate, MacCapabilities, MacKeys, Radio, RadioClock as _,
    RadioErrorKind, RadioRssi as _, SrcMatchConfig, TxFrame,
};

use super::{
    OPEN_THREAD_RADIO_CAPABILITIES, OpenThreadRadio, OpenThreadRadioDefaults, PortClock, PortRssi,
};
use crate::frames::{
    CSL_ACCURACY_PPM, CSL_UNCERTAINTY, extended_pending_address, short_pending_address,
};

mod model;

use oer_time_virtual::SkipClock;

type Runtime =
    Ieee802154Runtime<'static, NoopRawMutex, Ieee802154LlModel, SkipClock, FixedRandom, 16>;
type Thread = OpenThreadRadio<'static, Runtime, 4>;

static LEVELS: [i8; 3] = [-24, 0, 21];

/// A random source that always draws 21.
struct FixedRandom;

impl Ieee802154Random for FixedRandom {
    fn random(&mut self) -> u32 {
        21
    }
}

/// The radio clock of the bench: the runtime's clock starts here, and
/// OpenThread reads the same time.
const NOW_MICROS: u64 = 1_000;

const DEFAULTS: OpenThreadRadioDefaults = OpenThreadRadioDefaults {
    tx_power_dbm: 21,
    cca_threshold_dbm: -75,
    receive_sensitivity_dbm: -104,
};

/// 2006 data frame to short address 0x5678 of PAN 0x1234 requesting an ACK,
/// sequence one, with the FCS OpenThread leaves room for.
const DATA_PSDU: [u8; 11] = [0x61, 0x98, 0x01, 0x34, 0x12, 0x78, 0x56, 0xaa, 0xbb, 0, 0];

/// The immediate ACK of sequence one as the receive DMA writes it, RSSI -60
/// and LQI 200 in place of the FCS.
const ACK_IMAGE: [u8; 6] = [5, 0x02, 0x00, 0x01, (-60i8) as u8, 200];

/// A 2006 data frame without an ACK request as the receive DMA writes it,
/// RSSI -55 and LQI 180 in place of the FCS.
const RECEIVED_IMAGE: [u8; 12] = [
    11,
    0x41,
    0x98,
    0x07,
    0x34,
    0x12,
    0xff,
    0xff,
    0x78,
    0x56,
    (-55i8) as u8,
    180,
];

/// An installed runtime, and the OpenThread radio over it.
fn radio() -> (&'static Runtime, Thread) {
    let runtime: &'static Runtime = Box::leak(Box::new(Runtime::new(SkipClock::starting_at(
        oer_time::Instant::from_micros(NOW_MICROS),
    ))));
    let buffers = Box::leak(Box::new(Ieee802154EngineBuffers::new()));
    let levels = Ieee802154TxPowerLevels::new(&LEVELS).unwrap();
    let parts = Ieee802154RuntimeParts {
        engine: Ieee802154Engine::new(buffers, levels, Ieee802154PibDefaults::default()),
        hardware: Ieee802154LlModel::default(),
    };
    assert!(
        runtime
            .install(parts, FixedRandom, Ieee802154PibDefaults::default())
            .is_ok()
    );
    (runtime, OpenThreadRadio::new(runtime, DEFAULTS))
}

/// A radio OpenThread has initialized.
fn initialized() -> (&'static Runtime, Thread) {
    let (runtime, mut radio) = radio();
    block_on(radio.init()).unwrap();
    (runtime, radio)
}

fn frame(psdu: &mut [u8]) -> TxFrame<'_> {
    TxFrame {
        psdu,
        channel: 15,
        power: 0,
        cca_threshold: None,
        tx_at: None,
        retransmission: false,
        security_processed: true,
        header_updated: false,
        time_sync: None,
    }
}

/// `init` enables the runtime and reports ESP-IDF's figures and
/// capabilities, with the port's zeroed keys installed.
#[test]
fn init_enables_the_radio_and_reports_the_port_capabilities() {
    let (runtime, mut radio) = radio();
    let caps = block_on(radio.init()).unwrap();
    assert_eq!(caps.phy, OPEN_THREAD_RADIO_CAPABILITIES);
    assert_eq!(caps.mac, MacCapabilities::all());
    assert_eq!(caps.default_tx_power, 21);
    assert_eq!(caps.default_cca_threshold, -75);
    assert_eq!(caps.receive_sensitivity, -104);
    assert_eq!(caps.csl_accuracy, CSL_ACCURACY_PPM);
    assert_eq!(caps.csl_uncertainty, CSL_UNCERTAINTY);
    assert_eq!(runtime.frame_counter(Interface::PRIMARY), Ok(Some(0)));
    // A second init finds the radio enabled.
    assert!(block_on(radio.init()).is_ok());
}

/// OpenThread's `otPlatRadioGetNow` and `otPlatRadioGetRssi` read the
/// port itself: its clock and its live RSSI, not a second path to them.
#[test]
fn openthread_reads_the_port_clock_and_live_rssi() {
    let (runtime, _) = radio();
    assert_eq!(PortClock::new(runtime).now_micros(), NOW_MICROS);
    assert_eq!(
        PortRssi::new(runtime).rssi(),
        runtime.recent_rssi().ok(),
        "the live read is the port's"
    );
}

/// Identity and promiscuous mode reach the hardware filter.
#[test]
fn set_config_programs_the_identity() {
    let (runtime, mut radio) = initialized();
    let mut config = Config::new();
    config.pan_id = Some(0x1234);
    config.short_addr = Some(0x5678);
    config.ext_addr = Some(0x0102_0304_0506_0708);
    config.promiscuous = true;
    block_on(radio.set_config(&config)).unwrap();
    let (panid, short, extended) = runtime
        .with_model(|model| {
            (
                model.panid[0],
                model.short_address[0],
                model.extended_address[0],
            )
        })
        .unwrap();
    assert_eq!(panid, 0x1234);
    assert_eq!(short, 0x5678);
    assert_eq!(extended, [8, 7, 6, 5, 4, 3, 2, 1]);
    // Promiscuous mode is PIB state the next operation publishes, as in
    // the vendor driver.
    assert_eq!(runtime.with_model(|model| model.promiscuous), Ok(false));
    block_on(radio.set_receive(15)).unwrap();
    assert_eq!(runtime.with_model(|model| model.promiscuous), Ok(true));
}

/// A frame requesting an ACK completes with the ACK's PSDU, RSSI and LQI.
#[test]
fn an_acknowledged_transmission_returns_the_ack() {
    let (runtime, mut radio) = initialized();
    let mut psdu = DATA_PSDU;
    let mut ack = [0u8; 127];
    let (result, ()) = block_on(join(
        async {
            let mut frame = frame(&mut psdu);
            radio.transmit_frame(&mut frame, Some(&mut ack)).await
        },
        async {
            yield_now().await;
            runtime.model_interrupt(None, &[Ieee802154Event::TxDone]);
            runtime.model_interrupt(Some(&ACK_IMAGE), &[Ieee802154Event::AckRxDone]);
        },
    ));
    let meta = result.unwrap().expect("the ACK is reported");
    assert_eq!(&ack[..meta.len], &[0x02, 0x00, 0x01, 0, 0]);
    assert_eq!(meta.channel, 15);
    assert_eq!(meta.rssi, Some(-60));
    assert_eq!(meta.lqi, Some(200));
}

/// A missing ACK is OpenThread's `NoAck`, so `SubMac` retries the frame.
#[test]
fn a_missing_ack_is_an_ack_timeout() {
    let (runtime, mut radio) = initialized();
    let mut psdu = DATA_PSDU;
    let mut ack = [0u8; 127];
    let (result, ()) = block_on(join(
        async {
            let mut frame = frame(&mut psdu);
            radio.transmit_frame(&mut frame, Some(&mut ack)).await
        },
        async {
            yield_now().await;
            runtime.model_interrupt(None, &[Ieee802154Event::TxDone]);
            runtime.model_interrupt(None, &[Ieee802154Event::Timer0Overflow]);
        },
    ));
    assert_eq!(result, Err(RadioErrorKind::RxAckTimeout));
}

/// A frame received before a transmission ends waits for `receive`.
#[test]
fn frames_received_during_a_transmission_are_kept() {
    let (runtime, mut radio) = initialized();
    block_on(radio.set_receive(15)).unwrap();
    // The frame's event is still queued when OpenThread transmits.
    runtime.model_interrupt(Some(&RECEIVED_IMAGE), &[Ieee802154Event::RxDone]);
    let mut psdu = DATA_PSDU;
    psdu[0] = 0x41;
    let (result, ()) = block_on(join(
        async {
            let mut frame = frame(&mut psdu);
            radio.transmit_frame(&mut frame, None).await
        },
        async {
            yield_now().await;
            runtime.model_interrupt(None, &[Ieee802154Event::TxDone]);
        },
    ));
    assert_eq!(result, Ok(None));
    let mut buffer = [0u8; 127];
    let meta = block_on(radio.receive(&mut buffer)).unwrap();
    assert_eq!(meta.len, 11);
    assert_eq!(&buffer[..9], &RECEIVED_IMAGE[1..10]);
    assert_eq!(meta.rssi, Some(-55));
    assert_eq!(meta.lqi, Some(180));
    assert_eq!(meta.channel, 15);
}

/// OpenThread's CCA threshold asks for CCA-then-TX; without one the frame
/// goes out directly.
#[test]
fn a_cca_threshold_selects_cca_before_transmission() {
    let (runtime, mut radio) = initialized();
    for (threshold, command) in [
        (Some(-70), Ieee802154LlCommand::CcaTxStart),
        (None, Ieee802154LlCommand::TxStart),
    ] {
        let mut psdu = DATA_PSDU;
        psdu[0] = 0x41;
        let (result, ()) = block_on(join(
            async {
                let mut frame = frame(&mut psdu);
                frame.cca_threshold = threshold;
                radio.transmit_frame(&mut frame, None).await
            },
            async {
                yield_now().await;
                assert_eq!(runtime.with_model(|model| model.command), Ok(Some(command)));
                runtime.model_interrupt(None, &[Ieee802154Event::TxDone]);
            },
        ));
        assert_eq!(result, Ok(None));
    }
}

/// A transmission whose future OpenThread dropped ends before the next
/// operation starts.
#[test]
fn an_abandoned_transmission_ends_before_the_next_operation() {
    let (runtime, mut radio) = initialized();
    let mut psdu = DATA_PSDU;
    psdu[0] = 0x41;
    {
        let mut frame = frame(&mut psdu);
        let transmit = core::pin::pin!(radio.transmit_frame(&mut frame, None));
        // One poll submits the frame; then OpenThread drops the future.
        let waker = core::task::Waker::noop();
        let mut context = core::task::Context::from_waker(waker);
        assert!(transmit.poll(&mut context).is_pending());
    }
    let (result, ()) = block_on(join(radio.set_receive(15), async {
        yield_now().await;
        runtime.model_interrupt(None, &[Ieee802154Event::TxDone]);
    }));
    assert_eq!(result, Ok(()));
    assert_eq!(
        runtime.with_model(|model| model.command),
        Ok(Some(Ieee802154LlCommand::RxStart))
    );
}

/// OpenThread's CSL period and sample time are accepted by the runtime.
#[test]
fn csl_reaches_the_runtime() {
    let (_, mut radio) = initialized();
    assert_eq!(
        block_on(radio.set_csl(CslConfig {
            period: 3125,
            sample_time: 123_456,
        })),
        Ok(())
    );
}

/// An energy scan reports the measured energy.
#[test]
fn an_energy_scan_reports_the_energy() {
    let (runtime, mut radio) = initialized();
    runtime.with_model(|model| model.ed_rss = -63).unwrap();
    let (energy, ()) = block_on(join(radio.energy_scan(20, 2), async {
        yield_now().await;
        runtime.model_interrupt(None, &[Ieee802154Event::EdDone]);
    }));
    assert_eq!(energy, Ok(-63));
}

/// Source-match entries reach the pending table one by one, and enabling
/// the table selects the enhanced pending mode.
#[test]
fn source_matching_edits_the_pending_table() {
    let (runtime, mut radio) = initialized();
    let mut config = SrcMatchConfig::new();
    config.enabled = true;
    config.short_addrs.push(0x1111).unwrap();
    config.ext_addrs.push(0x0102_0304_0506_0708).unwrap();
    block_on(radio.set_src_match_config(&config)).unwrap();
    let holds = |runtime: &Runtime| {
        runtime
            .with_pending_table(|table| {
                (
                    table.contains(short_pending_address(0x1111)),
                    table.contains(extended_pending_address(0x0102_0304_0506_0708)),
                )
            })
            .unwrap()
    };
    assert_eq!(holds(runtime), (true, true));
    config.short_addrs.clear();
    block_on(radio.set_src_match_config(&config)).unwrap();
    assert_eq!(holds(runtime), (false, true));
}

/// The frame counter follows OpenThread's updates.
#[test]
fn the_frame_counter_follows_openthread() {
    let (runtime, mut radio) = initialized();
    block_on(radio.set_mac_frame_counter(FrameCounterUpdate::Set(10))).unwrap();
    block_on(radio.set_mac_frame_counter(FrameCounterUpdate::SetIfLarger(5))).unwrap();
    let counter = |runtime: &Runtime| runtime.frame_counter(Interface::PRIMARY).unwrap().unwrap();
    assert_eq!(counter(runtime), 10);
    block_on(radio.set_mac_frame_counter(FrameCounterUpdate::SetIfLarger(12))).unwrap();
    assert_eq!(counter(runtime), 12);
}

/// A secured 2006 data frame requesting an ACK: security level 5 with key
/// identifier mode 1, an empty frame counter and key index, two payload
/// bytes, room for the MIC-32 and the FCS.
const SECURED_PSDU: [u8; 23] = [
    0x69, 0x98, 0x01, 0x34, 0x12, 0x78, 0x56, 0xaa, 0xbb, // header
    0x0d, 0, 0, 0, 0, 0, // security control, frame counter, key index
    0xde, 0xad, // payload
    0, 0, 0, 0, // MIC
    0, 0, // FCS
];

/// Transmit `psdu` and acknowledge it; returns the frame's header update.
fn secured_transmission(
    runtime: &'static Runtime,
    radio: &mut Thread,
    psdu: &mut [u8],
    retransmission: bool,
) -> bool {
    let mut ack = [0u8; 127];
    let (updated, ()) = block_on(join(
        async {
            let mut frame = frame(psdu);
            frame.security_processed = false;
            frame.retransmission = retransmission;
            radio
                .transmit_frame(&mut frame, Some(&mut ack))
                .await
                .unwrap();
            frame.header_updated
        },
        async {
            yield_now().await;
            runtime.model_interrupt(None, &[Ieee802154Event::TxDone]);
            runtime.model_interrupt(Some(&ACK_IMAGE), &[Ieee802154Event::AckRxDone]);
        },
    ));
    updated
}

/// As ESP-IDF's port, the radio secures a first transmission with the next
/// frame counter and the current key index and writes both back into
/// OpenThread's frame; a retransmission keeps the frame's own.
#[test]
fn the_radio_secures_frames_with_openthread_keys() {
    let (runtime, mut radio) = initialized();
    block_on(radio.set_mac_keys(&MacKeys {
        key_id_mode: 1,
        key_id: 3,
        previous: [1; 16],
        current: [2; 16],
        next: [3; 16],
    }))
    .unwrap();
    block_on(radio.set_mac_frame_counter(FrameCounterUpdate::Set(10))).unwrap();
    let counter = |runtime: &Runtime| runtime.frame_counter(Interface::PRIMARY).unwrap().unwrap();

    let mut psdu = SECURED_PSDU;
    assert!(secured_transmission(runtime, &mut radio, &mut psdu, false));
    assert_eq!(&psdu[10..15], &[10, 0, 0, 0, 3]);
    assert_eq!(counter(runtime), 11);

    // `SubMac` retransmits the frame it got back, counter and key index
    // included.
    assert!(!secured_transmission(runtime, &mut radio, &mut psdu, true));
    assert_eq!(&psdu[10..15], &[10, 0, 0, 0, 3]);
    assert_eq!(counter(runtime), 11);
}
