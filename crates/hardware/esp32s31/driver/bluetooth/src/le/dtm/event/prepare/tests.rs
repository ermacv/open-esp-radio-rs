use crate::{
    ControllerSchedulerEpoch, ControllerTimeSample, DtmLinkStateReset, DtmRxRecurringEventWindow,
    DtmSchedulerReservation, SchedulerInstant,
    le::dtm::{
        DtmChannel, DtmDefaultTxPowerDbm, DtmPayloadLength, DtmPayloadPattern, DtmPhy, DtmRole,
        DtmSchedulerItemEvent, DtmTxGraphPrepare, DtmTxTimingMicros,
    },
    scheduler::{
        SchedulerSequenceAuthorizationError, SchedulerSequenceReady, SchedulerTimingPolicy,
        timeline::SchedulerTimeline,
    },
};

use oer_esp32s31_bluetooth_memory::{
    DtmMemoryGraphModelAddress, DtmMemoryGraphStorage, DtmRxResultProjection,
    DtmSchedulerAllocationConfig, DtmSchedulerItemCompletionStatus,
};

use oer_esp32s31_pac::BluetoothControllerHalInitConfig;

use super::{
    DtmActiveReceiverCpuOwned, DtmActiveTransmitterCpuOwned, DtmEventContext,
    DtmReceiverCommandFacts, DtmReceiverCpuOwned, DtmReceiverEvent, DtmReceiverEventContext,
    DtmRecycledEvent, DtmReviewedEventWordsPlan, DtmReviewedEventWordsPlanError,
    DtmRxCommittedWindow, DtmTestEndReport, DtmTransmitterCommandFacts, DtmTransmitterEvent,
    DtmTransmitterEventContext,
};

fn allocation_config() -> DtmSchedulerAllocationConfig {
    DtmSchedulerAllocationConfig::new(2, 3, 4)
}

fn owner(base: u32) -> crate::memory::DtmMemoryGraphCpuOwned {
    let storage = std::boxed::Box::leak(std::boxed::Box::new(DtmMemoryGraphStorage::new()));
    let base = DtmMemoryGraphModelAddress::new(base)
        .expect("test base has valid compressed-pointer syntax");
    DtmMemoryGraphStorage::pin_static_model(storage, base, allocation_config())
        .expect("test graph fits physical controller SRAM")
}

fn link_state(role: DtmRole) -> DtmLinkStateReset {
    DtmLinkStateReset::new(DtmDefaultTxPowerDbm::new(0), role)
}

fn epoch() -> ControllerSchedulerEpoch {
    ControllerSchedulerEpoch::new(
        ControllerTimeSample::for_validation(100),
        1_000,
        BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale(),
    )
}

fn channel() -> DtmChannel {
    DtmChannel::new(5).expect("channel five is valid")
}

fn margin() -> u32 {
    crate::scheduler::SchedulerSoftwareConfig::reviewed_standalone().preparation_lead_micros()
}

fn tx_timing() -> crate::le::dtm::DtmTxSchedulerTiming {
    DtmTxTimingMicros::new(DtmPayloadLength::from_hci_image(3), DtmPhy::Le2M, 0).scheduler_timing()
}

fn tx_window() -> crate::DtmTxEventWindow {
    tx_timing().initial_event_window(
        crate::scheduler::SchedulerSoftwareConfig::reviewed_standalone(),
        SchedulerInstant::from_image(64),
        SchedulerInstant::from_image(1_119),
    )
}

fn rx_initial_window() -> crate::DtmRxInitialEventWindow {
    crate::DtmRxInitialEventWindow::new(
        crate::scheduler::SchedulerSoftwareConfig::reviewed_standalone(),
        SchedulerInstant::from_image(64),
        SchedulerInstant::from_image(1_119),
    )
}

fn item(role: DtmRole) -> DtmSchedulerItemEvent {
    match role {
        DtmRole::Transmitter => {
            DtmSchedulerItemEvent::new_transmitter(channel(), DtmPhy::Le2M, tx_window())
        }
        DtmRole::Receiver => DtmSchedulerItemEvent::new_initial_receiver(
            channel(),
            DtmPhy::LeCoded,
            rx_initial_window(),
        ),
    }
    .expect("selected PHY is valid for its role")
}

fn timing_policy() -> SchedulerTimingPolicy {
    SchedulerTimingPolicy::from_scheduler_config(
        crate::scheduler::SchedulerSoftwareConfig::reviewed_standalone(),
        BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale(),
    )
}

fn admission_sample() -> ControllerTimeSample {
    ControllerTimeSample::for_validation(92)
}

fn reservation<const CAPACITY: usize>(
    timeline: &mut SchedulerTimeline<CAPACITY>,
    role: DtmRole,
) -> DtmSchedulerReservation<SchedulerSequenceReady> {
    initial_reservation_for_event(timeline, item(role))
}

fn initial_reservation_for_event<const CAPACITY: usize>(
    timeline: &mut SchedulerTimeline<CAPACITY>,
    event: DtmSchedulerItemEvent,
) -> DtmSchedulerReservation<SchedulerSequenceReady> {
    let epoch = epoch();
    let window = timeline
        .reserve_initial_window(
            event.raw_start(epoch),
            event.raw_end(epoch),
            timing_policy(),
            admission_sample(),
        )
        .expect("the first guarded deadline is open");
    DtmSchedulerReservation::new(window, event, epoch)
        .authorize_sequence(admission_sample())
        .expect("the initial sequence deadline is open")
}

fn recurring_reservation_for_event<const CAPACITY: usize>(
    timeline: &mut SchedulerTimeline<CAPACITY>,
    event: DtmSchedulerItemEvent,
) -> DtmSchedulerReservation<SchedulerSequenceReady> {
    let epoch = epoch();
    let window = timeline
        .reserve_recurring_window(
            event.raw_start(epoch),
            event.raw_end(epoch),
            timing_policy(),
        )
        .expect("the exact recurring window is collision-free");
    DtmSchedulerReservation::new(window, event, epoch)
        .authorize_sequence(admission_sample())
        .expect("the sole recurring sequence deadline is open")
}

#[test]
fn tx_plan_requires_and_retains_the_prepared_packet_identity() {
    let mut timeline = SchedulerTimeline::<1>::new();
    let reset = DtmLinkStateReset::new(DtmDefaultTxPowerDbm::new(20), DtmRole::Transmitter);
    let plan = DtmReviewedEventWordsPlan::new_transmitter(
        reset,
        reservation(&mut timeline, DtmRole::Transmitter),
    )
    .expect("both transforms encode TX");

    let packet = owner(0x2f07_0000).prepare_dtm_tx_packet(
        DtmPayloadPattern::Repeated11110000,
        DtmPayloadLength::from_hci_image(3),
    );

    let prepared = plan
        .prepare_first(
            packet,
            channel(),
            DtmPhy::Le2M,
            tx_timing(),
            margin(),
            tx_window(),
        )
        .expect("the consumed graph supplies both private link projections");
    let scheduler_prepared = prepared.prepare_scheduler_bookkeeping();
    assert_eq!(
        scheduler_prepared.packet_pattern(),
        DtmPayloadPattern::Repeated11110000
    );
    assert_eq!(scheduler_prepared.packet_length().hci_image(), 3);
    let prepared = scheduler_prepared
        .prepare_empty_list_link()
        .cancel()
        .cancel();
    let (_owner, reservation) = prepared.cancel_first();
    assert!(timeline.release(reservation.into_window()).is_ok());
    assert!(timeline.is_empty());
}

#[test]
fn receiver_plan_cancellation_preserves_the_session_owner() {
    let mut timeline = SchedulerTimeline::<1>::new();
    let reset = link_state(DtmRole::Receiver);
    let plan = DtmReviewedEventWordsPlan::new_receiver(
        reset,
        reservation(&mut timeline, DtmRole::Receiver),
    )
    .expect("both transforms encode RX");

    let prepared = plan
        .prepare_first(
            DtmReceiverCpuOwned::new(owner(0x2f00_0100)),
            channel(),
            DtmPhy::LeCoded,
            margin(),
            rx_initial_window(),
        )
        .expect("the bound graph accepts the receiver plan");
    let scheduler_prepared = prepared.prepare_scheduler_bookkeeping();
    let (owner, reservation) = scheduler_prepared
        .prepare_empty_list_link()
        .cancel()
        .cancel()
        .cancel_first();

    assert_eq!(owner.received_packet_count(), 0);
    assert!(timeline.release(reservation.into_window()).is_ok());
    assert!(timeline.is_empty());
}

#[test]
fn recurring_tx_sequence_ready_reservation_enters_plan_and_cancels_losslessly() {
    let mut timeline = SchedulerTimeline::<1>::new();
    let reset = link_state(DtmRole::Transmitter);
    let pattern = DtmPayloadPattern::Repeated11110000;
    let length = DtmPayloadLength::from_hci_image(3);
    let memory = owner(0x2f06_0000)
        .prepare_dtm_tx_packet(pattern, length)
        .discard();
    let committed_window = tx_window();
    let candidate_window = tx_timing()
        .advance_event_window(
            crate::scheduler::SchedulerSoftwareConfig::reviewed_standalone(),
            committed_window,
            SchedulerInstant::from_image(1_100),
        )
        .window();
    let facts = DtmTransmitterCommandFacts {
        link_state: reset,
        channel: channel(),
        phy: DtmPhy::Le2M,
        timing: tx_timing(),
        margin: margin(),
        pattern,
        length,
    };
    let active = DtmActiveTransmitterCpuOwned {
        memory,
        facts,
        last_committed_window: committed_window,
        status: DtmSchedulerItemCompletionStatus::Zero,
    };
    let event = DtmSchedulerItemEvent::new_transmitter(channel(), DtmPhy::Le2M, candidate_window)
        .expect("TX event accepts LE 2M");
    let plan = DtmReviewedEventWordsPlan::new_transmitter(
        reset,
        recurring_reservation_for_event(&mut timeline, event),
    )
    .expect("both transforms encode TX");

    let prepared = plan
        .prepare_recurring(active, candidate_window)
        .expect("active TX graph accepts recurring preparation");
    let (active, reservation) = prepared
        .prepare_scheduler_bookkeeping()
        .prepare_empty_list_link()
        .cancel()
        .cancel()
        .cancel_recurring();

    assert_eq!(active.link_state(), reset);
    assert_eq!(active.channel(), channel());
    assert_eq!(active.phy(), DtmPhy::Le2M);
    assert_eq!(active.timing(), tx_timing());
    assert_eq!(active.margin(), margin());
    assert_eq!(active.last_committed_window, committed_window);
    assert_eq!(active.status(), DtmSchedulerItemCompletionStatus::Zero);
    assert!(timeline.release(reservation.into_window()).is_ok());
}

#[test]
fn recurring_rx_cancellation_restores_session_and_committed_window() {
    let mut timeline = SchedulerTimeline::<1>::new();
    let reset = link_state(DtmRole::Receiver);
    let committed_window = DtmRxCommittedWindow::Initial(rx_initial_window());
    let candidate_window = DtmRxRecurringEventWindow::new(
        crate::scheduler::SchedulerSoftwareConfig::reviewed_standalone(),
        SchedulerInstant::from_image(1_100),
        SchedulerInstant::from_image(1_120),
    );
    let facts = DtmReceiverCommandFacts {
        link_state: reset,
        channel: channel(),
        phy: DtmPhy::LeCoded,
        margin: margin(),
    };
    let active = DtmActiveReceiverCpuOwned {
        memory: owner(0x2f05_0000),
        facts,
        session: crate::le::dtm::rx::DtmReceiverSession::new(),
        last_committed_window: committed_window,
    };
    let event =
        DtmSchedulerItemEvent::new_recurring_receiver(channel(), DtmPhy::LeCoded, candidate_window)
            .expect("RX event accepts LE Coded");
    let plan = DtmReviewedEventWordsPlan::new_receiver(
        reset,
        recurring_reservation_for_event(&mut timeline, event),
    )
    .expect("both transforms encode RX");

    let prepared = plan
        .prepare_recurring(active, candidate_window)
        .expect("active RX graph accepts recurring preparation");
    let (active, reservation) = prepared
        .prepare_scheduler_bookkeeping()
        .prepare_empty_list_link()
        .cancel()
        .cancel()
        .cancel_recurring();

    assert_eq!(active.link_state(), reset);
    assert_eq!(active.channel(), channel());
    assert_eq!(active.phy(), DtmPhy::LeCoded);
    assert_eq!(active.margin(), margin());
    assert_eq!(active.last_committed_window, committed_window);
    assert_eq!(active.received_packet_count(), 0);
    assert!(timeline.release(reservation.into_window()).is_ok());
}

#[test]
fn completed_events_commit_only_the_candidate_window_into_active_owners() {
    let reset_tx = link_state(DtmRole::Transmitter);
    let tx_candidate = tx_timing()
        .advance_event_window(
            crate::scheduler::SchedulerSoftwareConfig::reviewed_standalone(),
            tx_window(),
            SchedulerInstant::from_image(1_100),
        )
        .window();
    let tx_facts = DtmTransmitterCommandFacts {
        link_state: reset_tx,
        channel: channel(),
        phy: DtmPhy::Le2M,
        timing: tx_timing(),
        margin: margin(),
        pattern: DtmPayloadPattern::Repeated11110000,
        length: DtmPayloadLength::from_hci_image(3),
    };
    let recycled_tx = DtmRecycledEvent::<DtmTransmitterEvent> {
        memory: owner(0x2f04_0000),
        context: DtmEventContext::Transmitter(DtmTransmitterEventContext {
            facts: tx_facts,
            event_window: tx_candidate,
        }),
        status: DtmSchedulerItemCompletionStatus::Zero,
        _role: core::marker::PhantomData,
    };
    assert_eq!(recycled_tx.packet_pattern(), tx_facts.pattern);
    assert_eq!(recycled_tx.packet_length(), tx_facts.length);
    let tx = recycled_tx.into_next();
    assert_eq!(tx.packet_pattern(), tx_facts.pattern);
    assert_eq!(tx.packet_length(), tx_facts.length);
    assert_eq!(tx.last_committed_window(), tx_candidate);
    assert_eq!(tx.status(), DtmSchedulerItemCompletionStatus::Zero);

    let reset_rx = link_state(DtmRole::Receiver);
    let rx_candidate = DtmRxRecurringEventWindow::new(
        crate::scheduler::SchedulerSoftwareConfig::reviewed_standalone(),
        SchedulerInstant::from_image(1_100),
        SchedulerInstant::from_image(1_120),
    );
    let recycled_rx = DtmRecycledEvent::<DtmReceiverEvent> {
        memory: owner(0x2f03_0000),
        context: DtmEventContext::Receiver(DtmReceiverEventContext {
            facts: DtmReceiverCommandFacts {
                link_state: reset_rx,
                channel: channel(),
                phy: DtmPhy::LeCoded,
                margin: margin(),
            },
            session: crate::le::dtm::rx::DtmReceiverSession::new(),
            event_window: DtmRxCommittedWindow::Recurring(rx_candidate),
        }),
        status: DtmSchedulerItemCompletionStatus::Zero,
        _role: core::marker::PhantomData,
    };
    assert_eq!(recycled_rx.role(), DtmRole::Receiver);
    assert_eq!(recycled_rx.status(), DtmSchedulerItemCompletionStatus::Zero);
    let rx = recycled_rx.into_next();
    assert_eq!(
        rx.last_committed_window,
        DtmRxCommittedWindow::Recurring(rx_candidate)
    );
    assert_eq!(rx.received_packet_count(), 0);
}

#[test]
fn active_roles_hold_the_reclaimed_graph_through_test_end_handoff() {
    let reset_tx = link_state(DtmRole::Transmitter);
    let tx = DtmActiveTransmitterCpuOwned {
        memory: owner(0x2f02_0000),
        facts: DtmTransmitterCommandFacts {
            link_state: reset_tx,
            channel: channel(),
            phy: DtmPhy::Le2M,
            timing: tx_timing(),
            margin: margin(),
            pattern: DtmPayloadPattern::Repeated11110000,
            length: DtmPayloadLength::from_hci_image(3),
        },
        last_committed_window: tx_window(),
        status: DtmSchedulerItemCompletionStatus::Zero,
    };
    let ended = tx.into_test_ended();
    let stopping = crate::DtmSessionStopping::new(ended);
    assert_eq!(stopping.report(), DtmTestEndReport::Transmitter);
    assert_eq!(stopping.report().reported_packet_count(), 0);
    let _next_graph = stopping.response_published().begin_epoch().into_graph();

    let mut session = crate::le::dtm::rx::DtmReceiverSession::new();
    assert!(matches!(
        session.account_projection(DtmRxResultProjection::from_word(0)),
        crate::le::dtm::DtmRxCompletionOutcome::Counted {
            received_packet_count: 1,
            ..
        }
    ));
    let reset_rx = link_state(DtmRole::Receiver);
    let rx = DtmActiveReceiverCpuOwned {
        memory: owner(0x2f01_0000),
        facts: DtmReceiverCommandFacts {
            link_state: reset_rx,
            channel: channel(),
            phy: DtmPhy::LeCoded,
            margin: margin(),
        },
        session,
        last_committed_window: DtmRxCommittedWindow::Initial(rx_initial_window()),
    };
    let ended = rx.into_test_ended();
    let stopping = crate::DtmSessionStopping::new(ended);
    assert_eq!(
        stopping.report(),
        DtmTestEndReport::Receiver {
            received_packets: 1
        }
    );
    assert_eq!(stopping.report().reported_packet_count(), 1);
    let _next_graph = stopping.response_published().begin_epoch().into_graph();
}

#[test]
fn plan_rejects_mixed_roles_before_it_can_consume_memory() {
    let mut timeline = SchedulerTimeline::<1>::new();
    let reset = link_state(DtmRole::Transmitter);
    let failure = match DtmReviewedEventWordsPlan::new_transmitter(
        reset,
        reservation(&mut timeline, DtmRole::Receiver),
    ) {
        Ok(_) => panic!("a receiver reservation cannot form a transmitter plan"),
        Err(failure) => failure,
    };
    assert_eq!(
        failure.error(),
        DtmReviewedEventWordsPlanError::RoleMismatch {
            expected: DtmRole::Transmitter,
            link_state: DtmRole::Transmitter,
            scheduler_item: DtmRole::Receiver,
        }
    );
    assert!(
        timeline
            .release(failure.into_reservation().into_window())
            .is_ok()
    );
}

#[test]
fn sequence_authorization_rejects_the_second_guarded_deadline() {
    let mut timeline = SchedulerTimeline::<1>::new();
    let event = item(DtmRole::Receiver);
    let epoch = epoch();
    let window = timeline
        .reserve_initial_window(
            event.raw_start(epoch),
            event.raw_end(epoch),
            timing_policy(),
            admission_sample(),
        )
        .expect("the first guarded deadline is open");
    let reservation = DtmSchedulerReservation::new(window, event, epoch);
    let failure = reservation
        .authorize_sequence(ControllerTimeSample::for_validation(93))
        .expect_err("the second sample reaches the guarded start");
    assert_eq!(
        failure.error(),
        SchedulerSequenceAuthorizationError::DeadlineExpired
    );
    assert!(
        timeline
            .release(failure.into_reservation().into_window())
            .is_ok()
    );
}
