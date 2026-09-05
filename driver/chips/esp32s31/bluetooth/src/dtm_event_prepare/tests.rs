use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothDtmMemoryGraphModelAddress, BluetoothDtmMemoryGraphStorage,
    BluetoothDtmRxResultProjection, BluetoothDtmSchedulerAllocationConfig,
};
use open_esp_radio_esp32s31_pac::BluetoothControllerHalInitConfig;

use super::{
    BluetoothDtmActiveReceiverCpuOwned, BluetoothDtmActiveTransmitterCpuOwned,
    BluetoothDtmEventContext, BluetoothDtmReceiverCommandFacts, BluetoothDtmReceiverCpuOwned,
    BluetoothDtmReceiverEvent, BluetoothDtmReceiverEventContext, BluetoothDtmRecycledEvent,
    BluetoothDtmReviewedEventWordsPlan, BluetoothDtmReviewedEventWordsPlanError,
    BluetoothDtmRxCommittedWindow, BluetoothDtmTestEndReport, BluetoothDtmTransmitterCommandFacts,
    BluetoothDtmTransmitterEvent, BluetoothDtmTransmitterEventContext,
};
use crate::scheduler::timeline::BluetoothSchedulerTimeline;
use crate::{
    BluetoothControllerSchedulerEpoch, BluetoothControllerTimeSample, BluetoothDtmChannel,
    BluetoothDtmDefaultTxPowerDbm, BluetoothDtmLinkStateReset, BluetoothDtmPayloadLength,
    BluetoothDtmPayloadPattern, BluetoothDtmPhy, BluetoothDtmRole,
    BluetoothDtmRxRecurringEventWindow, BluetoothDtmSchedulerItemEvent,
    BluetoothDtmSchedulerReservation, BluetoothDtmTxGraphPrepare, BluetoothDtmTxTimingMicros,
    BluetoothSchedulerInstant, BluetoothSchedulerSequenceAuthorizationError,
    BluetoothSchedulerSequenceReady, BluetoothSchedulerTimingPolicy,
};
use open_esp_radio_esp32s31_bluetooth_memory::BluetoothDtmSchedulerItemCompletionStatus;

fn allocation_config() -> BluetoothDtmSchedulerAllocationConfig {
    BluetoothDtmSchedulerAllocationConfig::new(2, 3, 4)
}

fn owner(base: u32) -> crate::BluetoothDtmMemoryGraphCpuOwned {
    let storage =
        std::boxed::Box::leak(std::boxed::Box::new(BluetoothDtmMemoryGraphStorage::new()));
    let base = BluetoothDtmMemoryGraphModelAddress::new(base)
        .expect("test base has valid compressed-pointer syntax");
    BluetoothDtmMemoryGraphStorage::pin_static_model(storage, base, allocation_config())
        .expect("test graph fits physical controller SRAM")
}

fn link_state(role: BluetoothDtmRole) -> BluetoothDtmLinkStateReset {
    BluetoothDtmLinkStateReset::new(BluetoothDtmDefaultTxPowerDbm::new(0), role)
}

fn epoch() -> BluetoothControllerSchedulerEpoch {
    BluetoothControllerSchedulerEpoch::new(
        BluetoothControllerTimeSample::for_validation(100),
        1_000,
        BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale(),
    )
}

fn channel() -> BluetoothDtmChannel {
    BluetoothDtmChannel::new(5).expect("channel five is valid")
}

fn margin() -> u32 {
    crate::BluetoothSchedulerSoftwareConfig::reviewed_standalone().preparation_lead_micros()
}

fn tx_timing() -> crate::BluetoothDtmTxSchedulerTiming {
    BluetoothDtmTxTimingMicros::new(
        BluetoothDtmPayloadLength::from_hci_image(3),
        BluetoothDtmPhy::Le2M,
        0,
    )
    .scheduler_timing()
}

fn tx_window() -> crate::BluetoothDtmTxEventWindow {
    tx_timing().initial_event_window(
        crate::BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
        BluetoothSchedulerInstant::from_image(64),
        BluetoothSchedulerInstant::from_image(1_119),
    )
}

fn rx_initial_window() -> crate::BluetoothDtmRxInitialEventWindow {
    crate::BluetoothDtmRxInitialEventWindow::new(
        crate::BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
        BluetoothSchedulerInstant::from_image(64),
        BluetoothSchedulerInstant::from_image(1_119),
    )
}

fn item(role: BluetoothDtmRole) -> BluetoothDtmSchedulerItemEvent {
    match role {
        BluetoothDtmRole::Transmitter => BluetoothDtmSchedulerItemEvent::new_transmitter(
            channel(),
            BluetoothDtmPhy::Le2M,
            tx_window(),
        ),
        BluetoothDtmRole::Receiver => BluetoothDtmSchedulerItemEvent::new_initial_receiver(
            channel(),
            BluetoothDtmPhy::LeCoded,
            rx_initial_window(),
        ),
    }
    .expect("selected PHY is valid for its role")
}

fn timing_policy() -> BluetoothSchedulerTimingPolicy {
    BluetoothSchedulerTimingPolicy::from_scheduler_config(
        crate::BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
        BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale(),
    )
}

fn admission_sample() -> BluetoothControllerTimeSample {
    BluetoothControllerTimeSample::for_validation(92)
}

fn reservation<const CAPACITY: usize>(
    timeline: &mut BluetoothSchedulerTimeline<CAPACITY>,
    role: BluetoothDtmRole,
) -> BluetoothDtmSchedulerReservation<BluetoothSchedulerSequenceReady> {
    initial_reservation_for_event(timeline, item(role))
}

fn initial_reservation_for_event<const CAPACITY: usize>(
    timeline: &mut BluetoothSchedulerTimeline<CAPACITY>,
    event: BluetoothDtmSchedulerItemEvent,
) -> BluetoothDtmSchedulerReservation<BluetoothSchedulerSequenceReady> {
    let epoch = epoch();
    let window = timeline
        .reserve_initial_window(
            event.raw_start(epoch),
            event.raw_end(epoch),
            timing_policy(),
            admission_sample(),
        )
        .expect("the first guarded deadline is open");
    BluetoothDtmSchedulerReservation::new(window, event, epoch)
        .authorize_sequence(admission_sample())
        .expect("the initial sequence deadline is open")
}

fn recurring_reservation_for_event<const CAPACITY: usize>(
    timeline: &mut BluetoothSchedulerTimeline<CAPACITY>,
    event: BluetoothDtmSchedulerItemEvent,
) -> BluetoothDtmSchedulerReservation<BluetoothSchedulerSequenceReady> {
    let epoch = epoch();
    let window = timeline
        .reserve_recurring_window(
            event.raw_start(epoch),
            event.raw_end(epoch),
            timing_policy(),
        )
        .expect("the exact recurring window is collision-free");
    BluetoothDtmSchedulerReservation::new(window, event, epoch)
        .authorize_sequence(admission_sample())
        .expect("the sole recurring sequence deadline is open")
}

#[test]
fn tx_plan_requires_and_retains_the_prepared_packet_identity() {
    let mut timeline = BluetoothSchedulerTimeline::<1>::new();
    let reset = BluetoothDtmLinkStateReset::new(
        BluetoothDtmDefaultTxPowerDbm::new(20),
        BluetoothDtmRole::Transmitter,
    );
    let plan = BluetoothDtmReviewedEventWordsPlan::new_transmitter(
        reset,
        reservation(&mut timeline, BluetoothDtmRole::Transmitter),
    )
    .expect("both transforms encode TX");

    let packet = owner(0x2f07_0000).prepare_dtm_tx_packet(
        BluetoothDtmPayloadPattern::Repeated11110000,
        BluetoothDtmPayloadLength::from_hci_image(3),
    );

    let prepared = plan
        .prepare_first(
            packet,
            channel(),
            BluetoothDtmPhy::Le2M,
            tx_timing(),
            margin(),
            tx_window(),
        )
        .expect("the consumed graph supplies both private link projections");
    let scheduler_prepared = prepared.prepare_scheduler_bookkeeping();
    assert_eq!(
        scheduler_prepared.packet_pattern(),
        BluetoothDtmPayloadPattern::Repeated11110000
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
    let mut timeline = BluetoothSchedulerTimeline::<1>::new();
    let reset = link_state(BluetoothDtmRole::Receiver);
    let plan = BluetoothDtmReviewedEventWordsPlan::new_receiver(
        reset,
        reservation(&mut timeline, BluetoothDtmRole::Receiver),
    )
    .expect("both transforms encode RX");

    let prepared = plan
        .prepare_first(
            BluetoothDtmReceiverCpuOwned::new(owner(0x2f00_0100)),
            channel(),
            BluetoothDtmPhy::LeCoded,
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
    let mut timeline = BluetoothSchedulerTimeline::<1>::new();
    let reset = link_state(BluetoothDtmRole::Transmitter);
    let pattern = BluetoothDtmPayloadPattern::Repeated11110000;
    let length = BluetoothDtmPayloadLength::from_hci_image(3);
    let memory = owner(0x2f06_0000)
        .prepare_dtm_tx_packet(pattern, length)
        .discard();
    let committed_window = tx_window();
    let candidate_window = tx_timing()
        .advance_event_window(
            crate::BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
            committed_window,
            BluetoothSchedulerInstant::from_image(1_100),
        )
        .window();
    let facts = BluetoothDtmTransmitterCommandFacts {
        link_state: reset,
        channel: channel(),
        phy: BluetoothDtmPhy::Le2M,
        timing: tx_timing(),
        margin: margin(),
        pattern,
        length,
    };
    let active = BluetoothDtmActiveTransmitterCpuOwned {
        memory,
        facts,
        last_committed_window: committed_window,
        status: BluetoothDtmSchedulerItemCompletionStatus::Zero,
    };
    let event = BluetoothDtmSchedulerItemEvent::new_transmitter(
        channel(),
        BluetoothDtmPhy::Le2M,
        candidate_window,
    )
    .expect("TX event accepts LE 2M");
    let plan = BluetoothDtmReviewedEventWordsPlan::new_transmitter(
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
    assert_eq!(active.phy(), BluetoothDtmPhy::Le2M);
    assert_eq!(active.timing(), tx_timing());
    assert_eq!(active.margin(), margin());
    assert_eq!(active.last_committed_window, committed_window);
    assert_eq!(
        active.status(),
        BluetoothDtmSchedulerItemCompletionStatus::Zero
    );
    assert!(timeline.release(reservation.into_window()).is_ok());
}

#[test]
fn recurring_rx_cancellation_restores_session_and_committed_window() {
    let mut timeline = BluetoothSchedulerTimeline::<1>::new();
    let reset = link_state(BluetoothDtmRole::Receiver);
    let committed_window = BluetoothDtmRxCommittedWindow::Initial(rx_initial_window());
    let candidate_window = BluetoothDtmRxRecurringEventWindow::new(
        crate::BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
        BluetoothSchedulerInstant::from_image(1_100),
        BluetoothSchedulerInstant::from_image(1_120),
    );
    let facts = BluetoothDtmReceiverCommandFacts {
        link_state: reset,
        channel: channel(),
        phy: BluetoothDtmPhy::LeCoded,
        margin: margin(),
    };
    let active = BluetoothDtmActiveReceiverCpuOwned {
        memory: owner(0x2f05_0000),
        facts,
        session: crate::dtm_rx_completion::BluetoothDtmReceiverSession::new(),
        last_committed_window: committed_window,
    };
    let event = BluetoothDtmSchedulerItemEvent::new_recurring_receiver(
        channel(),
        BluetoothDtmPhy::LeCoded,
        candidate_window,
    )
    .expect("RX event accepts LE Coded");
    let plan = BluetoothDtmReviewedEventWordsPlan::new_receiver(
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
    assert_eq!(active.phy(), BluetoothDtmPhy::LeCoded);
    assert_eq!(active.margin(), margin());
    assert_eq!(active.last_committed_window, committed_window);
    assert_eq!(active.received_packet_count(), 0);
    assert!(timeline.release(reservation.into_window()).is_ok());
}

#[test]
fn completed_events_commit_only_the_candidate_window_into_active_owners() {
    let reset_tx = link_state(BluetoothDtmRole::Transmitter);
    let tx_candidate = tx_timing()
        .advance_event_window(
            crate::BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
            tx_window(),
            BluetoothSchedulerInstant::from_image(1_100),
        )
        .window();
    let tx_facts = BluetoothDtmTransmitterCommandFacts {
        link_state: reset_tx,
        channel: channel(),
        phy: BluetoothDtmPhy::Le2M,
        timing: tx_timing(),
        margin: margin(),
        pattern: BluetoothDtmPayloadPattern::Repeated11110000,
        length: BluetoothDtmPayloadLength::from_hci_image(3),
    };
    let recycled_tx = BluetoothDtmRecycledEvent::<BluetoothDtmTransmitterEvent> {
        memory: owner(0x2f04_0000),
        context: BluetoothDtmEventContext::Transmitter(BluetoothDtmTransmitterEventContext {
            facts: tx_facts,
            event_window: tx_candidate,
        }),
        status: BluetoothDtmSchedulerItemCompletionStatus::Zero,
        _role: core::marker::PhantomData,
    };
    assert_eq!(recycled_tx.packet_pattern(), tx_facts.pattern);
    assert_eq!(recycled_tx.packet_length(), tx_facts.length);
    let tx = recycled_tx.into_next();
    assert_eq!(tx.packet_pattern(), tx_facts.pattern);
    assert_eq!(tx.packet_length(), tx_facts.length);
    assert_eq!(tx.last_committed_window(), tx_candidate);
    assert_eq!(tx.status(), BluetoothDtmSchedulerItemCompletionStatus::Zero);

    let reset_rx = link_state(BluetoothDtmRole::Receiver);
    let rx_candidate = BluetoothDtmRxRecurringEventWindow::new(
        crate::BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
        BluetoothSchedulerInstant::from_image(1_100),
        BluetoothSchedulerInstant::from_image(1_120),
    );
    let recycled_rx = BluetoothDtmRecycledEvent::<BluetoothDtmReceiverEvent> {
        memory: owner(0x2f03_0000),
        context: BluetoothDtmEventContext::Receiver(BluetoothDtmReceiverEventContext {
            facts: BluetoothDtmReceiverCommandFacts {
                link_state: reset_rx,
                channel: channel(),
                phy: BluetoothDtmPhy::LeCoded,
                margin: margin(),
            },
            session: crate::dtm_rx_completion::BluetoothDtmReceiverSession::new(),
            event_window: BluetoothDtmRxCommittedWindow::Recurring(rx_candidate),
        }),
        status: BluetoothDtmSchedulerItemCompletionStatus::Zero,
        _role: core::marker::PhantomData,
    };
    assert_eq!(recycled_rx.role(), BluetoothDtmRole::Receiver);
    assert_eq!(
        recycled_rx.status(),
        BluetoothDtmSchedulerItemCompletionStatus::Zero
    );
    let rx = recycled_rx.into_next();
    assert_eq!(
        rx.last_committed_window,
        BluetoothDtmRxCommittedWindow::Recurring(rx_candidate)
    );
    assert_eq!(rx.received_packet_count(), 0);
}

#[test]
fn active_roles_hold_the_reclaimed_graph_through_test_end_handoff() {
    let reset_tx = link_state(BluetoothDtmRole::Transmitter);
    let tx = BluetoothDtmActiveTransmitterCpuOwned {
        memory: owner(0x2f02_0000),
        facts: BluetoothDtmTransmitterCommandFacts {
            link_state: reset_tx,
            channel: channel(),
            phy: BluetoothDtmPhy::Le2M,
            timing: tx_timing(),
            margin: margin(),
            pattern: BluetoothDtmPayloadPattern::Repeated11110000,
            length: BluetoothDtmPayloadLength::from_hci_image(3),
        },
        last_committed_window: tx_window(),
        status: BluetoothDtmSchedulerItemCompletionStatus::Zero,
    };
    let ended = tx.into_test_ended();
    let stopping = crate::BluetoothDtmSessionStopping::new(ended);
    assert_eq!(stopping.report(), BluetoothDtmTestEndReport::Transmitter);
    assert_eq!(stopping.report().reported_packet_count(), 0);
    let _next_graph = stopping.response_published().begin_epoch().into_graph();

    let mut session = crate::dtm_rx_completion::BluetoothDtmReceiverSession::new();
    assert!(matches!(
        session.account_projection(BluetoothDtmRxResultProjection::from_word(0)),
        crate::BluetoothDtmRxCompletionOutcome::Counted {
            received_packet_count: 1,
            ..
        }
    ));
    let reset_rx = link_state(BluetoothDtmRole::Receiver);
    let rx = BluetoothDtmActiveReceiverCpuOwned {
        memory: owner(0x2f01_0000),
        facts: BluetoothDtmReceiverCommandFacts {
            link_state: reset_rx,
            channel: channel(),
            phy: BluetoothDtmPhy::LeCoded,
            margin: margin(),
        },
        session,
        last_committed_window: BluetoothDtmRxCommittedWindow::Initial(rx_initial_window()),
    };
    let ended = rx.into_test_ended();
    let stopping = crate::BluetoothDtmSessionStopping::new(ended);
    assert_eq!(
        stopping.report(),
        BluetoothDtmTestEndReport::Receiver {
            received_packets: 1
        }
    );
    assert_eq!(stopping.report().reported_packet_count(), 1);
    let _next_graph = stopping.response_published().begin_epoch().into_graph();
}

#[test]
fn plan_rejects_mixed_roles_before_it_can_consume_memory() {
    let mut timeline = BluetoothSchedulerTimeline::<1>::new();
    let reset = link_state(BluetoothDtmRole::Transmitter);
    let failure = match BluetoothDtmReviewedEventWordsPlan::new_transmitter(
        reset,
        reservation(&mut timeline, BluetoothDtmRole::Receiver),
    ) {
        Ok(_) => panic!("a receiver reservation cannot form a transmitter plan"),
        Err(failure) => failure,
    };
    assert_eq!(
        failure.error(),
        BluetoothDtmReviewedEventWordsPlanError::RoleMismatch {
            expected: BluetoothDtmRole::Transmitter,
            link_state: BluetoothDtmRole::Transmitter,
            scheduler_item: BluetoothDtmRole::Receiver,
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
    let mut timeline = BluetoothSchedulerTimeline::<1>::new();
    let event = item(BluetoothDtmRole::Receiver);
    let epoch = epoch();
    let window = timeline
        .reserve_initial_window(
            event.raw_start(epoch),
            event.raw_end(epoch),
            timing_policy(),
            admission_sample(),
        )
        .expect("the first guarded deadline is open");
    let reservation = BluetoothDtmSchedulerReservation::new(window, event, epoch);
    let failure = reservation
        .authorize_sequence(BluetoothControllerTimeSample::for_validation(93))
        .expect_err("the second sample reaches the guarded start");
    assert_eq!(
        failure.error(),
        BluetoothSchedulerSequenceAuthorizationError::DeadlineExpired
    );
    assert!(
        timeline
            .release(failure.into_reservation().into_window())
            .is_ok()
    );
}
