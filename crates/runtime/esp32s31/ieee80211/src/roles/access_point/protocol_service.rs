#[cfg(any(feature = "diagnostics", test))]
fn observe_ht_rx_data_frame(
    observation: &mut AccessPointControlObservation,
    signal: HtSignal,
) {
    observation.rx_ht_data_frames = observation.rx_ht_data_frames.saturating_add(1);
    if signal.aggregation {
        observation.rx_ht_mpdus_with_aggregation_bit = observation
            .rx_ht_mpdus_with_aggregation_bit
            .saturating_add(1);
    }
    match signal.ht_duplicate_mcs32_classification() {
        HtDuplicateRxClassification::Ht40(_) => {
            observation.rx_ht40_mcs32_frames = observation.rx_ht40_mcs32_frames.saturating_add(1);
        }
        HtDuplicateRxClassification::Mismatch { .. } => {
            observation.rx_ht_mcs32_width_mismatches =
                observation.rx_ht_mcs32_width_mismatches.saturating_add(1);
        }
        HtDuplicateRxClassification::NotMcs32 => {}
    }
    if signal.channel_width_mhz == 40
        && let Some(count) = observation.rx_ht40_mcs_frames.get_mut(signal.mcs as usize)
    {
        *count = count.saturating_add(1);
        let guard_interval_count = if signal.short_guard_interval {
            &mut observation.rx_ht40_short_gi_frames
        } else {
            &mut observation.rx_ht40_long_gi_frames
        };
        *guard_interval_count = guard_interval_count.saturating_add(1);
    }
}

fn ap_security_material_for_management<S>(
    link_protection: LinkProtection,
    request: Option<ApManagementRequest<'_>>,
    peer_phase: Option<ApPeerPhase>,
    source: &mut S,
) -> ([u8; 32], u64)
where
    S: FnMut() -> ([u8; 32], u64),
{
    if link_protection == LinkProtection::Ccmp
        && matches!(request, Some(ApManagementRequest::Association { .. }))
        && peer_phase == Some(ApPeerPhase::Authenticated)
    {
        source()
    } else {
        ([0; 32], 0)
    }
}

fn retain_ap_power_save_action(
    engine: &mut ApEngine<'_>,
    pending: &mut PendingApBufferedReleases,
    action: ApPowerSaveAction,
) -> Result<(), AccessPointControlError> {
    let release = match action {
        ApPowerSaveAction::ReleaseOne(release) => Some(release),
        ApPowerSaveAction::None | ApPowerSaveAction::StateChanged { .. } => None,
    };
    let Some(release) = release else {
        return Ok(());
    };
    if let Err(release) = pending.push(release) {
        engine.complete_buffered_unicast_release(release, false)?;
        return Err(AccessPointControlError::ProtocolActionCapacity);
    }
    Ok(())
}

fn observe_ap_rx_peer_activity(
    engine: &mut ApEngine<'_>,
    pending: &mut PendingApBufferedReleases,
    peer: [u8; 6],
    power_state: Option<ApPeerPowerState>,
    at: oer_time::Instant,
) -> Result<(), AccessPointControlError> {
    match power_state {
        Some(ApPeerPowerState::Active) => {
            let action =
                engine.observe_rx_peer_power_state(peer, ApPeerPowerState::Active, at)?;
            retain_ap_power_save_action(engine, pending, action)?;
        }
        Some(ApPeerPowerState::Sleeping) => {
            let action =
                engine.observe_rx_peer_power_state(peer, ApPeerPowerState::Sleeping, at)?;
            retain_ap_power_save_action(engine, pending, action)?;
        }
        None => engine.observe_peer_activity(peer, at)?,
    }
    Ok(())
}

#[inline(always)]
const fn admitted_ap_data_power_state(frame_control: u16) -> ApPeerPowerState {
    if frame_control & 0x1000 == 0 {
        ApPeerPowerState::Active
    } else {
        ApPeerPowerState::Sleeping
    }
}

/// Consume the common in-order protected-data owner without borrowing the
/// AP ordinary-TX capability. Active and parked AP roles enter this same leaf.
/// `Ok(None)` is non-mutating for the staging owner and reorder state.
#[inline(never)]
fn try_service_ap_staged_rx_direct<'storage, F, const DMA_BUFFER_SIZE: usize>(
    engine: &mut ApEngine<'_>,
    data_rx: &mut ApRxDispatcher,
    rx_reorder: &mut AccessPointRxReorder<'storage, DMA_BUFFER_SIZE>,
    pending_buffered_releases: &mut PendingApBufferedReleases,
    rx_frame: &mut [u8],
    rx_batch_used: &mut usize,
    rx_batch_offset: &mut usize,
    staged_frame: &mut Option<F>,
    now: oer_time::Instant,
    #[cfg(any(feature = "diagnostics", test))]
    observation: &mut AccessPointControlObservation,
    #[cfg(feature = "diagnostics")] delivery_observer: Option<&dyn RxNetworkDeliveryObserver>,
) -> Result<Option<AccessPointRxProtocolClass>, AccessPointControlError>
where
    F: AccessPointStagedRxFrame,
{
    let segment = staged_frame
        .as_ref()
        .expect("current AP staged frame is live")
        .segment();
    if !data_rx.may_dispatch_ordinary_pairwise(segment) {
        return Ok(None);
    }
    let Some(key) = data_rx.reorder_key(segment) else {
        return Ok(None);
    };
    let Some(_reorder_progress) = rx_reorder.try_ingest_immediate(key, now)? else {
        return Ok(None);
    };

    #[cfg(feature = "task-poll-telemetry")]
    let mut core0_ap_rx = crate::diagnostics::core0_ap_rx_cycles::Core0ApRxCycleProfile::begin();
    #[cfg(feature = "task-poll-telemetry")]
    core0_ap_rx.view_complete();

    #[cfg(any(feature = "diagnostics", test))]
    let diagnostic_frame = view_normalized_rx_frame(
        &segment,
        RxIngressConfig {
            ring_entry_limit: 1,
            csi_config: 0,
            flags: 0,
        },
    )
    .ok();
    #[cfg(any(feature = "diagnostics", test))]
    {
        observation.protected_data_frames = observation.protected_data_frames.saturating_add(1);
        if let Some(diagnostic_frame) = diagnostic_frame
            && let MacRxEvidence::HardwareObserved(rssi_dbm) = diagnostic_frame.metadata.rssi_dbm
        {
            if observation.rx_rssi_samples == 0 {
                observation.rx_rssi_min_dbm = rssi_dbm;
                observation.rx_rssi_max_dbm = rssi_dbm;
            } else {
                observation.rx_rssi_min_dbm = observation.rx_rssi_min_dbm.min(rssi_dbm);
                observation.rx_rssi_max_dbm = observation.rx_rssi_max_dbm.max(rssi_dbm);
            }
            observation.rx_rssi_samples = observation.rx_rssi_samples.saturating_add(1);
            observation.rx_rssi_sum_dbm = observation
                .rx_rssi_sum_dbm
                .saturating_add(i32::from(rssi_dbm));
        }
        if let Some(diagnostic_frame) = diagnostic_frame
            && let MacRxEvidence::HardwareObserved(phy) = diagnostic_frame.metadata.rate
            && let Some(signal) = phy.ht_signal()
        {
            observe_ht_rx_data_frame(observation, signal);
        }
        observation.rx_reorder_dispatched_mpdus = observation
            .rx_reorder_dispatched_mpdus
            .saturating_add(u32::from(_reorder_progress.dispatched));
    }

    let power_state = segment
        .buffer
        .get(oer_esp32s31_ieee80211_mac::rx::PUBLIC_HEADER_SIZE..)
        .and_then(|mpdu| mpdu.get(..2))
        .map(|bytes| admitted_ap_data_power_state(u16::from_le_bytes([bytes[0], bytes[1]])))
        .expect("ordinary AP data preflight validated its public frame control");
    let mut admitted_activity = None;
    #[cfg(any(feature = "diagnostics", test))]
    let mut produced_data = false;
    let in_place_publication = {
        let mut in_place = InPlaceAccessPointRxSink::new(segment.buffer);
        #[cfg(feature = "task-poll-telemetry")]
        let dispatch_started = crate::diagnostics::core0_rx_cycles::cycle_count();
        AccessPointProtectedFrameDispatch::dispatch_ordinary(
            data_rx,
            segment,
            |request| {
                let (admission, activity) = engine.admit_ordinary_pairwise_rx_with_activity(
                    request,
                    power_state,
                    now,
                );
                admitted_activity = Some(activity);
                admission
            },
            key.peer,
            #[cfg(any(feature = "diagnostics", test))]
            now,
            &mut in_place,
            #[cfg(any(feature = "diagnostics", test))]
            observation,
            #[cfg(any(feature = "diagnostics", test))]
            &mut produced_data,
            #[cfg(feature = "task-poll-telemetry")]
            &mut core0_ap_rx,
        );
        #[cfg(feature = "task-poll-telemetry")]
        core0_ap_rx.dispatch_complete(
            crate::diagnostics::core0_rx_cycles::cycle_count().wrapping_sub(dispatch_started),
        );
        if in_place.unsupported {
            return Err(AccessPointControlError::ReceiveBatchCapacity);
        }
        in_place.publication
    };

    if let Some(ethernet) = in_place_publication {
        #[cfg(any(feature = "diagnostics", test))]
        let raw = segment.buffer;
        #[cfg(any(feature = "diagnostics", test))]
        let payload =
            &raw[ethernet.payload_offset..ethernet.payload_offset + ethernet.payload_length];
        #[cfg(any(feature = "diagnostics", test))]
        let ethernet_frame = EthernetFrameParts {
            destination: ethernet.destination,
            source: ethernet.source,
            ether_type: ethernet.ether_type,
            payload,
        };
        #[cfg(any(feature = "diagnostics", test))]
        let protocol = ethernet_parts_protocol(ethernet_frame);
        #[cfg(feature = "diagnostics")]
        if let Some(observer) = delivery_observer {
            observer.admitted(RxNetworkDeliveryEvent::decoded(ethernet_frame, Some(raw)));
        }
        let frame = EthernetFrameParts {
            destination: ethernet.destination,
            source: ethernet.source,
            ether_type: ethernet.ether_type,
            payload: &segment.buffer
                [ethernet.payload_offset..ethernet.payload_offset + ethernet.payload_length],
        };
        let mut writer = crate::datapath::rx::ethernet::PackedEthernetWriter::new(rx_frame);
        writer
            .push(frame)
            .map_err(|_| AccessPointControlError::ReceiveBatchCapacity)?;
        *rx_batch_used = writer.used();
        *rx_batch_offset = 0;
        drop(
            staged_frame
                .take()
                .expect("direct AP publication owns the current staging frame"),
        );
        #[cfg(any(feature = "diagnostics", test))]
        {
            observation.ethernet_frames_staged =
                observation.ethernet_frames_staged.saturating_add(1);
            match protocol {
                Some(EthernetProtocol::ArpRequest) => {
                    observation.ethernet_arp_requests_staged =
                        observation.ethernet_arp_requests_staged.saturating_add(1);
                }
                Some(EthernetProtocol::Ipv4Tcp) => {
                    observation.ethernet_tcp_frames_staged =
                        observation.ethernet_tcp_frames_staged.saturating_add(1);
                }
                _ => {}
            }
        }
    }
    #[cfg(any(feature = "diagnostics", test))]
    if !produced_data {
        observation.ignored_rx_frames = observation.ignored_rx_frames.saturating_add(1);
    }
    #[cfg(feature = "task-poll-telemetry")]
    core0_ap_rx.publication_complete();

    if let Some(activity) = admitted_activity
        && let Some(action) = activity?
    {
        retain_ap_power_save_action(engine, pending_buffered_releases, action)?;
    }
    #[cfg(feature = "task-poll-telemetry")]
    core0_ap_rx.finish();
    Ok(Some(AccessPointRxProtocolClass::ProtectedData))
}

/// Consume every AP data-frame ordering case without borrowing the shared
/// ordinary-TX capability. The immediate in-order case stays on the small
/// leaf above; gaps, duplicates and releases enter the same role-local BA
/// state from active and parked AP owners.
fn try_service_ap_staged_rx_data<'storage, F, const DMA_BUFFER_SIZE: usize>(
    engine: &mut ApEngine<'_>,
    state: &mut AccessPointProtocolState<'storage, DMA_BUFFER_SIZE>,
    staged_frame: &mut Option<F>,
    now: oer_time::Instant,
    #[cfg(feature = "diagnostics")] delivery_observer: Option<&dyn RxNetworkDeliveryObserver>,
) -> Result<Option<AccessPointRxProtocolClass>, AccessPointControlError>
where
    F: AccessPointStagedRxFrame,
{
    if let Some(class) = try_service_ap_staged_rx_direct(
        engine,
        state.data_rx,
        state.rx_reorder,
        &mut state.pending_buffered_releases,
        state.rx_frame,
        &mut state.rx_batch_used,
        &mut state.rx_batch_offset,
        staged_frame,
        now,
        #[cfg(any(feature = "diagnostics", test))]
        &mut state.observer.observation,
        #[cfg(feature = "diagnostics")]
        delivery_observer,
    )? {
        return Ok(Some(class));
    }

    #[cfg(feature = "task-poll-telemetry")]
    let mut core0_ap_rx = crate::diagnostics::core0_ap_rx_cycles::Core0ApRxCycleProfile::begin();
    let segment = staged_frame
        .as_ref()
        .expect("current AP staged frame is live")
        .segment();
    let frame = match view_normalized_rx_frame(
        &segment,
        RxIngressConfig {
            ring_entry_limit: 1,
            csi_config: 0,
            flags: 0,
        },
    ) {
        Ok(frame) => frame,
        Err(_error) => {
            observe_access_point!(state, observation, {
                match _error {
                    oer_esp32s31_ieee80211_mac::rx::RxError::MicFailure => {
                        observation.rx_mic_failures = observation.rx_mic_failures.saturating_add(1);
                    }
                    oer_esp32s31_ieee80211_mac::rx::RxError::Quarantined => {
                        let duplicate_or_stale = state
                            .data_rx
                            .reorder_key(segment)
                            .is_some_and(|key| state.rx_reorder.is_duplicate_or_stale(key));
                        if duplicate_or_stale {
                            observation.protected_data_duplicates =
                                observation.protected_data_duplicates.saturating_add(1);
                        } else {
                            observation.rx_quarantined_frames =
                                observation.rx_quarantined_frames.saturating_add(1);
                        }
                    }
                    _ => {
                        observation.rx_view_rejected =
                            observation.rx_view_rejected.saturating_add(1);
                    }
                }
                observation.ignored_rx_frames = observation.ignored_rx_frames.saturating_add(1);
            });
            return Ok(Some(AccessPointRxProtocolClass::Rejected));
        }
    };
    let frame_control = u16::from_le_bytes([frame.mpdu[0], frame.mpdu[1]]);
    let data_frame = frame_control & 0x000c == 0x0008;
    let protected = frame_control & 0x4000 != 0;
    if !data_frame || (engine.link_protection() != LinkProtection::Open && !protected) {
        return Ok(None);
    }

    #[cfg(feature = "task-poll-telemetry")]
    core0_ap_rx.view_complete();
    observe_access_point!(state, observation, {
        observation.protected_data_frames = observation.protected_data_frames.saturating_add(1);
        if let MacRxEvidence::HardwareObserved(rssi_dbm) = frame.metadata.rssi_dbm {
            if observation.rx_rssi_samples == 0 {
                observation.rx_rssi_min_dbm = rssi_dbm;
                observation.rx_rssi_max_dbm = rssi_dbm;
            } else {
                observation.rx_rssi_min_dbm = observation.rx_rssi_min_dbm.min(rssi_dbm);
                observation.rx_rssi_max_dbm = observation.rx_rssi_max_dbm.max(rssi_dbm);
            }
            observation.rx_rssi_samples = observation.rx_rssi_samples.saturating_add(1);
            observation.rx_rssi_sum_dbm = observation
                .rx_rssi_sum_dbm
                .saturating_add(i32::from(rssi_dbm));
        }
        if let MacRxEvidence::HardwareObserved(phy) = frame.metadata.rate
            && let Some(signal) = phy.ht_signal()
        {
            observe_ht_rx_data_frame(observation, signal);
        }
    });

    let power_state = Some(admitted_ap_data_power_state(frame_control));
    let ampdu_contained = matches!(
        frame.metadata.ampdu,
        MacRxEvidence::HardwareObserved(true) | MacRxEvidence::ProtocolValidated(true)
    );
    let ampdu_baseband_format = if ampdu_contained {
        match frame.metadata.rate {
            MacRxEvidence::HardwareObserved(phy) => Some(phy.baseband_format().raw()),
            _ => None,
        }
    } else {
        None
    };
    let mut activity_peer = None;
    #[cfg(feature = "task-poll-telemetry")]
    let mut protected_dispatch_cycles = 0_u32;
    let (reorder_progress, batch_used, batch_exhausted, in_place_publication, produced_data) = {
        let data_rx = &mut state.data_rx;
        #[cfg(any(feature = "diagnostics", test))]
        let report = &mut state.observer.observation;
        let mut deferred = DeferredAccessPointRxSink::new(state.rx_frame);
        let mut in_place = InPlaceAccessPointRxSink::new(segment.buffer);
        let mut produced_data = false;
        #[cfg(feature = "task-poll-telemetry")]
        let reorder_key_started = crate::diagnostics::core0_rx_cycles::cycle_count();
        let key = data_rx.reorder_key(segment);
        let peer = key.map(|key| key.peer).or_else(|| {
            frame
                .mpdu
                .get(10..16)
                .and_then(|bytes| <[u8; 6]>::try_from(bytes).ok())
        });
        #[cfg(feature = "task-poll-telemetry")]
        core0_ap_rx.record_reorder_key(
            crate::diagnostics::core0_rx_cycles::cycle_count().wrapping_sub(reorder_key_started),
        );
        let current_buffer = segment.buffer.as_ptr();
        let qos_control_offset = 24
            + if frame_control & 0x0300 == 0x0300 {
                6
            } else {
                0
            };
        let current_is_amsdu = frame_control & 0x0080 != 0
            && frame
                .mpdu
                .get(qos_control_offset)
                .is_some_and(|control| control & 0x80 != 0);
        let reorder_progress = {
            let mut dispatch = |ordered: oer_esp32s31_ieee80211_mac::rx::RxSegment<'_>| {
                #[cfg(feature = "task-poll-telemetry")]
                let dispatch_started = crate::diagnostics::core0_rx_cycles::cycle_count();
                AccessPointProtectedFrameDispatch::dispatch(
                    data_rx,
                    ordered,
                    &mut *engine,
                    peer,
                    current_buffer as usize,
                    current_is_amsdu,
                    now,
                    &mut deferred,
                    &mut in_place,
                    #[cfg(any(feature = "diagnostics", test))]
                    report,
                    &mut activity_peer,
                    &mut produced_data,
                    #[cfg(feature = "task-poll-telemetry")]
                    &mut core0_ap_rx,
                );
                #[cfg(feature = "task-poll-telemetry")]
                {
                    protected_dispatch_cycles = protected_dispatch_cycles.wrapping_add(
                        crate::diagnostics::core0_rx_cycles::cycle_count()
                            .wrapping_sub(dispatch_started),
                    );
                }
            };
            if let Some(key) = key {
                state.rx_reorder.ingest(
                    state.rx_reorder_storage,
                    segment,
                    key,
                    ampdu_baseband_format,
                    now,
                    &mut dispatch,
                )
            } else {
                dispatch(segment);
                Ok(Default::default())
            }
        }?;
        (
            reorder_progress,
            deferred.used(),
            deferred.exhausted || in_place.unsupported,
            in_place.publication,
            produced_data,
        )
    };
    #[cfg(feature = "task-poll-telemetry")]
    core0_ap_rx.dispatch_complete(protected_dispatch_cycles);
    #[cfg(feature = "task-poll-telemetry")]
    crate::diagnostics::core0_ap_rx_cycles::CORE0_AP_RX_CYCLES.record_ingress_path(
        false,
        false,
        false,
        reorder_progress.buffered,
    );

    if let Some(reset) = reorder_progress.hardware_window_reset {
        observe_access_point!(state, observation, {
            observation.rx_reorder_hardware_window_resets = observation
                .rx_reorder_hardware_window_resets
                .saturating_add(1);
        });
        let agreement = state.rx_block_ack.snapshots_for(MacInterface::AccessPoint)
            [usize::from(reset.hardware_index)]
        .expect("AP reorder reset belongs to one live AP BlockAck agreement");
        state
            .protocol_actions
            .publisher()
            .try_publish(AccessPointProtocolAction::Hardware(
                AccessPointHardwareAction::ResetRxBlockAckWindow {
                    hardware_index: reset.hardware_index,
                    tid: agreement.tid,
                    starting_sequence: reset.starting_sequence,
                    window: RX_BLOCK_ACK_MAX_WINDOW,
                },
            ))
            .map_err(|_| AccessPointControlError::ProtocolActionCapacity)?;
    }
    observe_access_point!(state, observation, {
        if reorder_progress.duplicate {
            observation.protected_data_duplicates =
                observation.protected_data_duplicates.saturating_add(1);
        }
        if reorder_progress.buffered {
            observation.rx_reorder_buffered_mpdus =
                observation.rx_reorder_buffered_mpdus.saturating_add(1);
        }
        observation.rx_reorder_dispatched_mpdus = observation
            .rx_reorder_dispatched_mpdus
            .saturating_add(u32::from(reorder_progress.dispatched));
        if let Some(reason) = reorder_progress.dropped {
            let reason = match reason {
                rx_reorder::AccessPointRxReorderDrop::StorageExhausted => {
                    AccessPointRxRejectionReason::ReorderStorageExhausted
                }
                rx_reorder::AccessPointRxReorderDrop::FrameTooLong => {
                    AccessPointRxRejectionReason::ReorderFrameTooLong
                }
            };
            observation.record_rx_rejection(reason, segment, now.as_micros());
            observation.protected_data_protocol_rejected = observation
                .protected_data_protocol_rejected
                .saturating_add(1);
        }
    });
    if batch_exhausted {
        observe_access_point!(state, observation, {
            observation.protected_data_protocol_rejected = observation
                .protected_data_protocol_rejected
                .saturating_add(1);
        });
        return Err(AccessPointControlError::ReceiveBatchCapacity);
    }
    let mut batch_used = batch_used;
    if let Some(ethernet) = in_place_publication {
        #[cfg(any(feature = "diagnostics", test))]
        let raw = segment.buffer;
        #[cfg(any(feature = "diagnostics", test))]
        let payload =
            &raw[ethernet.payload_offset..ethernet.payload_offset + ethernet.payload_length];
        #[cfg(any(feature = "diagnostics", test))]
        let ethernet_frame = EthernetFrameParts {
            destination: ethernet.destination,
            source: ethernet.source,
            ether_type: ethernet.ether_type,
            payload,
        };
        #[cfg(any(feature = "diagnostics", test))]
        let protocol = ethernet_parts_protocol(ethernet_frame);
        #[cfg(feature = "diagnostics")]
        if let Some(observer) = delivery_observer {
            observer.admitted(RxNetworkDeliveryEvent::decoded(ethernet_frame, Some(raw)));
        }
        let frame = EthernetFrameParts {
            destination: ethernet.destination,
            source: ethernet.source,
            ether_type: ethernet.ether_type,
            payload: &segment.buffer
                [ethernet.payload_offset..ethernet.payload_offset + ethernet.payload_length],
        };
        let mut writer =
            crate::datapath::rx::ethernet::PackedEthernetWriter::resume(state.rx_frame, batch_used)
                .map_err(|_| AccessPointControlError::ReceiveBatchCapacity)?;
        writer
            .push(frame)
            .map_err(|_| AccessPointControlError::ReceiveBatchCapacity)?;
        batch_used = writer.used();
        drop(
            staged_frame
                .take()
                .expect("in-place AP publication owns the current staging frame"),
        );
        observe_access_point!(state, observation, {
            observation.ethernet_frames_staged =
                observation.ethernet_frames_staged.saturating_add(1);
            match protocol {
                Some(EthernetProtocol::ArpRequest) => {
                    observation.ethernet_arp_requests_staged =
                        observation.ethernet_arp_requests_staged.saturating_add(1);
                }
                Some(EthernetProtocol::Ipv4Tcp) => {
                    observation.ethernet_tcp_frames_staged =
                        observation.ethernet_tcp_frames_staged.saturating_add(1);
                }
                _ => {}
            }
        });
    }
    if batch_used != 0 {
        state.rx_batch_used = batch_used;
        state.rx_batch_offset = 0;
    }
    observe_access_point!(state, observation, {
        if !produced_data {
            observation.ignored_rx_frames = observation.ignored_rx_frames.saturating_add(1);
        }
    });
    #[cfg(not(any(feature = "diagnostics", test)))]
    let _ = produced_data;
    #[cfg(feature = "task-poll-telemetry")]
    core0_ap_rx.publication_complete();

    if let Some(peer) = activity_peer {
        observe_ap_rx_peer_activity(
            engine,
            &mut state.pending_buffered_releases,
            peer,
            power_state,
            now,
        )?;
    }
    #[cfg(feature = "task-poll-telemetry")]
    core0_ap_rx.finish();
    Ok(Some(AccessPointRxProtocolClass::ProtectedData))
}

#[cfg(test)]
#[path = "protocol_service/ht_rx_observation_tests.rs"]
mod ht_rx_observation_tests;

