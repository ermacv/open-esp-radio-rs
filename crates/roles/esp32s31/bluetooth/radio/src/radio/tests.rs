use std::vec::Vec;

use oer_esp32s31_bluetooth_memory::BlePhyLe1MPacketStartCalibration;

use oer_bluetooth_radio::{
    AcceptListChange, AcceptListDevice, AccessAddress, AdvertisingChannel, AdvertisingChannels,
    AdvertisingConfiguration, AdvertisingEvent, AdvertisingPdu, AdvertisingReception,
    AdvertisingSetId, CoexistenceLevel, ConnectionConfiguration, ConnectionEvent,
    ConnectionEventTiming, ConnectionId, CrcInit, DataChannel, DataPdu, DataPduKind, EventId,
    EventResult, LePhy, RadioDuration, RadioInstant, RadioOutcome, RadioRequest, RadioWindow,
    RequestError, ScanFilterPolicy, ScanType, ScanWindow, ScannerConfiguration, ScannerId,
    TestChannel, TestPhy, TestReceive, TestReport, TxPower,
};
use oer_esp32s31_bluetooth::{
    ControllerTimeSample,
    scheduler::{
        SchedulerAction, SchedulerHardwareView, SchedulerNext, SchedulerObservation,
        SchedulerSoftwareConfig,
    },
};
use oer_esp32s31_hal::bluetooth::{
    BluetoothControllerHalInitConfig, BluetoothSchedulerBusyObservation,
    BluetoothSchedulerExecutionModifyDisposition,
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
            RadioOutcome::Fault(_) | RadioOutcome::Poisoned(_) => Seen::Fault,
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
        busy: BluetoothSchedulerBusyObservation::from_busy_for_validation(busy),
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
                reception: AdvertisingReception::None,
                tx_power: TxPower::from_dbm(0),
                phy: LePhy::Le1M,
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
        coexistence: CoexistenceLevel::Baseline,
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
    assert_eq!(radio.timing().preparation_lead.as_micros(), 137);
    assert_eq!(radio.timing().admission_guard.as_micros(), 40);
    assert_eq!(radio.now(), RadioInstant::from_micros(0));
    // A recurring event ends 1,074 us after its widened anchor less the lead.
    let connection = radio.timing().connection;
    assert_eq!(connection.local_sleep_clock_ppm, 500);
    assert_eq!(connection.event_length.as_micros(), 1_074 - 137);
    assert_eq!(connection.first_event_length.as_micros(), 1_075);
    // The widening adds the Controller's default `ble_ll_jitter_usecs`; the
    // receive guard is the private options' 10 us.
    assert_eq!(connection.widening_jitter.as_micros(), 16);
    assert_eq!(connection.receive_guard.as_micros(), 10);
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
                scan_type: ScanType::Passive,
                filter_policy: ScanFilterPolicy::AcceptAll,
                scanner: ScannerId::new(0),
                tx_power: TxPower::from_dbm(0),
                phy: LePhy::Le1M,
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
fn a_directed_set_receives_without_a_scan_response() {
    // ADV_DIRECT_IND from a public advertiser to a random target.
    const ADV_DIRECT_IND: [u8; 14] = [0xa1, 12, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 0xcc];
    let mut radio = radio();
    let mut sink = Sink::default();
    radio
        .request(
            RadioRequest::ConfigureAdvertising(AdvertisingConfiguration {
                set: AdvertisingSetId::new(4),
                pdu: AdvertisingPdu::new(&ADV_DIRECT_IND).unwrap(),
                reception: AdvertisingReception::Report,
                tx_power: TxPower::from_dbm(0),
                phy: LePhy::Le1M,
            }),
            &mut sink,
        )
        .unwrap();
    let slot = &radio.connectable[0]
        .as_ref()
        .expect("a receiving set")
        .instance;
    assert_eq!(
        radio.memory.connectable.adv_ind_pdu(slot),
        Some(&ADV_DIRECT_IND[..])
    );
    assert_eq!(radio.memory.connectable.scan_response_pdu(slot), None);
    assert!(radio.legacy.iter().all(Option::is_none));
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
                reception: AdvertisingReception::ScanResponse(
                    AdvertisingPdu::new(&SCAN_RSP).unwrap(),
                ),
                tx_power: TxPower::from_dbm(0),
                phy: LePhy::Le1M,
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
            coexistence: CoexistenceLevel::Baseline,
        })
    };
    // Every primary channel gets its own item.
    {
        let mut all = self::radio();
        all.request(
            RadioRequest::ConfigureAdvertising(AdvertisingConfiguration {
                set: AdvertisingSetId::new(3),
                pdu: AdvertisingPdu::new(&ADV_IND).unwrap(),
                reception: AdvertisingReception::ScanResponse(
                    AdvertisingPdu::new(&SCAN_RSP).unwrap(),
                ),
                tx_power: TxPower::from_dbm(0),
                phy: LePhy::Le1M,
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
                phy: LePhy::Le1M,
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
            interval: RadioDuration::from_micros(30_000),
            timing,
            priority: 13,
            coexistence: CoexistenceLevel::Baseline,
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
                    widening: RadioDuration::from_micros(40),
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
fn a_connection_event_may_run_past_its_reservation_within_the_interval() {
    // Up to 7,499 us the event may take the whole interval.
    assert_eq!(super::connection_event_span(7_499, 40), Ok(7_499));
    assert_eq!(
        super::connection_event_span(7_500, 40),
        Ok(7_500 - 2_000 + 40)
    );
    assert_eq!(
        super::connection_event_span(30_000, 16),
        Ok(30_000 - 2_000 + 16)
    );
    // The span never depends on the reserved air window.
    assert!(super::connection_event_span(30_000, 16).unwrap() > 1_074);
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
    let RadioStep::EnterTest(_) = radio.drive(view(false), &mut sink) else {
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
fn cancelling_a_running_test_stops_the_scheduler_instead_of_skipping_it() {
    let mut radio = radio();
    let mut sink = Sink::default();
    radio
        .request(
            RadioRequest::TestReceive(TestReceive {
                id: EventId::new(4),
                channel: TestChannel::new(19).unwrap(),
                phy: TestPhy::Le1M,
                window: window(10_000, 1_000),
                recurring: true,
                tx_power: TxPower::from_dbm(0),
            }),
            &mut sink,
        )
        .unwrap();
    let RadioStep::EnterTest(_) = radio.drive(view(false), &mut sink) else {
        panic!("the test starts")
    };
    radio
        .request(RadioRequest::Cancel(EventId::new(4)), &mut sink)
        .unwrap();
    // While hardware runs the test, no cancellation hold starts: the
    // scheduler stops first, however often the radio is driven.
    for _ in 0..2 {
        assert!(matches!(
            radio.drive(view(true), &mut sink),
            RadioStep::StopScheduler
        ));
    }
    assert!(sink.0.is_empty());
    radio
        .enter_stopped(oer_esp32s31_hal::bluetooth::BluetoothSchedulerStopped::for_validation())
        .unwrap();
    radio
        .resume(&ControllerTimeSample::for_validation(0))
        .unwrap();
    // The stopped scheduler lets the event leave its list at once.
    let RadioStep::Transaction(step) = radio.drive(view(false), &mut sink) else {
        panic!("the cancelled test leaves its list")
    };
    assert_eq!(
        step.actions().collect::<Vec<_>>(),
        [SchedulerAction::PublishHead(None)]
    );
    assert_eq!(sink.0, [Seen::Ended(EventId::new(4), false)]);
    radio.request(RadioRequest::EndTest, &mut sink).unwrap();
}

#[test]
fn a_test_disables_the_phy_route_and_its_end_restores_it() {
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
    let RadioStep::EnterTest(_) = radio.drive(view(false), &mut sink) else {
        panic!("a test event starts with the route disabled")
    };
    execute_all(&radio, 0);
    radio.complete(&mut sink);
    // The route stays disabled while the test holds its instance.
    assert!(matches!(
        radio.drive(view(false), &mut sink),
        RadioStep::Idle
    ));
    radio.request(RadioRequest::EndTest, &mut sink).unwrap();
    assert!(matches!(
        radio.drive(view(false), &mut sink),
        RadioStep::LeaveTest
    ));
    assert!(matches!(
        radio.drive(view(false), &mut sink),
        RadioStep::Idle
    ));
}

#[test]
fn a_test_event_stops_a_busy_scheduler_and_starts_as_the_list_head() {
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
    // A busy scheduler takes no live insertion of a test event.
    assert!(matches!(
        radio.drive(view(true), &mut sink),
        RadioStep::StopScheduler
    ));
    radio
        .enter_stopped(oer_esp32s31_hal::bluetooth::BluetoothSchedulerStopped::for_validation())
        .unwrap();
    radio
        .resume(&ControllerTimeSample::for_validation(0))
        .unwrap();
    let RadioStep::EnterTest(_) = radio.drive(view(false), &mut sink) else {
        panic!("the stopped scheduler starts at the test event")
    };
    assert!(sink.0.is_empty());
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
                reception: AdvertisingReception::None,
                tx_power: TxPower::from_dbm(0),
                phy: LePhy::Le1M,
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
                reception: AdvertisingReception::None,
                tx_power: TxPower::from_dbm(0),
                phy: LePhy::Le1M,
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
fn a_power_below_the_provider_table_is_refused_before_any_instance() {
    let mut radio = radio();
    let mut sink = Sink::default();
    assert_eq!(
        radio.request(
            RadioRequest::ConfigureScanner(ScannerConfiguration {
                scan_type: ScanType::Passive,
                filter_policy: ScanFilterPolicy::AcceptAll,
                scanner: ScannerId::new(0),
                tx_power: TxPower::from_dbm(-25),
                phy: LePhy::Le1M,
            }),
            &mut sink,
        ),
        Err(RequestError::Unsupported)
    );
    // The refusal took no scanner instance.
    radio
        .request(
            RadioRequest::ConfigureScanner(ScannerConfiguration {
                scan_type: ScanType::Passive,
                filter_policy: ScanFilterPolicy::AcceptAll,
                scanner: ScannerId::new(0),
                tx_power: TxPower::from_dbm(-24),
                phy: LePhy::Le1M,
            }),
            &mut sink,
        )
        .unwrap();
    assert!(sink.0.is_empty());
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

#[test]
fn a_reset_controller_returns_pools_free_for_the_next_epoch() {
    let mut radio = radio();
    let mut sink = Sink::default();
    configure_legacy(&mut radio, &mut sink);
    radio
        .request(advertise(1, 10_000, AdvertisingChannels::ALL), &mut sink)
        .unwrap();
    let RadioStep::Start(_) = radio.drive(view(false), &mut sink) else {
        panic!("an idle scheduler starts at once")
    };
    // The epoch ends with the event still listed in hardware.
    let memory =
        radio.into_memory(&oer_esp32s31_hal::bluetooth::BluetoothControllerReset::for_validation());

    let mut next = Radio::new(
        memory,
        SchedulerSoftwareConfig::reviewed_standalone(),
        BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale(),
        &ControllerTimeSample::for_validation(0),
        500,
    );
    configure_legacy(&mut next, &mut sink);
    next.request(advertise(2, 10_000, AdvertisingChannels::ALL), &mut sink)
        .unwrap();
    let RadioStep::Start(_) = next.drive(view(false), &mut sink) else {
        panic!("the next epoch starts from free pools")
    };
    assert_eq!(next.executor.list().len(), 3);
}

fn test_receive(id: u32, recurring: bool) -> RadioRequest<'static> {
    RadioRequest::TestReceive(TestReceive {
        id: EventId::new(id),
        channel: TestChannel::new(19).unwrap(),
        phy: TestPhy::Le1M,
        window: window(10_000, 1_000),
        recurring,
        tx_power: TxPower::from_dbm(0),
    })
}

#[test]
fn a_test_session_owns_the_link_layer_until_test_end() {
    let mut radio = radio();
    let mut sink = Sink::default();
    configure_legacy(&mut radio, &mut sink);
    radio.request(test_receive(4, false), &mut sink).unwrap();
    assert_eq!(
        radio.request(advertise(1, 10_000, AdvertisingChannels::ALL), &mut sink),
        Err(RequestError::Busy)
    );
    let RadioStep::EnterTest(_) = radio.drive(view(false), &mut sink) else {
        panic!("the test starts")
    };
    execute_all(&radio, 0);
    radio.complete(&mut sink);
    radio.request(RadioRequest::EndTest, &mut sink).unwrap();
    // Until the route is restored the session still owns the Link Layer.
    assert_eq!(
        radio.request(advertise(1, 10_000, AdvertisingChannels::ALL), &mut sink),
        Err(RequestError::Busy)
    );
    assert!(matches!(
        radio.drive(view(false), &mut sink),
        RadioStep::LeaveTest
    ));
    radio
        .request(advertise(1, 10_000, AdvertisingChannels::ALL), &mut sink)
        .unwrap();
}

#[test]
fn only_the_first_event_of_a_session_disables_the_route() {
    let mut radio = radio();
    let mut sink = Sink::default();
    radio.request(test_receive(4, false), &mut sink).unwrap();
    let RadioStep::EnterTest(_) = radio.drive(view(false), &mut sink) else {
        panic!("the first test event disables the route")
    };
    execute_all(&radio, 0);
    radio.complete(&mut sink);
    radio.request(test_receive(5, true), &mut sink).unwrap();
    let RadioStep::Start(_) = radio.drive(view(false), &mut sink) else {
        panic!("a recurring test event keeps the disabled route")
    };
    execute_all(&radio, 0);
    radio.complete(&mut sink);
    radio.request(RadioRequest::EndTest, &mut sink).unwrap();
    assert!(matches!(
        radio.drive(view(false), &mut sink),
        RadioStep::LeaveTest
    ));
}

#[test]
fn a_session_without_a_published_event_has_no_route_to_restore() {
    let mut radio = radio();
    let mut sink = Sink::default();
    radio.request(test_receive(4, false), &mut sink).unwrap();
    radio
        .request(RadioRequest::Cancel(EventId::new(4)), &mut sink)
        .unwrap();
    radio.request(RadioRequest::EndTest, &mut sink).unwrap();
    assert!(matches!(
        radio.drive(view(false), &mut sink),
        RadioStep::Idle
    ));
}

#[test]
fn list_changes_edit_the_device_table_up_to_its_capacity() {
    let mut radio = radio();
    let mut sink = Sink::default();
    let device = |index: u8| AcceptListDevice {
        random: index.is_multiple_of(2),
        address: [index, 1, 2, 3, 4, 5],
    };
    let mut change = |radio: &mut Radio, change| {
        radio.request(RadioRequest::FilterAcceptList(change), &mut sink)
    };
    for index in 0..12 {
        change(&mut radio, AcceptListChange::Add(device(index))).unwrap();
    }
    // Adding a listed device again keeps one entry.
    change(&mut radio, AcceptListChange::Add(device(0))).unwrap();
    assert_eq!(
        change(&mut radio, AcceptListChange::Add(device(12))),
        Err(RequestError::ListFull)
    );
    change(&mut radio, AcceptListChange::Remove(device(3))).unwrap();
    assert_eq!(
        change(&mut radio, AcceptListChange::Remove(device(3))),
        Err(RequestError::NotListed)
    );
    assert_eq!(radio.device_table_publication().count.get(), 11);
    change(&mut radio, AcceptListChange::Add(device(12))).unwrap();
    change(&mut radio, AcceptListChange::Clear).unwrap();
    assert_eq!(radio.device_table_publication().count.get(), 0);
    assert!(sink.0.is_empty());
}
