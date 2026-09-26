//! Role behavior over the HAL register model: admission, event correlation,
//! frame lending and resume. The engine's vendor sequences are covered by
//! the engine tests and the host stand.

use std::{boxed::Box, vec, vec::Vec};

use oer_esp32s31_hal::ieee802154::{
    Ieee802154TxPowerLevels,
    ll::{Ieee802154LlCommand, model::Ieee802154LlModel},
    mac::{
        Ieee802154Event, Ieee802154RxAbortReason, Ieee802154RxAbortReasonObservation,
        Ieee802154TxAbortReason, Ieee802154TxAbortReasonObservation,
    },
};
use oer_esp32s31_ieee802154::engine::{Ieee802154Engine, Ieee802154EngineBuffers};
use oer_esp32s31_ieee802154::pib::Ieee802154PibDefaults;
use oer_ieee802154::{
    Channel, CommandError, Configuration, EnergyScanRequest, FramePending, FrameView, MacKeys,
    RadioCommand, RadioEvent, RadioState, RequestId, RestingState, TxMode, TxRequest, TxStatus,
};

use super::{
    IEEE802154_ENHANCED_ACK_IE_CAPACITY, Ieee802154EnhancedAckGenerator,
    Ieee802154EnhancedAckIeTooLong, Ieee802154Platform, Ieee802154Radio, Ieee802154RadioSink,
};

static LEVELS: [i8; 3] = [-9, 0, 10];

const PLATFORM: Ieee802154Platform = Ieee802154Platform {
    now_micros: || 42,
    random: || 21,
};

/// Owned copy of one delivered event: the frame bytes and the event.
#[derive(Debug, PartialEq)]
enum Seen {
    Received {
        mac: Vec<u8>,
        channel: u8,
    },
    TransmitDone {
        id: RequestId,
        status: TxStatus,
        ack_pending: Option<FramePending>,
    },
    EnergyScanDone(i8),
    EnergyScanFailed,
    ClearChannelAssessmentDone {
        idle: bool,
    },
    ClearChannelAssessmentFailed,
    Fault,
}

#[derive(Default)]
struct Sink(Vec<Seen>);

impl Ieee802154RadioSink for Sink {
    fn event(&mut self, event: RadioEvent<'_>) {
        self.0.push(match event {
            RadioEvent::Received(frame) => Seen::Received {
                mac: frame.frame.bytes().to_vec(),
                channel: frame.metadata.channel.get(),
            },
            RadioEvent::TransmitDone {
                id,
                status,
                acknowledgement,
            } => Seen::TransmitDone {
                id,
                status,
                ack_pending: acknowledgement.map(|ack| ack.metadata.frame_pending),
            },
            RadioEvent::EnergyScanDone { energy_dbm, .. } => Seen::EnergyScanDone(energy_dbm),
            RadioEvent::EnergyScanFailed { .. } => Seen::EnergyScanFailed,
            RadioEvent::ClearChannelAssessmentDone { idle, .. } => {
                Seen::ClearChannelAssessmentDone { idle }
            }
            RadioEvent::ClearChannelAssessmentFailed { .. } => Seen::ClearChannelAssessmentFailed,
            RadioEvent::Fault { .. } => Seen::Fault,
        });
    }
}

/// 2006 data frame without an ACK request.
const DATA: [u8; 10] = [0x41, 0x98, 0x01, 0x34, 0x12, 0xff, 0xff, 0x78, 0x56, 0xaa];
/// The same frame requesting an ACK.
const DATA_ACK: [u8; 10] = [0x61, 0x98, 0x01, 0x34, 0x12, 0xff, 0xff, 0x78, 0x56, 0xaa];

/// `mac` as the receive DMA writes it, with RSSI and LQI after it.
fn image(mac: &[u8]) -> Vec<u8> {
    let mut image = vec![mac.len() as u8 + 2];
    image.extend_from_slice(mac);
    image.extend_from_slice(&[-60i8 as u8, 200]);
    image
}

fn channel(number: u8) -> Channel {
    Channel::new(number).unwrap()
}

struct Bench {
    radio: Ieee802154Radio<'static>,
    hw: Ieee802154LlModel,
    sink: Sink,
}

impl Bench {
    fn enabled() -> Self {
        let buffers = Box::leak(Box::new(Ieee802154EngineBuffers::new()));
        let levels = Ieee802154TxPowerLevels::new(&LEVELS).unwrap();
        let mut engine = Ieee802154Engine::new(buffers, levels, Ieee802154PibDefaults::default());
        let mut hw = Ieee802154LlModel::default();
        engine.enable();
        engine.mac_init(&mut hw, Ieee802154PibDefaults::default());
        let mut bench = Self {
            radio: Ieee802154Radio::new(engine, PLATFORM),
            hw,
            sink: Sink::default(),
        };
        bench
            .submit(RadioCommand::Enable {
                id: RequestId::new(1),
            })
            .unwrap();
        bench
    }

    fn submit(&mut self, command: RadioCommand<'_>) -> Result<(), CommandError> {
        self.radio
            .submit(&mut self.hw, command, &mut self.sink)
            .map(|_| ())
    }

    fn deliver(&mut self, mac: &[u8]) {
        let address = self.hw.rx_address.unwrap();
        assert!(self.radio.engine().model_dma_write(address, &image(mac)));
    }

    fn interrupt(&mut self, events: &[Ieee802154Event]) {
        self.hw.raise(events);
        self.radio.isr(&mut self.hw, &mut self.sink);
    }

    fn transmit(&mut self, id: u32, mac: &[u8], on: u8, mode: TxMode) -> Result<(), CommandError> {
        self.submit(RadioCommand::Transmit(TxRequest {
            id: RequestId::new(id),
            frame: FrameView::new(mac).unwrap(),
            channel: channel(on),
            mode,
            transmit_power_dbm: None,
            max_frame_retries: 0,
        }))
    }

    fn seen(&mut self) -> Vec<Seen> {
        core::mem::take(&mut self.sink.0)
    }
}

#[test]
fn admission_needs_enable() {
    let buffers = Box::leak(Box::new(Ieee802154EngineBuffers::new()));
    let levels = Ieee802154TxPowerLevels::new(&LEVELS).unwrap();
    let engine = Ieee802154Engine::new(buffers, levels, Ieee802154PibDefaults::default());
    let mut radio = Ieee802154Radio::new(engine, PLATFORM);
    assert_eq!(
        radio.submit(
            &mut Ieee802154LlModel::default(),
            RadioCommand::Sleep {
                id: RequestId::new(1)
            },
            &mut Sink::default()
        ),
        Err(CommandError::Disabled)
    );
    assert_eq!(radio.state(), RadioState::Disabled);
}

#[test]
fn received_frames_are_lent_without_their_rssi_and_lqi() {
    let mut bench = Bench::enabled();
    bench
        .submit(RadioCommand::Receive {
            id: RequestId::new(2),
            channel: channel(15),
        })
        .unwrap();
    for _ in 0..30 {
        bench.deliver(&DATA);
        bench.interrupt(&[Ieee802154Event::RxDone]);
    }
    let seen = bench.seen();
    assert_eq!(
        seen.len(),
        30,
        "slots return to the ring after each delivery"
    );
    assert_eq!(
        seen[0],
        Seen::Received {
            mac: DATA.to_vec(),
            channel: 15
        }
    );
}

/// A transmission on another channel resumes receive on the resting channel.
#[test]
fn a_transmission_resumes_receive_on_the_resting_channel() {
    let mut bench = Bench::enabled();
    bench
        .submit(RadioCommand::Receive {
            id: RequestId::new(2),
            channel: channel(15),
        })
        .unwrap();
    bench.transmit(3, &DATA, 20, TxMode::Direct).unwrap();
    assert_eq!(bench.hw.channel().unwrap().number(), 20);
    bench.interrupt(&[Ieee802154Event::TxDone]);
    assert_eq!(
        bench.seen(),
        [Seen::TransmitDone {
            id: RequestId::new(3),
            status: TxStatus::Success,
            ack_pending: None
        }]
    );
    assert_eq!(bench.hw.channel().unwrap().number(), 15);
    assert_eq!(
        bench.radio.state(),
        RadioState::Resting(RestingState::Receiving {
            channel: channel(15)
        })
    );
}

/// A frame the transmit start flushes from the ending receive is delivered.
#[test]
fn a_frame_flushed_by_a_transmit_start_is_delivered() {
    let mut bench = Bench::enabled();
    bench
        .submit(RadioCommand::Receive {
            id: RequestId::new(2),
            channel: channel(15),
        })
        .unwrap();
    bench.deliver(&DATA);
    bench.hw.raise(&[Ieee802154Event::RxDone]);
    bench.transmit(3, &DATA, 15, TxMode::Direct).unwrap();
    assert_eq!(
        bench.seen(),
        [Seen::Received {
            mac: DATA.to_vec(),
            channel: 15
        }]
    );
}

#[test]
fn an_acknowledged_transmission_reports_the_ack_pending_bit() {
    let mut bench = Bench::enabled();
    bench
        .transmit(3, &DATA_ACK, 11, TxMode::ClearChannelAssessment)
        .unwrap();
    bench.interrupt(&[Ieee802154Event::TxDone]);
    assert_eq!(bench.seen(), []);
    bench.deliver(&[0x12, 0x00, 0x01]);
    bench.interrupt(&[Ieee802154Event::AckRxDone]);
    assert_eq!(
        bench.seen(),
        [Seen::TransmitDone {
            id: RequestId::new(3),
            status: TxStatus::Success,
            ack_pending: Some(FramePending::Set)
        }]
    );
}

#[test]
fn transmit_failures_keep_their_vendor_reason() {
    for (reason, status) in [
        (Ieee802154TxAbortReason::CcaBusy, TxStatus::ChannelBusy),
        (Ieee802154TxAbortReason::CcaFailed, TxStatus::Aborted),
        (
            Ieee802154TxAbortReason::TxCoexistenceBreak,
            TxStatus::CoexistenceRejected,
        ),
        (
            Ieee802154TxAbortReason::TxSecurityError,
            TxStatus::SecurityFailure,
        ),
    ] {
        let mut bench = Bench::enabled();
        bench
            .transmit(3, &DATA, 11, TxMode::ClearChannelAssessment)
            .unwrap();
        bench.hw.tx_abort = Ieee802154TxAbortReasonObservation::Named(reason);
        bench.interrupt(&[Ieee802154Event::TxAbort]);
        assert_eq!(
            bench.seen(),
            [Seen::TransmitDone {
                id: RequestId::new(3),
                status,
                ack_pending: None
            }]
        );
    }
}

#[test]
fn energy_scans_and_cca_report_results_and_aborts() {
    let mut bench = Bench::enabled();
    let scan = |id| {
        RadioCommand::EnergyScan(EnergyScanRequest {
            id: RequestId::new(id),
            channel: channel(26),
            duration_us: 128,
        })
    };
    bench.submit(scan(2)).unwrap();
    bench.hw.ed_rss = -80;
    bench.interrupt(&[Ieee802154Event::EdDone]);
    bench.submit(scan(3)).unwrap();
    bench.hw.rx_abort = Ieee802154RxAbortReasonObservation::Named(Ieee802154RxAbortReason::EdAbort);
    bench.interrupt(&[Ieee802154Event::RxAbort]);
    bench
        .submit(RadioCommand::ClearChannelAssessment {
            id: RequestId::new(4),
            channel: channel(26),
        })
        .unwrap();
    bench.hw.cca_busy = true;
    bench.interrupt(&[Ieee802154Event::EdDone]);
    bench
        .submit(RadioCommand::ClearChannelAssessment {
            id: RequestId::new(5),
            channel: channel(26),
        })
        .unwrap();
    bench.interrupt(&[Ieee802154Event::RxAbort]);
    assert_eq!(
        bench.seen(),
        [
            Seen::EnergyScanDone(-80),
            Seen::EnergyScanFailed,
            Seen::ClearChannelAssessmentDone { idle: false },
            Seen::ClearChannelAssessmentFailed,
        ]
    );
    assert_eq!(
        bench.radio.state(),
        RadioState::Resting(RestingState::Sleeping)
    );
}

#[test]
fn configuration_reaches_the_identity_and_the_pib() {
    let mut bench = Bench::enabled();
    for configuration in [
        Configuration::PanId(0x1234),
        Configuration::ShortAddress(0x5678),
        Configuration::ExtendedAddress([1, 2, 3, 4, 5, 6, 7, 8]),
        Configuration::Promiscuous(false),
        Configuration::AutomaticAcknowledgement(false),
        Configuration::TransmitPowerDbm(0),
    ] {
        bench
            .submit(RadioCommand::Configure {
                id: RequestId::new(9),
                configuration,
            })
            .unwrap();
    }
    assert_eq!(
        (
            bench.hw.panid[0],
            bench.hw.short_address[0],
            bench.hw.extended_address[0]
        ),
        (0x1234, 0x5678, [1, 2, 3, 4, 5, 6, 7, 8])
    );
    let pib = bench.radio.engine().pib();
    assert!(!pib.promiscuous());
    assert!(!pib.auto_ack_tx());
    assert_eq!(pib.power_table(), [0; 16]);
}

/// A 2015 data frame requesting an ACK, short addresses, secured at
/// ENC-MIC-32 with key index 1, then a payload byte and its MIC.
const SECURED_2015: [u8; 20] = [
    0x69, 0xa8, 0x11, 0x34, 0x12, 0x01, 0x00, 0x02, 0x00, 0x0d, 0x01, 0x02, 0x03, 0x04, 0x01, 0xaa,
    0, 0, 0, 0,
];

impl Bench {
    fn receiving() -> Self {
        let mut bench = Self::enabled();
        bench
            .submit(RadioCommand::Receive {
                id: RequestId::new(2),
                channel: channel(15),
            })
            .unwrap();
        bench
    }

    /// The image the MAC transmits next.
    fn transmit_image(&mut self) -> Vec<u8> {
        let address = self.hw.tx_address.unwrap();
        let image = self.radio.engine().model_dma_read(address).unwrap();
        image[..=usize::from(image[0])].to_vec()
    }
}

/// With a generator and keys, a secured 2015 frame is answered with a
/// secured enhanced ACK carrying the next frame counter and the configured
/// header IEs; transmit security is armed for it and the frame is delivered
/// once the ACK is sent.
#[test]
fn a_secured_2015_frame_is_answered_with_a_secured_enhanced_ack() {
    let mut bench = Bench::receiving();
    let mut generator = Ieee802154EnhancedAckGenerator::new();
    generator.set_header_ies(&[0x00, 0x00]).unwrap();
    *generator.keys() = Some(MacKeys::new(1, [1; 16], [2; 16], [3; 16], 7));
    *bench.radio.enhanced_ack() = Some(generator);

    bench.deliver(&SECURED_2015);
    bench.interrupt(&[Ieee802154Event::RxDone]);
    assert!(bench.hw.transmit_security);
    assert_eq!(
        bench.transmit_image(),
        [
            0x15, // MAC bytes plus the FCS
            // ACK, secured, frame pending (the disabled automatic mode marks
            // every frame pending), IE present.
            0x1a, 0x2a, 0x11, 0x34, 0x12, 0x02, 0x00, //
            0x0d, 0x07, 0x00, 0x00, 0x00, 0x01, // frame counter 7, key index 1
            0x00, 0x00, // header IE
            0x00, 0x00, 0x00, 0x00, // MIC
            0x00, 0x00, // FCS
        ]
    );
    assert_eq!(
        bench
            .radio
            .enhanced_ack()
            .unwrap()
            .keys()
            .unwrap()
            .frame_counter(),
        8
    );
    assert!(bench.seen().is_empty(), "the frame waits for its ACK");

    bench.interrupt(&[Ieee802154Event::AckTxDone]);
    assert_eq!(
        bench.seen(),
        [Seen::Received {
            mac: SECURED_2015.to_vec(),
            channel: 15
        }]
    );
    assert!(!bench.hw.transmit_security);
}

/// Without keys a secured 2015 frame gets no ACK and is delivered at once,
/// as when the vendor generator fails.
#[test]
fn without_keys_a_secured_2015_frame_is_delivered_without_an_ack() {
    let mut bench = Bench::receiving();
    *bench.radio.enhanced_ack() = Some(Ieee802154EnhancedAckGenerator::new());
    bench.deliver(&SECURED_2015);
    bench.interrupt(&[Ieee802154Event::RxDone]);
    assert!(!bench.hw.transmit_security);
    assert_eq!(
        bench.seen(),
        [Seen::Received {
            mac: SECURED_2015.to_vec(),
            channel: 15
        }]
    );
}

#[test]
fn header_ies_are_bounded_by_the_port_capacity() {
    let mut generator = Ieee802154EnhancedAckGenerator::new();
    assert_eq!(
        generator.set_header_ies(&[0; IEEE802154_ENHANCED_ACK_IE_CAPACITY + 1]),
        Err(Ieee802154EnhancedAckIeTooLong)
    );
    assert!(generator.header_ies().is_empty());
    generator
        .set_header_ies(&[1; IEEE802154_ENHANCED_ACK_IE_CAPACITY])
        .unwrap();
    assert_eq!(
        generator.header_ies().len(),
        IEEE802154_ENHANCED_ACK_IE_CAPACITY
    );
}

impl Bench {
    fn tx_abort(&mut self, reason: Ieee802154TxAbortReason) {
        self.hw.tx_abort = Ieee802154TxAbortReasonObservation::Named(reason);
        self.interrupt(&[Ieee802154Event::TxAbort]);
    }
}

/// `SubMac` backs off before each CCA attempt, receiving on the transmit
/// channel while the radio receives when idle; a busy channel backs off
/// again with a larger exponent, and success resumes the resting channel.
#[test]
fn a_csma_ca_transmission_backs_off_before_each_cca_attempt() {
    let mut bench = Bench::enabled();
    bench
        .submit(RadioCommand::Receive {
            id: RequestId::new(2),
            channel: channel(15),
        })
        .unwrap();
    bench
        .transmit(3, &DATA, 20, TxMode::CsmaCa { max_backoffs: 4 })
        .unwrap();
    // Receive on the transmit channel during the backoff.
    assert_eq!(bench.hw.channel().unwrap().number(), 20);
    assert_eq!(bench.hw.command, Some(Ieee802154LlCommand::RxStart));
    // 21 % 2^3 unit periods of 320 us.
    assert_eq!(bench.radio.take_delay(), Some(5 * 320));
    assert_eq!(bench.radio.take_delay(), None, "one timer per backoff");

    bench.radio.delay_elapsed(&mut bench.hw, &mut bench.sink);
    assert_eq!(bench.hw.command, Some(Ieee802154LlCommand::CcaTxStart));
    bench.tx_abort(Ieee802154TxAbortReason::CcaBusy);
    assert!(bench.seen().is_empty(), "a busy channel backs off again");
    assert_eq!(bench.hw.command, Some(Ieee802154LlCommand::RxStart));
    // 21 % 2^4.
    assert_eq!(bench.radio.take_delay(), Some(5 * 320));

    bench.radio.delay_elapsed(&mut bench.hw, &mut bench.sink);
    bench.interrupt(&[Ieee802154Event::TxDone]);
    assert_eq!(
        bench.seen(),
        [Seen::TransmitDone {
            id: RequestId::new(3),
            status: TxStatus::Success,
            ack_pending: None
        }]
    );
    assert_eq!(bench.hw.channel().unwrap().number(), 15);
    assert_eq!(bench.radio.take_delay(), None);
}

/// A busy channel, an abort and a coexistence rejection are channel-access
/// failures; after `max_backoffs` of them the next ends the transmission
/// with a busy channel. A sleeping radio sleeps through its backoffs.
#[test]
fn channel_access_failures_end_with_a_busy_channel() {
    let mut bench = Bench::enabled();
    bench
        .transmit(3, &DATA, 11, TxMode::CsmaCa { max_backoffs: 2 })
        .unwrap();
    assert_eq!(bench.hw.command, Some(Ieee802154LlCommand::Stop));
    for reason in [
        Ieee802154TxAbortReason::CcaFailed,
        Ieee802154TxAbortReason::TxCoexistenceBreak,
    ] {
        assert!(bench.radio.take_delay().is_some());
        bench.radio.delay_elapsed(&mut bench.hw, &mut bench.sink);
        bench.tx_abort(reason);
        assert!(bench.seen().is_empty());
    }
    assert!(bench.radio.take_delay().is_some());
    bench.radio.delay_elapsed(&mut bench.hw, &mut bench.sink);
    bench.tx_abort(Ieee802154TxAbortReason::CcaBusy);
    assert_eq!(
        bench.seen(),
        [Seen::TransmitDone {
            id: RequestId::new(3),
            status: TxStatus::ChannelBusy,
            ack_pending: None
        }]
    );
    assert_eq!(bench.radio.take_delay(), None);
}

/// Other transmit failures end the transmission at once.
#[test]
fn a_security_failure_ends_a_csma_ca_transmission() {
    let mut bench = Bench::enabled();
    bench
        .transmit(3, &DATA, 11, TxMode::CsmaCa { max_backoffs: 4 })
        .unwrap();
    assert!(bench.radio.take_delay().is_some());
    bench.radio.delay_elapsed(&mut bench.hw, &mut bench.sink);
    bench.tx_abort(Ieee802154TxAbortReason::TxSecurityError);
    assert_eq!(
        bench.seen(),
        [Seen::TransmitDone {
            id: RequestId::new(3),
            status: TxStatus::SecurityFailure,
            ack_pending: None
        }]
    );
}

/// Without backoffs the frame goes out at once with one CCA.
#[test]
fn no_backoffs_transmit_at_once_with_one_cca() {
    let mut bench = Bench::enabled();
    bench
        .transmit(3, &DATA, 11, TxMode::CsmaCa { max_backoffs: 0 })
        .unwrap();
    assert_eq!(bench.hw.command, Some(Ieee802154LlCommand::CcaTxStart));
    assert_eq!(bench.radio.take_delay(), None);
    bench.tx_abort(Ieee802154TxAbortReason::CcaBusy);
    assert_eq!(
        bench.seen(),
        [Seen::TransmitDone {
            id: RequestId::new(3),
            status: TxStatus::ChannelBusy,
            ack_pending: None
        }]
    );
}

/// A frame received on the transmit channel during a backoff is delivered.
#[test]
fn a_frame_received_during_a_backoff_is_delivered() {
    let mut bench = Bench::enabled();
    bench
        .submit(RadioCommand::Receive {
            id: RequestId::new(2),
            channel: channel(15),
        })
        .unwrap();
    bench
        .transmit(3, &DATA, 15, TxMode::CsmaCa { max_backoffs: 4 })
        .unwrap();
    bench.deliver(&DATA);
    bench.interrupt(&[Ieee802154Event::RxDone]);
    assert_eq!(
        bench.seen(),
        [Seen::Received {
            mac: DATA.to_vec(),
            channel: 15
        }]
    );
}

impl Bench {
    fn transmit_retried(&mut self, mac: &[u8], mode: TxMode, retries: u8) {
        self.submit(RadioCommand::Transmit(TxRequest {
            id: RequestId::new(3),
            frame: FrameView::new(mac).unwrap(),
            channel: channel(11),
            mode,
            transmit_power_dbm: None,
            max_frame_retries: retries,
        }))
        .unwrap();
    }

    /// The frame went out, and no acknowledgement arrived.
    fn no_ack(&mut self) {
        self.interrupt(&[Ieee802154Event::TxDone]);
        self.tx_abort(Ieee802154TxAbortReason::RxAckTimeout);
    }
}

/// `SubMac` retries a frame without acknowledgement after a random delay
/// whose exponent grows from 0, and reports no acknowledgement once its
/// retries are spent.
#[test]
fn a_frame_without_acknowledgement_is_retried_after_a_growing_delay() {
    let mut bench = Bench::enabled();
    bench.transmit_retried(&DATA_ACK, TxMode::Direct, 2);
    assert_eq!(bench.hw.command, Some(Ieee802154LlCommand::TxStart));
    // 21 % 2^0, then 21 % 2^1 unit periods.
    for delay in [0, 320] {
        bench.no_ack();
        assert!(bench.seen().is_empty(), "the frame is retried");
        assert_eq!(bench.radio.take_delay(), Some(delay));
        bench.hw.command = None;
        bench.radio.delay_elapsed(&mut bench.hw, &mut bench.sink);
        assert_eq!(bench.hw.command, Some(Ieee802154LlCommand::TxStart));
    }
    bench.no_ack();
    assert_eq!(
        bench.seen(),
        [Seen::TransmitDone {
            id: RequestId::new(3),
            status: TxStatus::NoAcknowledgement,
            ack_pending: None
        }]
    );
    assert_eq!(bench.radio.take_delay(), None);
}

/// A CSMA-CA frame that finds no channel is retried with fresh backoffs.
#[test]
fn a_csma_ca_frame_without_channel_access_is_retried() {
    let mut bench = Bench::enabled();
    bench.transmit_retried(&DATA, TxMode::CsmaCa { max_backoffs: 1 }, 1);
    // Two CCA attempts per channel acquisition, two acquisitions.
    for _ in 0..4 {
        assert!(bench.seen().is_empty());
        assert!(bench.radio.take_delay().is_some());
        bench.radio.delay_elapsed(&mut bench.hw, &mut bench.sink);
        assert_eq!(bench.hw.command, Some(Ieee802154LlCommand::CcaTxStart));
        bench.tx_abort(Ieee802154TxAbortReason::CcaBusy);
    }
    assert_eq!(
        bench.seen(),
        [Seen::TransmitDone {
            id: RequestId::new(3),
            status: TxStatus::ChannelBusy,
            ack_pending: None
        }]
    );
}

/// A success after a retry ends the transmission as usual.
#[test]
fn a_retried_frame_can_succeed() {
    let mut bench = Bench::enabled();
    bench.transmit_retried(&DATA_ACK, TxMode::ClearChannelAssessment, 3);
    bench.tx_abort(Ieee802154TxAbortReason::CcaBusy);
    assert!(bench.seen().is_empty());
    // A channel-access retry starts at once, with no delay.
    assert_eq!(bench.radio.take_delay(), None);
    assert_eq!(bench.hw.command, Some(Ieee802154LlCommand::CcaTxStart));
    bench.interrupt(&[Ieee802154Event::TxDone]);
    bench.deliver(&[0x02, 0x00, 0x01]);
    bench.interrupt(&[Ieee802154Event::AckRxDone]);
    assert!(matches!(
        bench.seen().as_slice(),
        [Seen::TransmitDone {
            status: TxStatus::Success,
            ..
        }]
    ));
}

/// Security armed for the transmission is armed again before each later
/// attempt, since the engine clears it when an attempt ends.
#[test]
fn transmit_security_is_armed_again_for_each_retry() {
    // 2006 data frame requesting an ACK, secured at ENC-MIC-32 key mode 0.
    let secured = [
        0x69, 0x98, 0x01, 0x34, 0x12, 0xff, 0xff, 0x78, 0x56, 0x05, 1, 2, 3, 4, 0xaa, 0, 0, 0, 0,
    ];
    let mut bench = Bench::enabled();
    let mut image = vec![secured.len() as u8 + 2];
    image.extend_from_slice(&secured);
    bench
        .radio
        .set_transmit_security(&mut bench.hw, &image, &[7; 16], &[9; 8]);
    bench.transmit_retried(&secured, TxMode::Direct, 1);
    assert!(bench.hw.transmit_security);
    bench.no_ack();
    assert!(!bench.hw.transmit_security, "the failed attempt cleared it");
    assert!(bench.radio.take_delay().is_some());
    bench.radio.delay_elapsed(&mut bench.hw, &mut bench.sink);
    assert!(bench.hw.transmit_security, "the retry armed it again");
}
