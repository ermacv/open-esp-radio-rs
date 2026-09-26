use std::{boxed::Box, vec::Vec};

use super::{
    EVENT_ITEM, LINK_STATE_PACKET_CONTROL, LINK_STATE_RX_HEAD, LINK_STATE_RX_PATH,
    LINK_STATE_SCHEDULER_HEAD, LINK_STATE_TX_PATH, PeripheralConnectionCapturedAnchorAvailability,
    PeripheralConnectionDataChannel, PeripheralConnectionDefaultTxPowerDbm,
    PeripheralConnectionError, PeripheralConnectionEventSpan, PeripheralConnectionFirstEvent,
    PeripheralConnectionIdentity, PeripheralConnectionPool, PeripheralConnectionReceiveTime,
    PeripheralConnectionReceiveWait, PeripheralConnectionRecurringEvent,
    PeripheralConnectionRecurringReceiveWait, PeripheralConnectionSchedulerItemCompletionStatus,
    PeripheralConnectionSchedulerPriority, PeripheralConnectionSchedulerWindow,
    PeripheralConnectionStorage, PeripheralConnectionTransmitPduKind,
    SCHEDULER_ITEM_ALLOCATION_NUMBER, SCHEDULER_ITEM_CAPTURE_AVAILABLE,
    SCHEDULER_ITEM_CAPTURED_ANCHOR, SCHEDULER_ITEM_RECEIVE_WAIT_CONFIGURATION,
};
use crate::{
    DirectionFindingWorkspaceLink, DirectionFindingWorkspaceModelAddress,
    DirectionFindingWorkspaceStorage,
    le_rx_packet::LeRxOutcome,
    scheduler_pool::{
        SchedulerAllocationConfig, SchedulerPoolError, SchedulerPoolModelAddress,
        SchedulerRoleInstance, SchedulerRolePoolStorage,
    },
};

type Pool = PeripheralConnectionPool<2>;

const IDENTITY: PeripheralConnectionIdentity =
    PeripheralConnectionIdentity::new([0xd4, 0xc3, 0xb2, 0xa1], [0x33, 0x22, 0x11]);

fn pool() -> Pool {
    let storage = Box::leak(Box::new(SchedulerRolePoolStorage::<
        PeripheralConnectionStorage,
        2,
    >::new()));
    let numbers = SchedulerAllocationConfig::new(2, 2, 0)
        .unwrap()
        .connections();
    Pool::bind_model(
        storage,
        SchedulerPoolModelAddress::new(0x2f00_0100).expect("test base is aligned"),
        numbers,
    )
    .expect("the pool fits controller SRAM")
}

fn workspace() -> DirectionFindingWorkspaceLink {
    let storage = Box::leak(Box::new(DirectionFindingWorkspaceStorage::new()));
    DirectionFindingWorkspaceStorage::pin_static_model(
        storage,
        DirectionFindingWorkspaceModelAddress::new(0x2f02_0000).unwrap(),
    )
    .unwrap()
    .binding()
    .link()
}

fn first_event() -> PeripheralConnectionFirstEvent {
    PeripheralConnectionFirstEvent {
        channel: PeripheralConnectionDataChannel::new(0).unwrap(),
        receive_time: PeripheralConnectionReceiveTime::from_controller_ticks(24_000),
        event_span: PeripheralConnectionEventSpan::new(23_000).unwrap(),
        window: PeripheralConnectionSchedulerWindow::new(25_000, 26_000).unwrap(),
        receive_wait: PeripheralConnectionReceiveWait::new(1_250, 50).unwrap(),
        default_tx_power: PeripheralConnectionDefaultTxPowerDbm::new(0),
        priority: PeripheralConnectionSchedulerPriority::FIRST_EVENT,
        raw_sequence_lead: 100,
    }
}

fn recurring_event(receive_wait_micros: u32) -> PeripheralConnectionRecurringEvent {
    PeripheralConnectionRecurringEvent {
        channel: PeripheralConnectionDataChannel::new(20).unwrap(),
        event_span: PeripheralConnectionEventSpan::new(23_000).unwrap(),
        window: PeripheralConnectionSchedulerWindow::new(50_000, 51_000).unwrap(),
        receive_wait: PeripheralConnectionRecurringReceiveWait::new(receive_wait_micros).unwrap(),
        priority: PeripheralConnectionSchedulerPriority::RECURRING_BASELINE,
        raw_sequence_lead: 100,
    }
}

/// An acquired connection whose first event has run and been finished.
fn active(pool: &mut Pool) -> SchedulerRoleInstance {
    let instance = pool.acquire().unwrap();
    pool.prepare_identity(&instance, IDENTITY).unwrap();
    pool.prepare_first_event(&instance, first_event(), workspace())
        .unwrap();
    let id = pool.submit(&instance, EVENT_ITEM).unwrap();
    pool.retire(id).unwrap();
    pool.finish_event(&instance).unwrap();
    instance
}

fn item_word(pool: &Pool, instance: &SchedulerRoleInstance, word: usize) -> u32 {
    let (graph, _, _) = pool.shared(instance).unwrap();
    graph.items[EVENT_ITEM].words[word].get()
}

/// Hardware receives one PDU into the private chain.
fn receive(pool: &mut Pool, instance: &SchedulerRoleInstance, pdu: &[u8]) {
    let cpu = pool.cpu(instance).unwrap();
    let rx = cpu.state.rx.as_mut().unwrap();
    assert!(rx.view(&cpu.graph.rx).emulate_receive(pdu, 0));
}

/// Hardware transmits the packet after its TX cursor and, when the peer
/// acknowledges it, completes it and advances the cursor.
fn transmit(
    pool: &mut Pool,
    instance: &SchedulerRoleInstance,
    acknowledged: bool,
) -> Option<Vec<u8>> {
    let cpu = pool.cpu(instance).unwrap();
    let graph = &cpu.graph;
    let headers = [
        (&graph.tx_sentinel, cpu.binding.tx_sentinel),
        (&graph.tx_successor, cpu.binding.tx_successor),
    ];
    let path = &graph.link_state.words[LINK_STATE_TX_PATH];
    let current = path.get() & 0x000f_ffff;
    let (cursor, _) = headers
        .iter()
        .find(|(_, address)| address.compressed_image() == current)?;
    let next = cursor.snapshot()[0] & 0x000f_ffff;
    let (header, address) = headers
        .iter()
        .find(|(_, address)| address.compressed_image() == next)?;
    if header.transmission_completed() {
        return None;
    }
    let pdu = graph.tx_packet.model_pdu().to_vec();
    if acknowledged {
        header.model_complete_transmission();
        path.set((path.get() & !0x000f_ffff) | address.compressed_image());
    }
    Some(pdu)
}

#[test]
fn the_first_event_uses_the_private_chain_and_takes_the_free_head() {
    let mut pool = pool();
    let instance = pool.acquire().unwrap();
    assert_eq!(
        pool.prepare_first_event(&instance, first_event(), workspace()),
        Err(PeripheralConnectionError::State)
    );
    pool.prepare_identity(&instance, IDENTITY).unwrap();
    assert_eq!(pool.identity(&instance), Ok(IDENTITY));
    let event = pool
        .prepare_first_event(&instance, first_event(), workspace())
        .unwrap();
    assert_eq!(event.item(), EVENT_ITEM);
    assert_eq!(event.raw_window(), (25_000, 26_000));

    let (graph, binding, state) = pool.shared(&instance).unwrap();
    let words = &graph.link_state.words;
    let rx = state.rx.unwrap();
    // No RX class; the hardware cursor starts at the private software head.
    assert_eq!(words[LINK_STATE_PACKET_CONTROL].get(), 0);
    assert_eq!(
        words[LINK_STATE_RX_PATH].get() & 0x000f_ffff,
        rx.head_link().compressed_image()
    );
    assert_eq!(words[LINK_STATE_RX_HEAD].get(), rx.snapshot().0);
    assert_eq!(
        words[LINK_STATE_SCHEDULER_HEAD].get(),
        binding.items[0].controller_address().address()
    );
    let header = graph.items[EVENT_ITEM].header();
    assert_eq!(header.hardware_next_image(), 0);
    assert_eq!((header.raw_start(), header.raw_end()), (25_000, 26_000));
    assert_eq!(header.sequence_start(), 25_100);
    assert_eq!(
        pool.receive_time(&instance)
            .unwrap()
            .wrapping_controller_ticks(),
        24_000
    );
}

#[test]
fn connections_are_numbered_after_the_advertising_instances() {
    let mut pool = pool();
    let first = pool.acquire().unwrap();
    let second = pool.acquire().unwrap();
    assert_eq!(
        item_word(&pool, &first, SCHEDULER_ITEM_ALLOCATION_NUMBER),
        3
    );
    assert_eq!(
        item_word(&pool, &second, SCHEDULER_ITEM_ALLOCATION_NUMBER),
        4
    );
}

#[test]
fn finishing_reports_the_capture_and_returns_the_item() {
    let mut pool = pool();
    let instance = pool.acquire().unwrap();
    pool.prepare_identity(&instance, IDENTITY).unwrap();
    pool.prepare_first_event(&instance, first_event(), workspace())
        .unwrap();
    assert_eq!(
        pool.submit(&instance, 0),
        Err(SchedulerPoolError::NotPrepared)
    );
    let id = pool.submit(&instance, EVENT_ITEM).unwrap();
    {
        let (graph, _, _) = pool.shared(&instance).unwrap();
        let item = &graph.items[EVENT_ITEM];
        item.words[SCHEDULER_ITEM_CAPTURED_ANCHOR].set(0x1234);
        item.header().set_status(SCHEDULER_ITEM_CAPTURE_AVAILABLE);
    }
    pool.retire(id).unwrap();
    let result = pool.finish_event(&instance).unwrap();
    assert_eq!(
        result.status,
        PeripheralConnectionSchedulerItemCompletionStatus::NonZero
    );
    let PeripheralConnectionCapturedAnchorAvailability::Available(anchor) = result.capture else {
        panic!("the item published a capture")
    };
    assert_eq!(anchor.wrapping_controller_ticks(), 0x1234);

    let (graph, binding, _) = pool.shared(&instance).unwrap();
    assert_eq!(
        graph.link_state.words[LINK_STATE_SCHEDULER_HEAD].get(),
        binding.items[EVENT_ITEM].controller_address().address()
    );
    assert_eq!(
        graph.items[EVENT_ITEM].header().hardware_next_image(),
        binding.items[0].compressed_image()
    );
}

#[test]
fn an_unexecuted_item_finishes_as_aborted() {
    let mut pool = pool();
    let instance = pool.acquire().unwrap();
    pool.prepare_identity(&instance, IDENTITY).unwrap();
    pool.prepare_first_event(&instance, first_event(), workspace())
        .unwrap();
    let id = pool.submit(&instance, EVENT_ITEM).unwrap();
    crate::SchedulerItemSpace::new()
        .with(&pool)
        .prepare_for_list(id, None, None);
    pool.retire(id).unwrap();
    assert_eq!(
        pool.finish_event(&instance).unwrap().status,
        PeripheralConnectionSchedulerItemCompletionStatus::Aborted
    );
}

#[test]
fn recurring_events_select_the_receive_wait_form() {
    let mut pool = pool();
    let instance = active(&mut pool);
    for (micros, image) in [
        (0, 1),
        (500, 0x000f_0000 | 500),
        (0x1_0000, 0x001f_0000 | 0x8000),
    ] {
        pool.prepare_recurring_event(&instance, recurring_event(micros))
            .unwrap();
        assert_eq!(
            item_word(&pool, &instance, SCHEDULER_ITEM_RECEIVE_WAIT_CONFIGURATION),
            image
        );
        assert_eq!(
            pool.prepare_recurring_event(&instance, recurring_event(micros)),
            Err(PeripheralConnectionError::State)
        );
        let id = pool.submit(&instance, EVENT_ITEM).unwrap();
        pool.retire(id).unwrap();
        pool.finish_event(&instance).unwrap();
    }
    // The identity persists across events.
    assert_eq!(pool.identity(&instance), Ok(IDENTITY));
}

#[test]
fn received_pdus_rotate_through_the_private_chain() {
    let mut pool = pool();
    let instance = active(&mut pool);
    assert_eq!(pool.receive(&instance), Ok(None));
    for round in 0..10u8 {
        let pdu = [0x02, 1, round];
        receive(&mut pool, &instance, &pdu);
        let Ok(Some(LeRxOutcome::Received(received))) = pool.receive(&instance) else {
            panic!("the connection takes its own packet")
        };
        assert_eq!(received.as_bytes(), pdu);
        assert_eq!(pool.receive(&instance), Ok(None));
        let (graph, _, state) = pool.shared(&instance).unwrap();
        assert_eq!(
            graph.link_state.words[LINK_STATE_RX_HEAD].get(),
            state.rx.unwrap().snapshot().0
        );
    }
}

#[test]
fn one_packet_is_pending_until_the_peer_acknowledges_it() {
    let mut pool = pool();
    let instance = active(&mut pool);
    assert!(pool.can_enqueue_transmission(&instance));
    assert_eq!(
        pool.enqueue_transmission(
            &instance,
            PeripheralConnectionTransmitPduKind::Control,
            &[0x0c, 1]
        ),
        Ok(true)
    );
    assert!(!pool.can_enqueue_transmission(&instance));
    assert_eq!(
        pool.enqueue_transmission(
            &instance,
            PeripheralConnectionTransmitPduKind::Control,
            &[1]
        ),
        Ok(false)
    );
    assert_eq!(pool.reclaim_transmission(&instance), Ok(false));

    assert!(transmit(&mut pool, &instance, false).is_some());
    assert_eq!(pool.reclaim_transmission(&instance), Ok(false));
    assert!(transmit(&mut pool, &instance, true).is_some());
    assert_eq!(pool.reclaim_transmission(&instance), Ok(true));
    assert!(transmit(&mut pool, &instance, false).is_none());

    // The next packet uses the other header.
    assert_eq!(
        pool.enqueue_transmission(
            &instance,
            PeripheralConnectionTransmitPduKind::DataStartOrComplete,
            &[1, 2, 3]
        ),
        Ok(true)
    );
    assert!(transmit(&mut pool, &instance, true).is_some());
    assert_eq!(pool.reclaim_transmission(&instance), Ok(true));
}

#[test]
fn release_returns_a_pristine_instance() {
    let mut pool = pool();
    let instance = active(&mut pool);
    pool.release(instance).unwrap();
    let instance = pool.acquire().unwrap();
    assert_eq!(
        pool.enqueue_transmission(
            &instance,
            PeripheralConnectionTransmitPduKind::Control,
            &[1]
        ),
        Err(PeripheralConnectionError::State)
    );
    let (graph, binding, _) = pool.shared(&instance).unwrap();
    assert_eq!(
        graph.link_state.words[LINK_STATE_SCHEDULER_HEAD].get(),
        binding.items[EVENT_ITEM].controller_address().address()
    );
}
