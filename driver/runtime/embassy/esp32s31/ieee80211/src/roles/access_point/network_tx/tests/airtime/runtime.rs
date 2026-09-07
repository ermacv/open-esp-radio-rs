//! Real AP start/service and modeled hardware: packet, PS and budget ownership.
use super::*;

mod fifo;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Case {
    Unicast,
    Group,
    DetachFailure,
    UnknownCost,
    Awake,
    Dtim,
    AggregateRetry,
    PreparedOrdinary,
    PreparedSleep,
    SelectionCapacity,
    FifoReservationFailure,
    FifoStorageFull,
    ScheduledOrdinary,
    ScheduledFifo,
    EmptySnapshot,
    PsPollPriority,
    DtimPriority,
}

#[test]
fn unicast_completion_settles_published_work() {
    run(Case::Unicast);
}

#[test]
fn group_completion_settles_without_ack() {
    run(Case::Group);
}

#[test]
fn failed_detach_keeps_budget_outstanding() {
    run(Case::DetachFailure);
}

#[test]
fn unknown_cost_keeps_terminal_receipt() {
    run(Case::UnknownCost);
}

#[test]
fn awake_power_save_release_settles_on_completion() {
    run(Case::Awake);
}

#[test]
fn dtim_group_release_settles_on_completion() {
    run(Case::Dtim);
}

#[test]
fn aggregate_retry_keeps_one_reservation_until_terminal_release() {
    run(Case::AggregateRetry);
}

#[test]
fn selected_prepared_head_reuses_its_grant_for_ordinary_publication() {
    run(Case::PreparedOrdinary);
}

#[test]
fn sleeping_after_selection_refunds_the_unpublished_grant_and_retains_packet() {
    run(Case::PreparedSleep);
}

#[test]
fn unavailable_ledger_does_not_dequeue_the_source_head() {
    run(Case::SelectionCapacity);
}

#[test]
fn fifo_reservation_failure_retains_the_claimed_packet() {
    run(Case::FifoReservationFailure);
}

#[test]
fn fifo_without_rollback_room_reports_capacity_before_dequeue() {
    run(Case::FifoStorageFull);
}

#[test]
fn deficit_mode_pulls_and_completes_ordinary_tx_without_a_standby_arena() {
    run(Case::ScheduledOrdinary);
}

#[test]
fn deficit_mode_rejects_an_opaque_source_before_claiming_its_head() {
    run(Case::ScheduledFifo);
}

#[test]
fn publication_after_empty_snapshot_cannot_bypass_deficit_selection() {
    run(Case::EmptySnapshot);
}

#[test]
fn ps_poll_preempts_selected_head_but_preserves_its_packet() {
    run(Case::PsPollPriority);
}

#[test]
fn dtim_preempts_selected_head_but_preserves_its_packet() {
    run(Case::DtimPriority);
}

fn run(case: Case) {
    use super::super::support::{allocator, packet, with_authorized_ap_capabilities};
    use crate::datapath::tx::resources::AggregateTxResources;
    use crate::datapath::{DatapathTxConsumer, PinnedTxPool, PinnedTxResources};
    use crate::roles::access_point::*;
    use core::{
        future::Future,
        pin::pin,
        task::{Context, Poll, Waker},
    };
    use embassy_sync::blocking_mutex::raw::NoopRawMutex;
    use hardware::{Hardware, Power, Timer};
    use open_esp_radio_embassy_net::{NetworkInterfaceId, OwnedEndpointResources};
    use open_esp_radio_esp32s31_hal::types::MacTxCompletionObservation;
    use open_esp_radio_esp32s31_wifi::ordinary_tx::WifiTxResources;
    use open_esp_radio_esp32s31_wifi_ap::tx::Esp32s31ApTxConfig;
    use open_esp_radio_esp32s31_wifi_mac::{
        tx::TxSlot,
        tx_ampdu::{HtAmpduTxResources, HtAmpduTxStorage, RetainedAmpduDmaStorage},
        tx_runtime::WifiTxRuntimePolicy,
    };
    use std::boxed::Box;
    // Unicast, group, failed detach and an explicitly unknown cost.
    with_authorized_ap_capabilities(case == Case::AggregateRetry, |engine| {
        let identity = engine.admit_downlink([4; 6]).unwrap().identity();
        let peer = if matches!(case, Case::Group | Case::Dtim | Case::DtimPriority) {
            AccessPointAirtimePeer::Group
        } else {
            AccessPointAirtimePeer::Unicast(identity)
        };
        let mut hardware = Hardware::default();
        let mut slot = pin!(TxSlot::<512>::new_model());
        let mac = Esp32s31ApMac::new(
            engine,
            WifiTxResources {
                slot: slot.as_mut(),
                policy: WifiTxRuntimePolicy::vendor_defaults(),
                power: Power,
                entropy: || 0,
                timer: Timer::default(),
            },
            Esp32s31ApTxConfig {
                publication_timeout_micros: 1000,
            },
        );
        let mut rx = [0; 512];
        let mut tx = [0; 512];
        let mut dispatch = Esp32s31ApRxDispatcher::new(Esp32s31ApRxConfig {
            access_point: [2; 6],
            ingress: RxIngressConfig {
                ring_entry_limit: 1,
                csi_config: 0,
                flags: 0,
            },
            security: WifiSecurityMode::Open,
        });
        let ba = Esp32s31StaApRxBlockAck::new();
        let reorder_storage = RxReorderFrameStorage::<512>::new();
        let reorder = Box::leak(Box::new(Esp32s31AccessPointRxReorder::<512>::new()));
        let mut control = Esp32s31AccessPointProtocolProcessor::new(
            mac,
            &mut rx,
            &mut tx,
            &mut dispatch,
            &ba,
            reorder,
            &reorder_storage,
            #[cfg(feature = "diagnostics")]
            Box::leak(Box::new(AccessPointObservationStorage::default())),
        );
        let packets = allocator::<2>();
        let endpoint = Box::leak(Box::new(OwnedEndpointResources::<NoopRawMutex, 1, 2>::new()));
        let interface = NetworkInterfaceId::new(0);
        let (mut device, radio) = endpoint.split(interface, [2; 6], allocator::<1>());
        radio.link_controller().set_link_up(true);
        let dma =
            PinnedTxPool::<64, 64, 16, 2>::pin_static(Box::leak(Box::new(PinnedTxPool::new())));
        let dma_resources = Box::leak(Box::new(
            PinnedTxResources::<NoopRawMutex, 64, 64, 16, 2>::new(),
        ));
        let source =
            DatapathTxConsumer::new(&radio, dma_resources.split(dma).for_interface(interface));
        let mut aggregate_storage = pin!(HtAmpduTxStorage::<2, 0>::new());
        let mut retained = RetainedAmpduDmaStorage::new();
        let mut aggregate = Esp32s31AccessPointAmpdu::new(
            AggregateTxResources::single(
                HtAmpduTxResources::new_model(aggregate_storage.as_mut()).unwrap(),
                &mut retained,
            ),
            4095,
            2,
        );
        let mut ledger = AccessPointAirtimeStorage::new(us(1000));
        if matches!(case, Case::SelectionCapacity | Case::FifoReservationFailure) {
            use open_esp_radio_wifi_datapath::airtime::AirtimeCandidate;
            // A prior quarantined owner can leave both reservations outstanding.
            let mut scheduler = ledger.scheduler();
            let candidate = AirtimeCandidate {
                key: peer,
                minimum_micros: us(100),
            };
            let _first = scheduler.reserve([candidate]).unwrap().unwrap();
            let _second = scheduler.reserve([candidate]).unwrap().unwrap();
        }

        let mut frames = AccessPointTxStorage::new();
        let mut owner = Esp32s31AccessPointNetworkTx::new_with_airtime_accounting(
            &mut frames,
            None,
            &mut ledger,
            us(100),
            if case == Case::UnknownCost {
                |_, _| None
            } else {
                tariff
            },
        );
        if matches!(
            case,
            Case::ScheduledOrdinary | Case::ScheduledFifo | Case::EmptySnapshot
        ) {
            owner.airtime = owner.airtime.take().map(Accounting::with_peer_selection);
        }
        if case == Case::AggregateRetry {
            use super::super::super::AggregateServicePhase;
            use open_esp_radio_esp32s31_hal::types::MacHtAmpduCompletionObservation;
            use open_esp_radio_esp32s31_wifi_ap::engine::Esp32s31ApAggregateFrame;
            use open_esp_radio_esp32s31_wifi_mac::tx::{
                HtChannelWidth, HtGuardInterval, HtMcs, HtRate,
            };
            use open_esp_radio_ieee80211::ap::EncodedApFrame;
            use open_esp_radio_wifi_datapath::SelectedBurstMaterializer;
            // Model already-encoded AP MPDUs: this exercises runtime completion,
            // not WPA2/BA admission or the production encoder.
            owner
                .airtime
                .as_mut()
                .unwrap()
                .reserve_active(control.mac.engine(), ApTxFlowKey::associated(identity))
                .unwrap();
            let rate = HtRate::new(
                HtMcs::Mcs7,
                HtGuardInterval::Short400Ns,
                HtChannelWidth::Mhz20,
            );
            aggregate.active_mut().begin([4; 6], rate, 0, 0).unwrap();
            for sequence in 0..2 {
                device.transmit(packet(packets, 4, sequence)).unwrap();
                let frame = source
                    .try_materialize(radio.try_receive_tx().unwrap())
                    .ok()
                    .unwrap();
                aggregate
                    .active_mut()
                    .push(
                        [4; 6],
                        frame,
                        Esp32s31ApAggregateFrame {
                            encoded: EncodedApFrame {
                                offset: 32,
                                length: 48,
                            },
                            hardware_key_selector: 0,
                            sequence_number: u16::from(sequence),
                        },
                    )
                    .unwrap();
            }
            let (_, ordinary) = control.mac.try_aggregate_adapter().unwrap();
            aggregate
                .active_mut()
                .publish(ordinary, &mut hardware)
                .unwrap();
            owner.airtime.as_mut().unwrap().publish_active();
            owner.aggregate_phase = Some(AggregateServicePhase::Published(1000));
            // Reserve the successor while this exchange still owns hardware.
            owner
                .airtime
                .as_mut()
                .unwrap()
                .reserve_standby(control.mac.engine(), ApTxFlowKey::associated(identity))
                .unwrap();
            hardware.aggregate_completion = Some(MacHtAmpduCompletionObservation::new_model(
                MacTxCompletionObservation::new_model(0, 0),
                0,
                0,
                1,
                true,
            ));
            let complete = WifiTxWake::Interrupt {
                events: open_esp_radio_esp32s31_wifi_mac::irq::EVENT_TX_COMPLETE,
            };
            assert_eq!(
                owner.service(&mut aggregate, &mut control, &mut hardware, complete),
                Ok(WifiTxProgress::Pending)
            );
            assert_eq!(hardware.ht_publications, 2);
            assert_eq!(
                owner.airtime_balance_micros(peer),
                Some(0),
                "selective retry retains the same outstanding grant"
            );
            hardware.aggregate_completion = Some(MacHtAmpduCompletionObservation::new_model(
                MacTxCompletionObservation::new_model(0, 0),
                0,
                1,
                1,
                true,
            ));
            assert_eq!(
                owner.service(&mut aggregate, &mut control, &mut hardware, complete),
                Ok(WifiTxProgress::Complete)
            );
            assert_eq!(owner.airtime_balance_micros(peer), Some(-200));
            assert!(aggregate.active_mut().is_idle());
            owner.airtime.as_mut().unwrap().cancel_standby().unwrap();
            assert_eq!(owner.airtime_balance_micros(peer), Some(800));
            return;
        }
        if matches!(
            case,
            Case::Awake | Case::Dtim | Case::PsPollPriority | Case::DtimPriority
        ) {
            control
                .mac
                .engine_mut()
                .observe_rx_peer_power_state([4; 6], ApPeerPowerState::Sleeping, 2)
                .unwrap();
        }
        device
            .transmit(packet(
                packets,
                if matches!(case, Case::Group | Case::Dtim | Case::DtimPriority) {
                    255
                } else {
                    4
                },
                1,
            ))
            .unwrap();
        if case == Case::EmptySnapshot {
            assert!(
                owner
                    .take_scheduled_active_or_network(
                        control.mac.engine_mut(),
                        &fifo::Fifo::<_, true>(&source),
                    )
                    .unwrap()
                    .is_none()
            );
            assert_eq!(radio.tx_queue_len(), 1);
            assert_eq!(radio.try_receive_tx().unwrap().ethernet()[14], 1);
            return;
        }
        if case == Case::ScheduledFifo {
            assert!(matches!(
                owner.take_scheduled_active_or_network(
                    control.mac.engine_mut(),
                    &fifo::Fifo::<_>(&source)
                ),
                Err(Esp32s31AccessPointDatapathError::Airtime(
                    AccessPointAirtimeError::DestinationQueuesRequired
                ))
            ));
            assert_eq!(radio.tx_queue_len(), 1);
            assert_eq!(radio.try_receive_tx().unwrap().ethernet()[14], 1);
            return;
        }
        if case == Case::FifoStorageFull {
            use super::super::super::AP_SOFTWARE_TX_CAPACITY;
            let pool = allocator::<AP_SOFTWARE_TX_CAPACITY>();
            let held = Box::leak(Box::new(OwnedEndpointResources::<
                NoopRawMutex,
                1,
                AP_SOFTWARE_TX_CAPACITY,
            >::new()));
            let (mut producer, consumer) = held.split(interface, [2; 6], allocator::<1>());
            consumer.link_controller().set_link_up(true);
            for n in 0..AP_SOFTWARE_TX_CAPACITY {
                // Model storage pinned by releases outside the voluntary queue.
                producer.transmit(packet(pool, 4, n as u8)).unwrap();
                let retained = consumer.try_receive_tx().unwrap();
                owner.frame_arena.insert(retained).ok().unwrap();
            }
            assert_eq!(owner.frame_arena.remaining_capacity(), 0);
            assert!(matches!(
                owner.take_scheduled_active_or_network(
                    control.mac.engine_mut(),
                    &fifo::Fifo::<_>(&source)
                ),
                Err(Esp32s31AccessPointDatapathError::Airtime(
                    AccessPointAirtimeError::SelectionStorageFull
                ))
            ));
            assert_eq!(radio.tx_queue_len(), 1);
            assert_eq!(radio.try_receive_tx().unwrap().ethernet()[14], 1);
            assert_eq!(hardware.legacy_publications, 0);
            return;
        }
        if case == Case::FifoReservationFailure {
            assert!(matches!(
                owner.take_scheduled_active_or_network(
                    control.mac.engine_mut(),
                    &fifo::Fifo::<_>(&source)
                ),
                Err(Esp32s31AccessPointDatapathError::Airtime(
                    AccessPointAirtimeError::Ledger(
                        open_esp_radio_wifi_datapath::airtime::AirtimeError::ReservationCapacity
                    )
                ))
            ));
            assert_eq!(radio.tx_queue_len(), 0);
            let key = ApTxFlowKey::associated(identity);
            let index = owner.active_frames.pop_key(key).unwrap();
            assert_eq!(owner.frame_arena.take(index).ethernet()[14], 1);
            assert_eq!(hardware.legacy_publications, 0);
            return;
        }
        if case == Case::SelectionCapacity {
            assert!(matches!(
                owner.take_scheduled_active_or_network(control.mac.engine_mut(), &source),
                Err(Esp32s31AccessPointDatapathError::Airtime(
                    AccessPointAirtimeError::Ledger(
                        open_esp_radio_wifi_datapath::airtime::AirtimeError::ReservationCapacity
                    )
                ))
            ));
            assert_eq!(radio.tx_queue_len(), 1);
            assert_eq!(radio.try_receive_tx().unwrap().ethernet()[14], 1);
            assert_eq!(hardware.legacy_publications, 0);
            return;
        }
        let mut result = if case == Case::ScheduledOrdinary {
            Poll::Ready(owner.start_prepared(&mut aggregate, &mut control, &mut hardware, &source))
        } else if matches!(case, Case::PreparedOrdinary | Case::PreparedSleep) {
            let selected = owner
                .take_scheduled_active_or_network(control.mac.engine_mut(), &source)
                .unwrap()
                .unwrap();
            let (key, frame) = selected.expect_frame();
            owner.prepared_first_key = Some(key);
            owner.prepared_first = Some(frame);
            assert_eq!(owner.airtime_balance_micros(peer), Some(0));
            assert_eq!(radio.tx_queue_len(), 0);
            if case == Case::PreparedSleep {
                control
                    .mac
                    .engine_mut()
                    .observe_rx_peer_power_state([4; 6], ApPeerPowerState::Sleeping, 3)
                    .unwrap();
                assert_eq!(
                    owner.start_prepared(&mut aggregate, &mut control, &mut hardware, &source),
                    Ok(WifiTxProgress::Complete)
                );
                assert_eq!(hardware.legacy_publications, 0);
                assert_eq!(owner.airtime_balance_micros(peer), Some(1000));
                assert_eq!(owner.buffered_unicast.len, 1);
                owner
                    .airtime
                    .as_ref()
                    .unwrap()
                    .require_selection_idle()
                    .unwrap();
                control
                    .mac
                    .engine_mut()
                    .observe_rx_peer_power_state([4; 6], ApPeerPowerState::Active, 4)
                    .unwrap();
            }
            Poll::Ready(owner.start_prepared(&mut aggregate, &mut control, &mut hardware, &source))
        } else {
            let frame = radio.try_receive_tx().unwrap();
            pin!(owner.start(&mut aggregate, &mut control, &mut hardware, frame, &source))
                .poll(&mut Context::from_waker(Waker::noop()))
        };
        if matches!(
            case,
            Case::Awake | Case::Dtim | Case::PsPollPriority | Case::DtimPriority
        ) {
            assert_eq!(result, Poll::Ready(Ok(WifiTxProgress::Complete)));
            assert_eq!(hardware.legacy_publications, 0);
            assert_eq!(
                owner.airtime_balance_micros(peer),
                None,
                "sleeping backlog owns no grant"
            );
            if matches!(case, Case::PsPollPriority | Case::DtimPriority) {
                device.transmit(packet(packets, 6, 2)).unwrap();
                let (key, frame) = owner
                    .take_scheduled_active_or_network(control.mac.engine_mut(), &source)
                    .unwrap()
                    .unwrap()
                    .expect_frame();
                owner.prepared_first_key = Some(key);
                owner.prepared_first = Some(frame);
                assert_eq!(
                    owner.airtime_balance_micros(AccessPointAirtimePeer::Unicast(
                        key.association().unwrap()
                    )),
                    Some(0)
                );
            }
            if case == Case::PsPollPriority {
                use open_esp_radio_esp32s31_wifi_ap::protocol::ApPowerSaveAction;
                use open_esp_radio_ieee80211::ap::ApPowerSaveObservation;
                let action = control
                    .mac
                    .engine_mut()
                    .observe_power_save(
                        ApPowerSaveObservation::PsPoll {
                            peer: identity.address(),
                            association_id: identity.association_id(),
                        },
                        3,
                    )
                    .unwrap();
                let ApPowerSaveAction::ReleaseOne(release) = action else {
                    panic!("PS-Poll owns a buffered release");
                };
                control
                    .pending_buffered_releases
                    .push(release)
                    .ok()
                    .unwrap();
                // The RX service delivers the protocol request through this bridge.
                assert!(owner.refresh_power_save_demand(&mut control).unwrap());
            } else if case == Case::Awake {
                control
                    .mac
                    .engine_mut()
                    .observe_rx_peer_power_state([4; 6], ApPeerPowerState::Active, 3)
                    .unwrap();
            } else {
                // The protocol owner supplies this exact successfully advertised DTIM prefix.
                control.pending_dtim_group_frames = Some(1);
            }
            result = Poll::Ready(owner.start_prepared(
                &mut aggregate,
                &mut control,
                &mut hardware,
                &source,
            ));
        }
        assert_eq!(result, Poll::Ready(Ok(WifiTxProgress::Pending)));
        assert_eq!(owner.airtime_balance_micros(peer), Some(0));
        assert_eq!(hardware.legacy_publications, 1);
        if matches!(case, Case::PsPollPriority | Case::DtimPriority) {
            let other = owner.prepared_first_key.unwrap().association().unwrap();
            assert_eq!(
                owner.airtime_balance_micros(AccessPointAirtimePeer::Unicast(other)),
                Some(0),
                "reserving the priority peer clears idle positive credit"
            );
            owner
                .airtime
                .as_ref()
                .unwrap()
                .require_selection_idle()
                .unwrap();
            assert_eq!(owner.prepared_first.as_ref().unwrap().ethernet()[14], 2);
        }

        if case == Case::DetachFailure {
            let timeout = WifiTxWake::Interrupt {
                events: open_esp_radio_esp32s31_wifi_mac::irq::EVENT_TX_TIMEOUT,
            };
            assert_eq!(
                owner.service(&mut aggregate, &mut control, &mut hardware, timeout),
                Ok(WifiTxProgress::Pending)
            );
            assert_eq!(
                pin!(control.wait_tx_deadline()).poll(&mut Context::from_waker(Waker::noop())),
                Poll::Ready(())
            );
            assert!(
                owner
                    .service(
                        &mut aggregate,
                        &mut control,
                        &mut hardware,
                        WifiTxWake::Deadline
                    )
                    .is_err()
            );
            assert_eq!(hardware.timeout_detaches, 1);
            assert_eq!(
                owner.airtime_balance_micros(peer),
                Some(0),
                "no refund without detach"
            );
            owner
                .cancel_prepared(&mut aggregate, &mut control, &source)
                .unwrap();
            assert_eq!(owner.airtime_balance_micros(peer), Some(0));
            return;
        }
        hardware.ordinary_completion = Some(MacTxCompletionObservation::new_model(0, 0));
        let result = owner.service(
            &mut aggregate,
            &mut control,
            &mut hardware,
            WifiTxWake::Interrupt {
                events: open_esp_radio_esp32s31_wifi_mac::irq::EVENT_TX_COMPLETE,
            },
        );
        if case == Case::UnknownCost {
            assert_eq!(
                result,
                Err(Esp32s31AccessPointDatapathError::Airtime(
                    AccessPointAirtimeError::UnpricedWork
                ))
            );
            assert_eq!(owner.airtime_balance_micros(peer), Some(0));
            assert_eq!(owner.unresolved_airtime_work().unwrap().1.publications, 1);
        } else {
            assert_eq!(result, Ok(WifiTxProgress::Complete));
            assert_eq!(owner.airtime_balance_micros(peer), Some(400));
        }
    });
}
