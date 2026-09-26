//! Role behavior over the HAL register model: admission, event correlation,
//! frame lending and resume. The engine's vendor sequences are covered by
//! the engine tests and the host stand.

use std::{boxed::Box, vec, vec::Vec};

use oer_esp32s31_hal::ieee802154::{
    Ieee802154TxPowerLevels,
    ll::model::Ieee802154LlModel,
    mac::{
        Ieee802154Event, Ieee802154RxAbortReason, Ieee802154RxAbortReasonObservation,
        Ieee802154TxAbortReason, Ieee802154TxAbortReasonObservation,
    },
    pib::Ieee802154PibDefaults,
};
use oer_esp32s31_ieee802154::engine::{Ieee802154Engine, Ieee802154EngineBuffers};
use oer_ieee802154::{
    Channel, CommandError, Configuration, EnergyScanRequest, FramePending, FrameView,
    RadioCapabilities, RadioCommand, RadioEvent, RadioState, RequestId, RestingState, TxMode,
    TxRequest, TxStatus,
};

use super::{Ieee802154Platform, Ieee802154Radio, Ieee802154RadioSink};

static LEVELS: [i8; 3] = [-9, 0, 10];

const PLATFORM: Ieee802154Platform = Ieee802154Platform {
    now_micros: || 42,
    enhanced_ack: None,
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
        }))
    }

    fn seen(&mut self) -> Vec<Seen> {
        core::mem::take(&mut self.sink.0)
    }
}

#[test]
fn csma_ca_is_not_advertised_and_admission_needs_enable() {
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

    let mut bench = Bench::enabled();
    assert_eq!(
        bench.transmit(2, &DATA, 11, TxMode::CsmaCa { max_backoffs: 4 }),
        Err(CommandError::Unsupported {
            command: oer_ieee802154::CommandKind::Transmit,
            required: RadioCapabilities::CSMA_CA,
        })
    );
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
