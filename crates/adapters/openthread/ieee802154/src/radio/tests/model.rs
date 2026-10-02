//! The OpenThread radio over a host model of the radio port: no runtime,
//! engine or chip, which proves the adapter needs nothing but the portable
//! port. The model admits commands through the portable state machine,
//! completes each operation at once and records the settings it received.

use core::cell::RefCell;
use core::task::Poll;
use std::{collections::VecDeque, vec, vec::Vec};

use embassy_futures::block_on;
use oer_ieee802154::{
    AcceptedCommand, ClockInfo, CommandError, Configuration, CslReceiver, EnhancedAckGeneration,
    EventsLost, FailureClass, FcsStatus, Frame, FrameCounterUpdate, FramePending,
    Ieee802154Capabilities, Ieee802154RadioPort, Interface, LifecycleCommand, LifecycleError,
    LifecycleEvent, LinkMetrics, MacKeys, PortError, ProbingInitiator, RadioCapabilities,
    RadioCommand, RadioEvent, RadioInstant, RadioSetting, RadioState, RadioStateMachine,
    ReceivedFrame, RequestId, RxMetadata, SecurityStatus, SentAcknowledgement, SettingError,
    TxSecurity, TxStatus,
};
use openthread_radio::{
    CslConfig, EnhAckProbingConfig, EnhAckProbingInitiator, FrameCounterUpdate as OtCounter,
    MacKeys as OtKeys, Radio, TxFrame,
};

use super::super::{OpenThreadRadio, OpenThreadRadioDefaults};

/// The model's owned event.
#[derive(Clone, Debug)]
enum ModelEvent {
    Received(Frame, RxMetadata),
    /// An event without frame bytes.
    Other(RadioEvent<'static>),
}

/// What the model recorded.
#[derive(Default)]
struct Recorded {
    commands: Vec<RadioCommand<'static>>,
    transmitted: Vec<(Vec<u8>, u8, TxSecurity)>,
    keys: Option<MacKeys>,
    csl: CslReceiver,
    enhanced_ack: Option<EnhancedAckGeneration>,
    probing: Vec<ProbingInitiator>,
}

struct Model {
    machine: RadioStateMachine,
    events: VecDeque<Result<ModelEvent, EventsLost>>,
    recorded: Recorded,
}

/// A radio port without hardware.
struct ModelPort(RefCell<Model>);

/// The model never fails as a whole.
#[derive(Debug)]
struct Never;

impl PortError for Never {
    fn class(&self) -> FailureClass {
        FailureClass::Rejected
    }
}

const CAPABILITIES: RadioCapabilities = RadioCapabilities::CLEAR_CHANNEL_ASSESSMENT
    .union(RadioCapabilities::ENERGY_SCAN)
    .union(RadioCapabilities::HARDWARE_ACKNOWLEDGEMENT)
    .union(RadioCapabilities::SCHEDULED_TRANSMIT)
    .union(RadioCapabilities::SCHEDULED_RECEIVE)
    .union(RadioCapabilities::TRANSMIT_POWER)
    .union(RadioCapabilities::PROMISCUOUS)
    .union(RadioCapabilities::SOURCE_MATCH)
    .union(RadioCapabilities::TIME_SYNC);

impl ModelPort {
    fn new() -> Self {
        Self(RefCell::new(Model {
            machine: RadioStateMachine::new(CAPABILITIES),
            events: VecDeque::new(),
            recorded: Recorded::default(),
        }))
    }

    /// Complete the running operation with `event`.
    fn complete(model: &mut Model, event: RadioEvent<'static>) {
        model
            .machine
            .observe(event)
            .expect("a valid terminal event");
        model.events.push_back(Ok(ModelEvent::Other(event)));
    }

    /// The port drops events: a loss marker follows the queued ones.
    fn lose(&self) {
        self.0.borrow_mut().events.push_back(Err(EventsLost));
    }

    /// A frame arrives on the receive channel.
    fn arrive(&self, bytes: &[u8], channel: u8) {
        let metadata = RxMetadata {
            channel: oer_ieee802154::Channel::new(channel).unwrap(),
            rssi_dbm: -48,
            link_quality: 170,
            timestamp: Some(RadioInstant::from_micros(77)),
            fcs: FcsStatus::Valid,
            security: SecurityStatus::Unprocessed,
            frame_pending: FramePending::Unavailable,
            sent_acknowledgement: SentAcknowledgement::NONE,
            interface: Some(Interface::PRIMARY),
        };
        let frame = Frame::try_from_bytes(bytes).unwrap();
        let mut model = self.0.borrow_mut();
        model
            .machine
            .observe(RadioEvent::Received(ReceivedFrame {
                frame: frame.view(),
                metadata,
            }))
            .expect("the model receives");
        model
            .events
            .push_back(Ok(ModelEvent::Received(frame, metadata)));
    }
}

impl Ieee802154RadioPort for ModelPort {
    type Event = ModelEvent;
    type Error = Never;

    fn view(event: &ModelEvent) -> RadioEvent<'_> {
        match event {
            ModelEvent::Received(frame, metadata) => RadioEvent::Received(ReceivedFrame {
                frame: frame.view(),
                metadata: *metadata,
            }),
            ModelEvent::Other(event) => *event,
        }
    }

    fn capabilities(&self) -> Ieee802154Capabilities {
        Ieee802154Capabilities {
            operations: CAPABILITIES,
            interfaces: 1,
        }
    }

    fn lifecycle(&self, command: LifecycleCommand) -> Result<Result<(), LifecycleError>, Never> {
        let id = RequestId::new(u32::MAX);
        let (command, terminal) = match command {
            LifecycleCommand::Enable => (RadioCommand::Enable { id }, LifecycleEvent::Enabled),
            LifecycleCommand::Disable => (RadioCommand::Disable { id }, LifecycleEvent::Disabled),
            LifecycleCommand::Quiesce => return Ok(Err(LifecycleError::InvalidState)),
        };
        let mut model = self.0.borrow_mut();
        Ok(match model.machine.admit(command) {
            Ok(_) => {
                model.recorded.commands.push(command);
                model
                    .events
                    .push_back(Ok(ModelEvent::Other(RadioEvent::Lifecycle(terminal))));
                Ok(())
            }
            Err(CommandError::AlreadyEnabled) => Err(LifecycleError::AlreadyInState),
            Err(_) => Err(LifecycleError::InvalidState),
        })
    }

    fn submit(
        &self,
        command: RadioCommand<'_>,
    ) -> Result<Result<AcceptedCommand, CommandError>, Never> {
        let mut model = self.0.borrow_mut();
        let accepted = match model.machine.admit(command) {
            Ok(accepted) => accepted,
            refused => return Ok(refused),
        };
        let id = command.id();
        match command {
            RadioCommand::Transmit(request) => {
                model.recorded.transmitted.push((
                    request.frame.bytes().to_vec(),
                    request.channel.get(),
                    request.security,
                ));
                Self::complete(
                    &mut model,
                    RadioEvent::TransmitDone {
                        id,
                        status: TxStatus::Success,
                        acknowledgement: None,
                        security: None,
                    },
                );
            }
            RadioCommand::EnergyScan(_) => Self::complete(
                &mut model,
                RadioEvent::EnergyScanDone {
                    id,
                    energy_dbm: -50,
                },
            ),
            RadioCommand::Configure { configuration, .. } => {
                model
                    .recorded
                    .commands
                    .push(RadioCommand::Configure { id, configuration });
            }
            RadioCommand::Enable { .. } => {
                model.recorded.commands.push(RadioCommand::Enable { id })
            }
            RadioCommand::Receive { channel, .. } => {
                model
                    .recorded
                    .commands
                    .push(RadioCommand::Receive { id, channel });
            }
            _ => {}
        }
        Ok(Ok(accepted))
    }

    async fn next_event(&self) -> Result<ModelEvent, EventsLost> {
        core::future::poll_fn(|_| match self.0.borrow_mut().events.pop_front() {
            Some(event) => Poll::Ready(event),
            None => Poll::Pending,
        })
        .await
    }

    fn now(&self) -> Result<RadioInstant, Never> {
        Ok(RadioInstant::from_micros(1_000))
    }

    fn clock_info(&self) -> ClockInfo {
        ClockInfo::MONOTONIC_MICROS
    }

    fn state(&self) -> Result<RadioState, Never> {
        Ok(self.0.borrow().machine.state())
    }

    fn apply(&self, setting: RadioSetting<'_>) -> Result<Result<(), SettingError>, Never> {
        let recorded = &mut self.0.borrow_mut().recorded;
        match setting {
            RadioSetting::MacKeys {
                key_id,
                previous,
                current,
                next,
                ..
            } => recorded
                .keys
                .get_or_insert(MacKeys::ZEROED)
                .set_keys(key_id, previous, current, next),
            RadioSetting::FrameCounter { update, .. } => {
                let keys = recorded.keys.get_or_insert(MacKeys::ZEROED);
                match update {
                    FrameCounterUpdate::Set(counter) => keys.set_frame_counter(counter),
                    FrameCounterUpdate::SetIfLarger(counter) => {
                        keys.set_frame_counter_if_larger(counter);
                    }
                }
            }
            RadioSetting::Csl(csl) => recorded.csl = csl,
            RadioSetting::EnhancedAck(generation) => recorded.enhanced_ack = generation,
            RadioSetting::EnhancedAckProbing(initiators) => {
                recorded.probing = initiators.to_vec();
            }
            _ => return Ok(Err(SettingError::Unsupported)),
        }
        Ok(Ok(()))
    }

    fn frame_counter(&self, _: Interface) -> Result<Option<u32>, Never> {
        Ok(self
            .0
            .borrow()
            .recorded
            .keys
            .map(|keys| keys.frame_counter()))
    }

    fn recent_rssi(&self) -> Result<i8, Never> {
        Ok(-71)
    }
}

const DEFAULTS: OpenThreadRadioDefaults = OpenThreadRadioDefaults::esp_idf(8, -97);

/// A frame without an ACK request, with the FCS OpenThread counts.
const PSDU: [u8; 11] = [0x41, 0x98, 0x01, 0x34, 0x12, 0x78, 0x56, 0xaa, 0xbb, 0, 0];

/// OpenThread's whole radio life cycle reaches the model port as portable
/// commands and settings.
#[test]
fn the_adapter_drives_a_host_model_port() {
    let port = ModelPort::new();
    let mut radio = OpenThreadRadio::<'_, _, 4>::new(&port, || 1_000, || Some(-71), DEFAULTS);

    let caps = block_on(radio.init()).unwrap();
    assert_eq!(
        (
            caps.default_tx_power,
            caps.default_cca_threshold,
            caps.receive_sensitivity
        ),
        (8, -75, -97)
    );
    {
        let model = port.0.borrow();
        assert_eq!(
            model.machine.state(),
            RadioState::Resting(oer_ieee802154::RestingState::Sleeping)
        );
        // The zeroed keys, and a generator measuring from the sensitivity.
        assert_eq!(model.recorded.keys, Some(MacKeys::ZEROED));
        assert_eq!(
            model.recorded.enhanced_ack,
            Some(EnhancedAckGeneration {
                noise_floor_dbm: -97
            })
        );
        assert!(model.recorded.commands.iter().any(|command| matches!(
            command,
            RadioCommand::Configure {
                configuration: Configuration::CcaThresholdDbm(-75),
                ..
            }
        )));
    }
    assert_eq!(radio.clock()(), 1_000);

    block_on(radio.set_csl(CslConfig {
        period: 0x1_0005,
        sample_time: 9,
    }))
    .unwrap();
    block_on(radio.set_mac_keys(&OtKeys {
        key_id_mode: 1,
        key_id: 4,
        previous: [1; 16],
        current: [2; 16],
        next: [3; 16],
    }))
    .unwrap();
    block_on(radio.set_mac_frame_counter(OtCounter::Set(30))).unwrap();
    block_on(radio.set_mac_frame_counter(OtCounter::SetIfLarger(20))).unwrap();
    let mut config = EnhAckProbingConfig::default();
    let lqi = openthread_radio::LinkMetrics {
        lqi: true,
        ..Default::default()
    };
    config
        .initiators
        .push(EnhAckProbingInitiator {
            short_address: 0x0102,
            ext_address: [1, 2, 3, 4, 5, 6, 7, 8],
            metrics: lqi,
        })
        .unwrap();
    block_on(radio.set_enh_ack_probing(&config)).unwrap();
    {
        let model = port.0.borrow();
        assert_eq!(
            model.recorded.csl,
            CslReceiver {
                period: 5,
                sample_time: 9
            }
        );
        let mut keys = model.recorded.keys.unwrap();
        assert_eq!(keys.frame_counter(), 30);
        let security = keys.transmit_security(false);
        assert_eq!((security.key_id, security.key), (Some(4), [2; 16]));
        assert_eq!(
            model.recorded.probing,
            vec![ProbingInitiator {
                short_address: 0x0102,
                extended_address: [8, 7, 6, 5, 4, 3, 2, 1],
                metrics: LinkMetrics {
                    lqi: true,
                    ..LinkMetrics::NONE
                },
            }]
        );
    }

    // A frame goes out as OpenThread secured it, then the scan reports.
    let mut psdu = PSDU;
    let mut frame = TxFrame {
        psdu: &mut psdu,
        channel: 15,
        power: 0,
        cca_threshold: None,
        tx_at: None,
        retransmission: false,
        security_processed: true,
        header_updated: false,
        time_sync: None,
    };
    assert_eq!(block_on(radio.transmit_frame(&mut frame, None)), Ok(None));
    assert_eq!(
        port.0.borrow().recorded.transmitted,
        vec![(PSDU[..9].to_vec(), 15, TxSecurity::Processed)]
    );
    assert_eq!(block_on(radio.energy_scan(20, 1)), Ok(-50));

    // A received frame reaches OpenThread with its metadata.
    block_on(radio.set_receive(15)).unwrap();
    port.arrive(&PSDU[..9], 15);
    let mut buffer = [0; 127];
    let meta = block_on(radio.receive(&mut buffer)).unwrap();
    assert_eq!(&buffer[..meta.len], &PSDU);
    assert_eq!(
        (meta.channel, meta.rssi, meta.lqi, meta.timestamp),
        (15, Some(-48), Some(170), Some(77))
    );
}

/// A refusal of the model port reaches OpenThread as an error, and the
/// adapter keeps serving.
#[test]
fn a_port_refusal_is_an_openthread_error() {
    let port = ModelPort::new();
    let mut radio = OpenThreadRadio::<'_, _, 4>::new(&port, || 1_000, || None, DEFAULTS);
    // Not enabled yet: the model refuses receive mode.
    assert_eq!(
        block_on(radio.set_receive(15)),
        Err(openthread_radio::RadioErrorKind::Other)
    );
    block_on(radio.init()).unwrap();
    assert_eq!(block_on(radio.set_receive(15)), Ok(()));
}

/// A loss of events reaches OpenThread as one failed reception after the
/// frames received before it, and reception goes on.
#[test]
fn lost_events_are_a_failed_reception() {
    let port = ModelPort::new();
    let mut radio = OpenThreadRadio::<'_, _, 4>::new(&port, || 1_000, || None, DEFAULTS);
    block_on(radio.init()).unwrap();
    block_on(radio.set_receive(15)).unwrap();
    port.arrive(&PSDU[..9], 15);
    port.lose();
    port.arrive(&PSDU[..9], 15);
    let mut buffer = [0; 127];
    assert!(block_on(radio.receive(&mut buffer)).is_ok());
    assert_eq!(
        block_on(radio.receive(&mut buffer)),
        Err(openthread_radio::RadioErrorKind::RxFailed)
    );
    assert!(block_on(radio.receive(&mut buffer)).is_ok());
}

/// `init` enables through the port's lifecycle and takes its terminal
/// event; a second `init` finds the radio enabled.
#[test]
fn init_takes_the_enable_terminal_event() {
    let port = ModelPort::new();
    let mut radio = OpenThreadRadio::<'_, _, 4>::new(&port, || 1_000, || None, DEFAULTS);
    block_on(radio.init()).unwrap();
    assert!(port.0.borrow().events.is_empty());
    block_on(radio.init()).unwrap();
}
