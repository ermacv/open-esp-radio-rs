use std::vec::Vec;

use oer_esp32s31_bluetooth_memory::BlePhyLe1MPacketStartCalibration;

use oer_bluetooth_radio::{
    AccessAddress, AdvertisingChannel, AdvertisingChannels, AdvertisingConfiguration,
    AdvertisingEvent, AdvertisingPdu, AdvertisingSetId, ConnectionConfiguration, ConnectionEvent,
    ConnectionEventTiming, ConnectionId, CrcInit, DataChannel, DataPdu, DataPduKind, EventId,
    EventResult, RadioDuration, RadioInstant, RadioOutcome, RadioRequest, RadioWindow,
    RequestError, ScanWindow, ScannerConfiguration, ScannerId, TestChannel, TestPhy, TestReceive,
    TestReport, TxPower,
};
use oer_esp32s31_bluetooth::{
    ControllerTimeSample,
    scheduler::{
        SchedulerAction, SchedulerHardwareView, SchedulerNext, SchedulerObservation,
        SchedulerSoftwareConfig,
    },
};
use oer_esp32s31_hal::bluetooth::{
    BluetoothControllerHalInitConfig, BluetoothSchedulerExecutionModifyDisposition,
    BluetoothSchedulerWorkObservation,
};

use super::{BluetoothRadio, BluetoothRadioSink, RadioStep};

type Radio = BluetoothRadio<1, 1, 1, 1, 2, 3, 8>;

/// Owned summary of one outcome.
#[derive(Clone, Debug, Eq, PartialEq)]
enum Seen {
    Received(EventId, Vec<u8>),
    Ended(EventId, bool),
    Anchor(EventId, Option<RadioInstant>),
    Acknowledged(ConnectionId),
    Test(EventId, TestReport),
    Fault,
}

#[derive(Default)]
struct Sink(Vec<Seen>, Vec<Option<RadioInstant>>);

impl BluetoothRadioSink for Sink {
    fn outcome(&mut self, outcome: RadioOutcome<'_>) {
        if let RadioOutcome::Received { pdu, .. } = outcome {
            self.1.push(pdu.captured_at);
        }
        self.0.push(match outcome {
            RadioOutcome::Received { id, pdu } => Seen::Received(id, pdu.pdu.to_vec()),
            RadioOutcome::EventEnded {
                id,
                result:
                    EventResult::Executed {
                        anchor: Some(anchor),
                    },
            } => Seen::Anchor(id, Some(anchor)),
            RadioOutcome::EventEnded { id, result } => {
                Seen::Ended(id, matches!(result, EventResult::Executed { .. }))
            }
            RadioOutcome::TransmitAcknowledged(connection) => Seen::Acknowledged(connection),
            RadioOutcome::TestReport { id, report } => Seen::Test(id, report),
            RadioOutcome::Fault(_) => Seen::Fault,
        });
    }
}

fn radio() -> Radio {
    Radio::new(
        crate::validation::model_memory(),
        SchedulerSoftwareConfig::reviewed_standalone(),
        BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale(),
        &ControllerTimeSample::for_validation(0),
        500,
    )
}

fn view(busy: bool) -> SchedulerHardwareView {
    SchedulerHardwareView {
        work: BluetoothSchedulerWorkObservation::from_fields_for_validation(busy, false, 0),
        hardware_head: None,
    }
}

fn window(start: u64, duration: u32) -> RadioWindow {
    RadioWindow::new(
        RadioInstant::from_micros(start),
        RadioDuration::from_micros(duration),
    )
    .unwrap()
}

const NONCONN: [u8; 8] = [0x02, 6, 1, 2, 3, 4, 5, 6];
const ADV_IND: [u8; 8] = [0x00, 6, 1, 2, 3, 4, 5, 6];
const SCAN_RSP: [u8; 8] = [0x04, 6, 1, 2, 3, 4, 5, 6];

fn configure_legacy(radio: &mut Radio, sink: &mut Sink) {
    radio
        .request(
            RadioRequest::ConfigureAdvertising(AdvertisingConfiguration {
                set: AdvertisingSetId::new(0),
                pdu: AdvertisingPdu::new(&NONCONN).unwrap(),
                scan_response: None,
                tx_power: TxPower::from_dbm(0),
            }),
            sink,
        )
        .unwrap();
}

fn advertise(id: u32, anchor: u64, channels: AdvertisingChannels) -> RadioRequest<'static> {
    RadioRequest::Advertise(AdvertisingEvent {
        id: EventId::new(id),
        set: AdvertisingSetId::new(0),
        anchor: RadioInstant::from_micros(anchor),
        channels,
        channel_spacing: RadioDuration::from_micros(1_000),
    })
}

/// Record a status in every listed item, as hardware does.
fn execute_all(radio: &Radio, status: u32) {
    let space = radio.item_space();
    for (id, _) in radio.executor.list().iter() {
        space.record_status_for_validation(id, status);
    }
}

#[test]
fn the_timing_follows_the_scheduler_policy() {
    let radio = radio();
    assert_eq!(radio.timing().preparation_lead.as_micros(), 107);
    assert_eq!(radio.timing().admission_guard.as_micros(), 40);
    assert_eq!(radio.now(), RadioInstant::from_micros(0));
    // A recurring event ends 5,154 us after its widened anchor less the lead.
    let connection = radio.timing().connection;
    assert_eq!(connection.local_sleep_clock_ppm, 500);
    assert_eq!(connection.event_length.as_micros(), 5_154 - 107);
    assert_eq!(connection.first_event_length.as_micros(), 5_155);
}

#[test]
fn an_idle_scheduler_starts_at_the_first_channel_and_ends_the_event_once() {
    let mut radio = radio();
    let mut sink = Sink::default();
    configure_legacy(&mut radio, &mut sink);
    radio
        .request(advertise(1, 10_000, AdvertisingChannels::ALL), &mut sink)
        .unwrap();
    assert_eq!(
        radio.request(advertise(2, 20_000, AdvertisingChannels::ALL), &mut sink),
        Err(RequestError::Busy)
    );

    let RadioStep::Start(start) = radio.drive(view(false), &mut sink) else {
        panic!("an idle scheduler starts at once")
    };
    assert_eq!(radio.executor.list().len(), 3);
    let first = radio.executor.list().head().unwrap().0;
    assert_eq!(start.head, radio.item_space().link(first));
    assert!(matches!(
        radio.drive(view(true), &mut sink),
        RadioStep::Idle
    ));

    radio.complete(&mut sink);
    assert!(sink.0.is_empty());
    execute_all(&radio, 0);
    radio.complete(&mut sink);
    assert_eq!(sink.0, [Seen::Ended(EventId::new(1), true)]);
    // The set advertises again.
    radio
        .request(advertise(2, 20_000, AdvertisingChannels::ALL), &mut sink)
        .unwrap();
}

#[test]
fn reservations_are_admitted_in_time_and_apart() {
    let mut radio = radio();
    let mut sink = Sink::default();
    configure_legacy(&mut radio, &mut sink);
    // The preparation lead and the admission guard do not fit before 100.
    assert_eq!(
        radio.request(advertise(1, 100, AdvertisingChannels::ALL), &mut sink),
        Err(RequestError::TooLate)
    );
    assert_eq!(
        radio.request(advertise(1, 1 << 31, AdvertisingChannels::ALL), &mut sink),
        Err(RequestError::TooFar)
    );
    radio
        .request(advertise(1, 10_000, AdvertisingChannels::ALL), &mut sink)
        .unwrap();
    radio
        .request(
            RadioRequest::ConfigureScanner(ScannerConfiguration {
                scanner: ScannerId::new(0),
                tx_power: TxPower::from_dbm(0),
            }),
            &mut sink,
        )
        .unwrap();
    let scan = |start| {
        RadioRequest::Scan(ScanWindow {
            id: EventId::new(5),
            scanner: ScannerId::new(0),
            channel: AdvertisingChannel::Channel37,
            window: window(start, 500),
        })
    };
    assert_eq!(
        radio.request(scan(11_000), &mut sink),
        Err(RequestError::Overlap)
    );
    radio.request(scan(13_200), &mut sink).unwrap();
    assert_eq!(
        radio.request(RadioRequest::RemoveScanner(ScannerId::new(0)), &mut sink),
        Err(RequestError::Busy)
    );
}

#[test]
fn a_running_scheduler_inserts_one_item_per_transaction() {
    let mut radio = radio();
    let mut sink = Sink::default();
    configure_legacy(&mut radio, &mut sink);
    radio
        .request(
            advertise(
                1,
                10_000,
                AdvertisingChannels::new(true, true, false).unwrap(),
            ),
            &mut sink,
        )
        .unwrap();
    for _ in 0..2 {
        let RadioStep::Transaction(step) = radio.drive(view(true), &mut sink) else {
            panic!("a running scheduler needs a live insertion")
        };
        assert!(step.actions().any(|action| matches!(
            action,
            SchedulerAction::PublishExecutionModify | SchedulerAction::PublishExecutionLock(_)
        )));
        // Nothing else starts while the insertion waits.
        assert!(matches!(
            radio.drive(view(true), &mut sink),
            RadioStep::Idle
        ));
        let mut step = step;
        loop {
            let observation = match step.next() {
                SchedulerNext::Finished => break,
                SchedulerNext::Await(_) => {
                    if step
                        .actions()
                        .any(|action| matches!(action, SchedulerAction::PublishExecutionLock(_)))
                    {
                        SchedulerObservation::ExecutionLock(
                            oer_esp32s31_hal::bluetooth::BluetoothSchedulerExecutionLockDisposition::ReconcileCurrentHead,
                        )
                    } else {
                        SchedulerObservation::ExecutionModify(
                            BluetoothSchedulerExecutionModifyDisposition::Ready,
                        )
                    }
                }
            };
            let RadioStep::Transaction(next) = radio.advance(observation, &mut sink).unwrap()
            else {
                panic!("an insertion continues as a transaction")
            };
            step = next;
        }
    }
    assert_eq!(radio.executor.list().len(), 2);
    assert!(matches!(
        radio.drive(view(true), &mut sink),
        RadioStep::Idle
    ));
}

#[test]
fn a_waiting_event_is_cancelled_at_once_and_a_listed_one_on_the_next_drive() {
    let mut radio = radio();
    let mut sink = Sink::default();
    configure_legacy(&mut radio, &mut sink);
    radio
        .request(advertise(1, 10_000, AdvertisingChannels::ALL), &mut sink)
        .unwrap();
    radio
        .request(RadioRequest::Cancel(EventId::new(1)), &mut sink)
        .unwrap();
    assert_eq!(sink.0, [Seen::Ended(EventId::new(1), false)]);
    assert!(matches!(
        radio.drive(view(false), &mut sink),
        RadioStep::Idle
    ));

    radio
        .request(advertise(2, 20_000, AdvertisingChannels::ALL), &mut sink)
        .unwrap();
    let RadioStep::Start(_) = radio.drive(view(false), &mut sink) else {
        panic!("the event starts")
    };
    radio
        .request(RadioRequest::Cancel(EventId::new(2)), &mut sink)
        .unwrap();
    assert_eq!(
        radio.request(RadioRequest::Cancel(EventId::new(9)), &mut sink),
        Err(RequestError::UnknownEvent)
    );
    // The scheduler has stopped by now: the idle path republishes the head.
    let RadioStep::Transaction(step) = radio.drive(view(false), &mut sink) else {
        panic!("a listed event needs a cancellation")
    };
    assert_eq!(
        step.actions().collect::<Vec<_>>(),
        [SchedulerAction::PublishHead(None)]
    );
    assert_eq!(sink.0.last(), Some(&Seen::Ended(EventId::new(2), false)));
    assert!(radio.executor.list().is_empty());
}

#[test]
fn a_connectable_set_receives_its_requests() {
    let mut radio = radio();
    let mut sink = Sink::default();
    radio
        .request(
            RadioRequest::ConfigureAdvertising(AdvertisingConfiguration {
                set: AdvertisingSetId::new(3),
                pdu: AdvertisingPdu::new(&ADV_IND).unwrap(),
                scan_response: Some(AdvertisingPdu::new(&SCAN_RSP).unwrap()),
                tx_power: TxPower::from_dbm(0),
            }),
            &mut sink,
        )
        .unwrap();
    let event = |channels| {
        RadioRequest::Advertise(AdvertisingEvent {
            id: EventId::new(7),
            set: AdvertisingSetId::new(3),
            anchor: RadioInstant::from_micros(10_000),
            channels,
            channel_spacing: RadioDuration::from_micros(600),
        })
    };
    // Every primary channel gets its own item.
    {
        let mut all = self::radio();
        all.request(
            RadioRequest::ConfigureAdvertising(AdvertisingConfiguration {
                set: AdvertisingSetId::new(3),
                pdu: AdvertisingPdu::new(&ADV_IND).unwrap(),
                scan_response: Some(AdvertisingPdu::new(&SCAN_RSP).unwrap()),
                tx_power: TxPower::from_dbm(0),
            }),
            &mut sink,
        )
        .unwrap();
        all.request(event(AdvertisingChannels::ALL), &mut sink)
            .unwrap();
        assert_eq!(all.pending.iter().flatten().count(), 3);
        assert_eq!(
            all.request(event(AdvertisingChannels::ALL), &mut sink),
            Err(RequestError::Busy)
        );
    }
    radio
        .request(
            event(AdvertisingChannels::single(AdvertisingChannel::Channel38)),
            &mut sink,
        )
        .unwrap();
    let RadioStep::Start(_) = radio.drive(view(false), &mut sink) else {
        panic!("the event starts")
    };
    let instance = radio.connectable[0].as_ref().unwrap();
    let source = radio
        .memory
        .connectable
        .receive_source(&instance.instance)
        .unwrap();
    let scan_request = [0x03, 12, 9, 9, 9, 9, 9, 9, 1, 2, 3, 4, 5, 6];
    assert!(
        radio
            .memory
            .non_scanning
            .emulate_receive_for_validation(&scan_request, source)
    );
    execute_all(&radio, 0);
    radio.complete(&mut sink);
    assert_eq!(
        sink.0,
        [
            Seen::Received(EventId::new(7), scan_request.to_vec()),
            Seen::Ended(EventId::new(7), true)
        ]
    );
    assert_eq!(sink.1.len(), 1);
}

#[test]
fn captures_become_the_on_air_packet_start() {
    let radio = radio();
    let delay = BlePhyLe1MPacketStartCalibration::le_1m().capture_delay_micros();
    assert!(delay > 0);
    let raw = radio.clock.raw(20_000);
    assert_eq!(
        radio.clock.packet_start(raw).as_micros(),
        radio.clock.instant(raw).as_micros() - u64::from(delay)
    );
}

#[test]
fn a_connection_reports_its_anchor_receptions_and_acknowledgement() {
    let mut radio = radio();
    let mut sink = Sink::default();
    let connection = ConnectionId::new(0);
    radio
        .request(
            RadioRequest::OpenConnection(ConnectionConfiguration {
                connection,
                access_address: AccessAddress([0xd4, 0xc3, 0xb2, 0xa1]),
                crc_init: CrcInit([0x33, 0x22, 0x11]),
                created_at: RadioInstant::from_micros(1_000),
                tx_power: TxPower::from_dbm(0),
            }),
            &mut sink,
        )
        .unwrap();
    let event = |id, start, timing| {
        RadioRequest::ConnectionEvent(ConnectionEvent {
            id: EventId::new(id),
            connection,
            channel: DataChannel::new(3).unwrap(),
            window: window(start, 2_000),
            timing,
            priority: 13,
        })
    };
    radio
        .request(
            RadioRequest::Transmit {
                connection,
                pdu: DataPdu::new(DataPduKind::Control, &[0x0c, 1]).unwrap(),
            },
            &mut sink,
        )
        .unwrap();
    radio
        .request(
            event(
                1,
                10_000,
                ConnectionEventTiming::First {
                    transmit_window: RadioDuration::from_micros(1_250),
                    timing_guard: RadioDuration::from_micros(50),
                },
            ),
            &mut sink,
        )
        .unwrap();
    assert_eq!(
        radio.request(
            RadioRequest::Transmit {
                connection,
                pdu: DataPdu::new(DataPduKind::Start, &[1]).unwrap(),
            },
            &mut sink,
        ),
        Err(RequestError::Busy)
    );
    let RadioStep::Start(_) = radio.drive(view(false), &mut sink) else {
        panic!("the event starts")
    };
    // The event captured an anchor.
    execute_all(&radio, 1 << 11);
    radio.complete(&mut sink);
    assert!(matches!(sink.0.as_slice(), [Seen::Anchor(id, Some(_))] if *id == EventId::new(1)));

    sink.0.clear();
    let slot = &radio.connections[0].as_ref().unwrap().0;
    assert!(
        radio
            .memory
            .connections
            .emulate_receive_for_validation(&slot.instance, &[0x02, 1, 9])
    );
    radio
        .request(
            event(
                2,
                30_000,
                ConnectionEventTiming::Recurring {
                    receive_wait: RadioDuration::from_micros(500),
                },
            ),
            &mut sink,
        )
        .unwrap();
    let RadioStep::Start(_) = radio.drive(view(false), &mut sink) else {
        panic!("the event starts")
    };
    execute_all(&radio, 0);
    radio.complete(&mut sink);
    assert_eq!(
        sink.0,
        [
            Seen::Received(EventId::new(2), std::vec![0x02, 1, 9]),
            Seen::Ended(EventId::new(2), true)
        ]
    );
    assert_eq!(
        radio.request(RadioRequest::CloseConnection(connection), &mut sink),
        Ok(())
    );
}

#[test]
fn a_test_receiver_reports_before_its_end() {
    let mut radio = radio();
    let mut sink = Sink::default();
    radio
        .request(
            RadioRequest::TestReceive(TestReceive {
                id: EventId::new(4),
                channel: TestChannel::new(19).unwrap(),
                phy: TestPhy::Le1M,
                window: window(10_000, 1_000),
                recurring: false,
                tx_power: TxPower::from_dbm(0),
            }),
            &mut sink,
        )
        .unwrap();
    assert_eq!(
        radio.request(RadioRequest::EndTest, &mut sink),
        Err(RequestError::Busy)
    );
    let RadioStep::Start(_) = radio.drive(view(false), &mut sink) else {
        panic!("the test starts")
    };
    execute_all(&radio, 0);
    radio.complete(&mut sink);
    assert_eq!(
        sink.0,
        [
            Seen::Test(EventId::new(4), TestReport::Nothing),
            Seen::Ended(EventId::new(4), true)
        ]
    );
    radio.request(RadioRequest::EndTest, &mut sink).unwrap();
    assert_eq!(
        radio.request(RadioRequest::EndTest, &mut sink),
        Err(RequestError::Unknown)
    );
}

#[test]
fn configuration_errors_leave_the_radio_unchanged() {
    let mut radio = radio();
    let mut sink = Sink::default();
    configure_legacy(&mut radio, &mut sink);
    assert_eq!(
        radio.request(
            RadioRequest::ConfigureAdvertising(AdvertisingConfiguration {
                set: AdvertisingSetId::new(0),
                pdu: AdvertisingPdu::new(&NONCONN).unwrap(),
                scan_response: None,
                tx_power: TxPower::from_dbm(0),
            }),
            &mut sink,
        ),
        Err(RequestError::AlreadyConfigured)
    );
    // The only legacy instance is taken.
    assert_eq!(
        radio.request(
            RadioRequest::ConfigureAdvertising(AdvertisingConfiguration {
                set: AdvertisingSetId::new(1),
                pdu: AdvertisingPdu::new(&NONCONN).unwrap(),
                scan_response: None,
                tx_power: TxPower::from_dbm(0),
            }),
            &mut sink,
        ),
        Err(RequestError::NoInstance)
    );
    assert_eq!(
        radio.request(RadioRequest::RemoveScanner(ScannerId::new(0)), &mut sink),
        Err(RequestError::Unknown)
    );
    radio
        .request(
            RadioRequest::RemoveAdvertising(AdvertisingSetId::new(0)),
            &mut sink,
        )
        .unwrap();
    assert!(sink.0.is_empty());
    assert!(!sink.0.contains(&Seen::Fault));
}

#[test]
fn a_stopped_scheduler_yields_the_bluetooth_quiescence_proof() {
    let mut radio = radio();
    assert!(radio.quiescence().is_none());
    radio
        .enter_stopped(oer_esp32s31_hal::bluetooth::BluetoothSchedulerStopped::for_validation())
        .unwrap();
    let proof = radio.quiescence().expect("the stopped receipt is held");
    assert_eq!(
        proof.client(),
        oer_esp32s31_hal::shared_radio::RadioClient::Bluetooth
    );
    assert_eq!(
        proof.span(),
        oer_esp32s31_hal::shared_radio::QuiescentSpan::Stopped
    );
    drop(proof);
    radio
        .resume(&ControllerTimeSample::for_validation(0))
        .unwrap();
    assert!(radio.quiescence().is_none());
    assert!(
        radio
            .resume(&ControllerTimeSample::for_validation(0))
            .is_err()
    );
}

#[test]
fn resuming_cancels_passed_events_and_restarts_the_rest() {
    let mut radio = radio();
    let mut sink = Sink::default();
    configure_legacy(&mut radio, &mut sink);
    let single = AdvertisingChannels::single(AdvertisingChannel::Channel37);
    radio
        .request(advertise(1, 10_000, single), &mut sink)
        .unwrap();
    let RadioStep::Start(_) = radio.drive(view(false), &mut sink) else {
        panic!("the event starts")
    };
    radio
        .enter_stopped(oer_esp32s31_hal::bluetooth::BluetoothSchedulerStopped::for_validation())
        .unwrap();
    // Stopped: nothing restarts.
    assert!(matches!(
        radio.drive(view(false), &mut sink),
        RadioStep::Idle
    ));
    // 30 ms later, at two raw ticks per microsecond.
    radio
        .resume(&ControllerTimeSample::for_validation(60_000))
        .unwrap();
    let RadioStep::Transaction(step) = radio.drive(view(false), &mut sink) else {
        panic!("the passed event is cancelled")
    };
    assert_eq!(
        step.actions().collect::<Vec<_>>(),
        [SchedulerAction::PublishHead(None)]
    );
    assert_eq!(sink.0.last(), Some(&Seen::Ended(EventId::new(1), false)));

    radio
        .request(advertise(2, 100_000, single), &mut sink)
        .unwrap();
    let RadioStep::Start(_) = radio.drive(view(false), &mut sink) else {
        panic!("the event starts")
    };
    radio
        .enter_stopped(oer_esp32s31_hal::bluetooth::BluetoothSchedulerStopped::for_validation())
        .unwrap();
    radio
        .resume(&ControllerTimeSample::for_validation(60_000))
        .unwrap();
    // The event still lies ahead: the idle scheduler restarts at it.
    let RadioStep::Start(_) = radio.drive(view(false), &mut sink) else {
        panic!("the listed event restarts")
    };
}
