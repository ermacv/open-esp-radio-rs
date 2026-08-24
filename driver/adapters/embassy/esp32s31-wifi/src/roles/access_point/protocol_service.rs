
#[cfg(any(feature = "diagnostics", test))]
fn observe_ht_rx_data_frame(
    observation: &mut Esp32s31AccessPointControlObservation,
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
            observation.rx_ht40_mcs32_frames =
                observation.rx_ht40_mcs32_frames.saturating_add(1);
        }
        HtDuplicateRxClassification::Mismatch { .. } => {
            observation.rx_ht_mcs32_width_mismatches = observation
                .rx_ht_mcs32_width_mismatches
                .saturating_add(1);
        }
        HtDuplicateRxClassification::NotMcs32 => {}
    }
    if signal.channel_width_mhz == 40
        && let Some(count) = observation.rx_ht40_mcs_frames.get_mut(signal.mcs as usize)
    {
        *count = count.saturating_add(1);
    }
}

#[cfg(test)]
mod ht_rx_observation_tests {
    use super::*;

    #[test]
    fn ap_observation_separates_valid_ht40_mcs32_from_width_mismatch() {
        let mut observation = Esp32s31AccessPointControlObservation::default();
        observe_ht_rx_data_frame(
            &mut observation,
            HtSignal {
                mcs: 32,
                channel_width_mhz: 40,
                aggregation: false,
                short_guard_interval: false,
            },
        );
        observe_ht_rx_data_frame(
            &mut observation,
            HtSignal {
                mcs: 32,
                channel_width_mhz: 20,
                aggregation: true,
                short_guard_interval: false,
            },
        );
        observe_ht_rx_data_frame(
            &mut observation,
            HtSignal {
                mcs: 7,
                channel_width_mhz: 40,
                aggregation: true,
                short_guard_interval: true,
            },
        );

        assert_eq!(observation.rx_ht_data_frames, 3);
        assert_eq!(observation.rx_ht_mpdus_with_aggregation_bit, 2);
        assert_eq!(observation.rx_ht40_mcs32_frames, 1);
        assert_eq!(observation.rx_ht_mcs32_width_mismatches, 1);
        assert_eq!(observation.rx_ht40_mcs_frames[7], 1);
    }
}

impl<'storage, 'beacon, 'slot, P, E, T, const DMA_BUFFER_SIZE: usize, const TX_BUFFER_SIZE: usize>
    Esp32s31AccessPointProtocolProcessor<
        'storage,
        'beacon,
        'slot,
        P,
        E,
        T,
        DMA_BUFFER_SIZE,
        TX_BUFFER_SIZE,
    >
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
{
    /// Construct AP protocol/control state without binding a DMA producer or
    /// private RX queue. Same-channel STA+AP composition owns those physical
    /// resources at its common DATAPATH boundary.
    pub fn new(
        mac: Esp32s31ApMac<'beacon, 'slot, P, E, T, TX_BUFFER_SIZE>,
        rx_frame: &'storage mut [u8],
        tx_frame: &'storage mut [u8],
        data_rx: &'storage mut Esp32s31ApRxDispatcher,
        rx_block_ack: &'storage Esp32s31StaApRxBlockAck,
        rx_reorder: &'storage mut Esp32s31AccessPointRxReorder<'storage, DMA_BUFFER_SIZE>,
        rx_reorder_storage: &'storage RxReorderFrameStorage<
            DMA_BUFFER_SIZE,
            RX_REORDER_BACKING_SLOT_COUNT,
        >,
        #[cfg(feature = "diagnostics")]
        observation_storage: &'static mut AccessPointObservationStorage,
    ) -> Self {
        let access_point = mac.engine().service_address();
        let security = mac.engine().security_mode();
        data_rx.reset(Esp32s31ApRxConfig {
            access_point,
            ingress: RxIngressConfig {
                ring_entry_limit: 1,
                csi_config: 0,
                flags: 0,
            },
            security,
        });
        rx_block_ack
            .prepare_interface(MacInterface::AccessPoint)
            .expect("AP start requires its previous RX BlockAck epoch to be quiescent");
        let discarded_reorder_frames = rx_reorder.discard_all();
        debug_assert_eq!(discarded_reorder_frames, 0);
        #[cfg(feature = "diagnostics")]
        observation_storage.reset();
        Self {
            mac,
            rx_frame,
            tx_frame,
            data_rx,
            rx_block_ack,
            rx_reorder,
            rx_reorder_storage,
            rx_addba_in_flight: None,
            protocol_actions: Esp32s31AccessPointProtocolMailbox::new(),
            pending_buffered_releases: PendingApBufferedReleases::new(),
            pending_dtim_group_frames: None,
            rx_batch_used: 0,
            rx_batch_offset: 0,
            serviced_rx_frames: 0,
            serviced_rx_descriptors: 0,
            last_terminal_tx_succeeded: None,
            #[cfg(feature = "diagnostics")]
            observer: observation_storage,
            #[cfg(all(test, not(feature = "diagnostics")))]
            observer: AccessPointObservationStorage::default(),
            #[cfg(any(feature = "diagnostics", test))]
            terminal_observer: None,
        }
    }

    /// Attach the non-owning terminal observer for this AP role epoch.
    #[cfg(any(feature = "diagnostics", test))]
    pub fn with_terminal_observer(
        mut self,
        observer: &'static dyn AccessPointTerminalObserver,
    ) -> Self {
        self.terminal_observer = Some(observer);
        self
    }

    /// Remove the unique ordinary-TX capability at an idle transaction edge.
    /// A pending publication fails closed and returns the complete processor
    /// unchanged, so the caller cannot lose either hardware or protocol state.
    #[allow(clippy::result_large_err, clippy::type_complexity)]
    pub fn try_park(
        self,
    ) -> Result<
        (
            WifiTxResources<'slot, P, E, T, TX_BUFFER_SIZE>,
            Esp32s31AccessPointProtocolProcessorParked<'storage, 'beacon, DMA_BUFFER_SIZE>,
        ),
        Self,
    > {
        let Self {
            mac,
            rx_frame,
            tx_frame,
            data_rx,
            rx_block_ack,
            rx_reorder,
            rx_reorder_storage,
            rx_addba_in_flight,
            protocol_actions,
            pending_buffered_releases,
            pending_dtim_group_frames,
            rx_batch_used,
            rx_batch_offset,
            serviced_rx_frames,
            serviced_rx_descriptors,
            last_terminal_tx_succeeded,
            #[cfg(any(feature = "diagnostics", test))]
            observer,
            #[cfg(any(feature = "diagnostics", test))]
            terminal_observer,
        } = self;
        match mac.try_park() {
            Ok((resources, mac)) => Ok((
                resources,
                Esp32s31AccessPointProtocolProcessorParked {
                    mac,
                    rx_frame,
                    tx_frame,
                    data_rx,
                    rx_block_ack,
                    rx_reorder,
                    rx_reorder_storage,
                    rx_addba_in_flight,
                    protocol_actions,
                    pending_buffered_releases,
                    pending_dtim_group_frames,
                    rx_batch_used,
                    rx_batch_offset,
                    serviced_rx_frames,
                    serviced_rx_descriptors,
                    last_terminal_tx_succeeded,
                    #[cfg(any(feature = "diagnostics", test))]
                    observer,
                    #[cfg(any(feature = "diagnostics", test))]
                    terminal_observer,
                },
            )),
            Err(mac) => Err(Self {
                mac,
                rx_frame,
                tx_frame,
                data_rx,
                rx_block_ack,
                rx_reorder,
                rx_reorder_storage,
                rx_addba_in_flight,
                protocol_actions,
                pending_buffered_releases,
                pending_dtim_group_frames,
                rx_batch_used,
                rx_batch_offset,
                serviced_rx_frames,
                serviced_rx_descriptors,
                last_terminal_tx_succeeded,
                #[cfg(any(feature = "diagnostics", test))]
                observer,
                #[cfg(any(feature = "diagnostics", test))]
                terminal_observer,
            }),
        }
    }

    /// Reconstitute the AP processor from its exact role state and the sole
    /// ordinary-TX capability owned by the paired physical transaction.
    pub fn resume(
        resources: WifiTxResources<'slot, P, E, T, TX_BUFFER_SIZE>,
        parked: Esp32s31AccessPointProtocolProcessorParked<'storage, 'beacon, DMA_BUFFER_SIZE>,
    ) -> Self {
        let Esp32s31AccessPointProtocolProcessorParked {
            mac,
            rx_frame,
            tx_frame,
            data_rx,
            rx_block_ack,
            rx_reorder,
            rx_reorder_storage,
            rx_addba_in_flight,
            protocol_actions,
            pending_buffered_releases,
            pending_dtim_group_frames,
            rx_batch_used,
            rx_batch_offset,
            serviced_rx_frames,
            serviced_rx_descriptors,
            last_terminal_tx_succeeded,
            #[cfg(any(feature = "diagnostics", test))]
            observer,
            #[cfg(any(feature = "diagnostics", test))]
            terminal_observer,
        } = parked;
        Self {
            mac: Esp32s31ApMac::resume(resources, mac),
            rx_frame,
            tx_frame,
            data_rx,
            rx_block_ack,
            rx_reorder,
            rx_reorder_storage,
            rx_addba_in_flight,
            protocol_actions,
            pending_buffered_releases,
            pending_dtim_group_frames,
            rx_batch_used,
            rx_batch_offset,
            serviced_rx_frames,
            serviced_rx_descriptors,
            last_terminal_tx_succeeded,
            #[cfg(any(feature = "diagnostics", test))]
            observer,
            #[cfg(any(feature = "diagnostics", test))]
            terminal_observer,
        }
    }

    /// Consume one frame already classified for the AP by the common physical
    /// RX dispatcher.
    ///
    /// This path owns no DMA operation and never reads an AP-private queue. It
    /// is the protocol boundary used by same-channel STA+AP composition. If
    /// ordering or an active hardware TX prevents safe processing, the exact
    /// staging lease is returned instead of copied or dropped.
    pub fn service_routed_rx<H, F, Q>(
        &mut self,
        hardware: &mut H,
        frame: F,
        authenticator_nonce: [u8; 32],
        initial_replay_counter: u64,
        now_micros: u64,
        publish_shared_rx: &mut Q,
        #[cfg(feature = "diagnostics")] delivery_observer: Option<&dyn RxNetworkDeliveryObserver>,
    ) -> Result<
        crate::roles::concurrent::Esp32s31RoutedRxDisposition<F>,
        Esp32s31AccessPointControlError,
    >
    where
        H: TxHardware + Esp32s31ApRuntimeHardware + RxBlockAckHardware,
        F: AccessPointStagedRxFrame,
        Q: FnMut(u8),
    {
        let tx_pending = self.mac.tx_pending();
        self.apply_protocol_actions(hardware)?;
        if self.rx_batch_pending() || self.service_rx_reorder_expiry(now_micros)? {
            return Ok(crate::roles::concurrent::Esp32s31RoutedRxDisposition::Deferred(frame));
        }

        if tx_pending && !rx_pipeline::is_protected_data(frame.segment()) {
            return Ok(crate::roles::concurrent::Esp32s31RoutedRxDisposition::Deferred(frame));
        }

        self.serviced_rx_frames = self.serviced_rx_frames.saturating_add(1);
        #[cfg(feature = "diagnostics")]
        let protocol_started = Instant::now().as_micros();
        let protocol_class = self.service_staged_rx(
            rx_protocol_consumer_has_hardware(tx_pending).then_some(hardware),
            frame,
            AccessPointRxPublication::OwnedNetworkPool,
            authenticator_nonce,
            initial_replay_counter,
            now_micros,
            publish_shared_rx,
            #[cfg(feature = "diagnostics")]
            delivery_observer,
        )?;
        self.apply_protocol_actions(hardware)?;

        #[cfg(not(feature = "diagnostics"))]
        let _ = protocol_class;
        #[cfg(feature = "diagnostics")]
        self.observe_rx_protocol_service(
            protocol_class,
            Instant::now().as_micros().saturating_sub(protocol_started),
        );
        Ok(crate::roles::concurrent::Esp32s31RoutedRxDisposition::Processed)
    }

    /// Consume only protected data while another transaction owns the
    /// physical TX domain.
    ///
    /// The frame parser may update role-local reorder/report state and append
    /// value-only mailbox actions. It cannot borrow MMIO or publish a frame;
    /// management and EAPOL owners are returned unchanged for the first idle
    /// transaction boundary.
    pub fn service_routed_rx_during_tx<H, F, Q>(
        &mut self,
        frame: F,
        authenticator_nonce: [u8; 32],
        initial_replay_counter: u64,
        now_micros: u64,
        publish_shared_rx: &mut Q,
        #[cfg(feature = "diagnostics")] delivery_observer: Option<&dyn RxNetworkDeliveryObserver>,
    ) -> Result<
        crate::roles::concurrent::Esp32s31RoutedRxDisposition<F>,
        Esp32s31AccessPointControlError,
    >
    where
        H: TxHardware + Esp32s31ApRuntimeHardware + RxBlockAckHardware,
        F: AccessPointStagedRxFrame,
        Q: FnMut(u8),
    {
        if self.rx_batch_pending() || !rx_pipeline::is_protected_data(frame.segment()) {
            return Ok(crate::roles::concurrent::Esp32s31RoutedRxDisposition::Deferred(frame));
        }
        // Do not consume the affine staging lease unless every value-only
        // action this frame can produce has a slot. A long physical TX may
        // admit several DMA/protocol turns before hardware can drain the
        // mailbox; the exact ordered head remains queued instead of turning
        // bounded backpressure into a role fault.
        if self.protocol_actions.remaining_capacity() < AP_PROTOCOL_ACTIONS_PER_RX_FRAME {
            return Ok(crate::roles::concurrent::Esp32s31RoutedRxDisposition::Deferred(frame));
        }
        if self
            .rx_reorder
            .next_deadline()
            .is_some_and(|deadline| deadline <= now_micros)
        {
            return Ok(crate::roles::concurrent::Esp32s31RoutedRxDisposition::Deferred(frame));
        }

        self.serviced_rx_frames = self.serviced_rx_frames.saturating_add(1);
        #[cfg(feature = "diagnostics")]
        let protocol_started = Instant::now().as_micros();
        let protocol_class = self.service_staged_rx::<H, _, _>(
            None,
            frame,
            AccessPointRxPublication::OwnedNetworkPool,
            authenticator_nonce,
            initial_replay_counter,
            now_micros,
            publish_shared_rx,
            #[cfg(feature = "diagnostics")]
            delivery_observer,
        )?;
        #[cfg(not(feature = "diagnostics"))]
        let _ = protocol_class;
        #[cfg(feature = "diagnostics")]
        self.observe_rx_protocol_service(
            protocol_class,
            Instant::now().as_micros().saturating_sub(protocol_started),
        );
        Ok(crate::roles::concurrent::Esp32s31RoutedRxDisposition::Processed)
    }

    /// Execute value-only RX actions after the physical transaction owner has
    /// returned. This is the sole paired-role mailbox drain edge.
    pub fn apply_pending_protocol_actions<H>(
        &mut self,
        hardware: &mut H,
    ) -> Result<(), Esp32s31AccessPointControlError>
    where
        H: RxBlockAckHardware,
    {
        self.apply_protocol_actions(hardware)
    }

    fn retain_power_save_action(
        &mut self,
        action: ApPowerSaveAction,
    ) -> Result<(), Esp32s31AccessPointControlError> {
        let release = match action {
            ApPowerSaveAction::ReleaseOne(release) => Some(release),
            ApPowerSaveAction::StateChanged {
                peer,
                state: ApPeerPowerState::Active,
                buffered_frames,
            } if buffered_frames != 0 => {
                if self
                    .mac
                    .engine()
                    .peer_status(peer)
                    .is_some_and(|status| status.buffered_release_in_flight)
                {
                    // A PS-Poll already owns the oldest frame. Its terminal
                    // completion will observe the peer as Active and drain
                    // the remaining queue without manufacturing a second
                    // reservation for this transition.
                    None
                } else {
                    self.mac
                        .engine_mut()
                        .begin_buffered_unicast_release(peer)?
                }
            }
            ApPowerSaveAction::None | ApPowerSaveAction::StateChanged { .. } => None,
        };
        let Some(release) = release else {
            return Ok(());
        };
        if let Err(release) = self.pending_buffered_releases.push(release) {
            self.mac
                .engine_mut()
                .complete_buffered_unicast_release(release, false)?;
            return Err(Esp32s31AccessPointControlError::ProtocolActionCapacity);
        }
        Ok(())
    }

    pub(super) fn take_pending_buffered_release(
        &mut self,
    ) -> Option<ApBufferedUnicastRelease> {
        self.pending_buffered_releases.pop()
    }

    pub(super) fn rollback_pending_buffered_releases(
        &mut self,
    ) -> Result<(), Esp32s31AccessPointControlError> {
        while let Some(release) = self.pending_buffered_releases.pop() {
            self.mac
                .engine_mut()
                .complete_buffered_unicast_release(release, false)?;
        }
        Ok(())
    }

    #[cfg(feature = "diagnostics")]
    fn observe_rx_protocol_service(
        &mut self,
        protocol_class: AccessPointRxProtocolClass,
        elapsed: u64,
    ) {
        self.observer.observation.maximum_rx_protocol_service_micros = self
            .observer
            .observation
            .maximum_rx_protocol_service_micros
            .max(u32::try_from(elapsed).unwrap_or(u32::MAX));
        let elapsed = u32::try_from(elapsed).unwrap_or(u32::MAX);
        let class_maximum = match protocol_class {
            AccessPointRxProtocolClass::ProtectedData => {
                self.observer
                    .observation
                    .total_rx_protected_data_service_micros = self
                    .observer
                    .observation
                    .total_rx_protected_data_service_micros
                    .saturating_add(elapsed);
                Some(
                    &mut self
                        .observer
                        .observation
                        .maximum_rx_protected_data_service_micros,
                )
            }
            AccessPointRxProtocolClass::Management => Some(
                &mut self
                    .observer
                    .observation
                    .maximum_rx_management_service_micros,
            ),
            AccessPointRxProtocolClass::Eapol => {
                Some(&mut self.observer.observation.maximum_rx_eapol_service_micros)
            }
            AccessPointRxProtocolClass::Other | AccessPointRxProtocolClass::Rejected => None,
        };
        if let Some(class_maximum) = class_maximum {
            *class_maximum = (*class_maximum).max(elapsed);
        }
    }

    /// Consume one staged AP RX owner on the protocol hot path.
    ///
    /// Saturated AP RX keeps this routine resident for most of the radio-task
    /// budget.  The S31 PSRAM-code profile therefore places the routine in the
    /// semantic hot-text class; the board linker decides whether that class is
    /// backed by internal executable SRAM.  This does not make the protocol
    /// routine interrupt-safe and does not change its ownership semantics.
    #[cfg_attr(
        target_arch = "riscv32",
        unsafe(link_section = ".hot.text.open_radio_ap_rx")
    )]
    #[inline(never)]
    fn service_staged_rx<H, F, Q>(
        &mut self,
        mut hardware: Option<&mut H>,
        staged_frame: F,
        publication: AccessPointRxPublication,
        authenticator_nonce: [u8; 32],
        initial_replay_counter: u64,
        now_micros: u64,
        publish_shared_rx: &mut Q,
        #[cfg(feature = "diagnostics")] delivery_observer: Option<&dyn RxNetworkDeliveryObserver>,
    ) -> Result<AccessPointRxProtocolClass, Esp32s31AccessPointControlError>
    where
        H: TxHardware + Esp32s31ApRuntimeHardware + RxBlockAckHardware,
        F: AccessPointStagedRxFrame,
        Q: FnMut(u8),
    {
        let mut staged_frame = Some(staged_frame);
        let segment = staged_frame
            .as_ref()
            .expect("current AP staged frame is live")
            .segment();
        let mut activity_peer = None;
        let mut batch_exhausted = false;
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
                observe_access_point!(self, observation, {
                    match _error {
                        open_esp_radio_esp32s31_wifi_mac::rx::RxError::MicFailure => {
                            observation.rx_mic_failures =
                                observation.rx_mic_failures.saturating_add(1);
                        }
                        open_esp_radio_esp32s31_wifi_mac::rx::RxError::Quarantined => {
                            let duplicate_or_stale = self
                                .data_rx
                                .reorder_key(segment)
                                .is_some_and(|key| self.rx_reorder.is_duplicate_or_stale(key));
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
                return Ok(AccessPointRxProtocolClass::Rejected);
            }
        };
        let frame_control = u16::from_le_bytes([frame.mpdu[0], frame.mpdu[1]]);
        let power_save_observation = observe_ap_power_save_for_access_point(
            frame.mpdu,
            self.mac.engine().service_address(),
        );
        let null_data_power_save_observation =
            observe_ap_null_data_power_save_for_access_point(
                frame.mpdu,
                self.mac.engine().service_address(),
            );
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
        let security_mode = self.mac.engine().security_mode();
        let data_frame = frame_control & 0x000c == 0x0008;
        let protected = frame_control & 0x4000 != 0;
        let protocol_class = if data_frame
            && (security_mode == WifiSecurityMode::Open || protected)
        {
            observe_access_point!(self, observation, {
                observation.protected_data_frames =
                    observation.protected_data_frames.saturating_add(1);
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
            let (
                reorder_progress,
                batch_used,
                current_batch_exhausted,
                in_place_publication,
                produced_data,
            ) = {
                let processor = &mut *self;
                let mac = &mut processor.mac;
                let data_rx = &mut processor.data_rx;
                #[cfg(any(feature = "diagnostics", test))]
                let report = &mut processor.observer.observation;
                let mut deferred = DeferredAccessPointRxSink::new(processor.rx_frame);
                let mut in_place = InPlaceAccessPointRxSink::new(segment.buffer);
                let mut produced_data = false;
                let key = data_rx.reorder_key(segment);
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
                    let mut dispatch =
                        |ordered: open_esp_radio_esp32s31_wifi_mac::rx::RxSegment<'_>| {
                            AccessPointProtectedFrameDispatch::dispatch(
                                data_rx,
                                ordered,
                                |request| mac.engine_mut().admit_rx_data(request),
                                publication,
                                current_buffer as usize,
                                current_is_amsdu,
                                now_micros,
                                &mut deferred,
                                &mut in_place,
                                #[cfg(any(feature = "diagnostics", test))]
                                report,
                                &mut activity_peer,
                                &mut produced_data,
                            );
                        };
                    if let Some(key) = key {
                        processor.rx_reorder.ingest(
                            processor.rx_reorder_storage,
                            segment,
                            key,
                            ampdu_baseband_format,
                            now_micros,
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
            if let Some(reset) = reorder_progress.hardware_window_reset {
                observe_access_point!(self, observation, {
                    observation.rx_reorder_hardware_window_resets = observation
                        .rx_reorder_hardware_window_resets
                        .saturating_add(1);
                });
                let agreement = self.rx_block_ack.snapshots_for(MacInterface::AccessPoint)
                    [usize::from(reset.hardware_index)]
                .expect("AP reorder reset belongs to one live AP BlockAck agreement");
                self.protocol_actions
                    .publisher()
                    .try_publish(Esp32s31AccessPointProtocolAction::Hardware(
                        Esp32s31AccessPointHardwareAction::ResetRxBlockAckWindow {
                            hardware_index: reset.hardware_index,
                            tid: agreement.tid,
                            starting_sequence: reset.starting_sequence,
                            window: RX_BLOCK_ACK_MAX_WINDOW,
                        },
                    ))
                    .map_err(|_| Esp32s31AccessPointControlError::ProtocolActionCapacity)?;
            }
            observe_access_point!(self, observation, {
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
                if reorder_progress.dropped {
                    observation.protected_data_protocol_rejected = observation
                        .protected_data_protocol_rejected
                        .saturating_add(1);
                }
            });
            batch_exhausted = current_batch_exhausted;
            // Protocol parsing has released all frame and scratch borrows.
            // Only the radio owner now translates the value-only request.
            if let Some(hardware) = hardware.as_deref_mut() {
                self.apply_protocol_actions(hardware)?;
            }
            if batch_used != 0 {
                self.rx_batch_used = batch_used;
                self.rx_batch_offset = 0;
            }
            if let Some(ethernet) = in_place_publication {
                #[cfg(any(feature = "diagnostics", test))]
                let raw = segment.buffer;
                #[cfg(any(feature = "diagnostics", test))]
                let payload = &raw
                    [ethernet.payload_offset..ethernet.payload_offset + ethernet.payload_length];
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
                let current = staged_frame
                    .take()
                    .expect("in-place AP publication owns the current staging frame");
                let index = current
                    .publish_ethernet_in_place(ethernet)
                    .map_err(|_| Esp32s31AccessPointControlError::ReceiveBatchCapacity)?;
                publish_shared_rx(index);
                observe_access_point!(self, observation, {
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
            if !produced_data {
                observe_access_point!(self, observation, {
                    observation.ignored_rx_frames = observation.ignored_rx_frames.saturating_add(1);
                });
            }
            AccessPointRxProtocolClass::ProtectedData
        } else if frame_control & 0x000c == 0 {
            let hardware = hardware
                .as_deref_mut()
                .ok_or(Esp32s31AccessPointControlError::ProtocolFrameRequiresHardware)?;
            if self.service_management(
                hardware,
                frame.mpdu,
                authenticator_nonce,
                initial_replay_counter,
                now_micros,
            )? {
                observe_access_point!(self, observation, {
                    observation.control_frames_staged =
                        observation.control_frames_staged.saturating_add(1);
                });
            }
            AccessPointRxProtocolClass::Management
        } else if data_frame {
            let hardware = hardware
                .as_deref_mut()
                .ok_or(Esp32s31AccessPointControlError::ProtocolFrameRequiresHardware)?;
            if self.service_eapol(hardware, frame.mpdu, now_micros)? {
                AccessPointRxProtocolClass::Eapol
            } else {
                observe_access_point!(self, observation, {
                    observation.security_mode_mismatches =
                        observation.security_mode_mismatches.saturating_add(1);
                });
                AccessPointRxProtocolClass::Rejected
            }
        } else {
            observe_access_point!(self, observation, {
                observation.ignored_rx_frames = observation.ignored_rx_frames.saturating_add(1);
            });
            AccessPointRxProtocolClass::Other
        };
        if batch_exhausted {
            observe_access_point!(self, observation, {
                observation.protected_data_protocol_rejected = observation
                    .protected_data_protocol_rejected
                    .saturating_add(1);
            });
            return Err(Esp32s31AccessPointControlError::ReceiveBatchCapacity);
        }
        let payload_activity = activity_peer.map(|peer| {
            let power_state = match power_save_observation {
                Some(ApPowerSaveObservation::Sleeping { peer: observed }) if observed == peer => {
                    Some(ApPeerPowerState::Sleeping)
                }
                Some(ApPowerSaveObservation::Active { peer: observed }) if observed == peer => {
                    Some(ApPeerPowerState::Active)
                }
                _ => None,
            };
            (peer, power_state)
        });
        let null_data_activity = match null_data_power_save_observation {
            Some(ApPowerSaveObservation::Sleeping { peer })
                if self.mac.engine().is_authorized_peer(peer) =>
            {
                Some((peer, Some(ApPeerPowerState::Sleeping)))
            }
            Some(ApPowerSaveObservation::Active { peer })
                if self.mac.engine().is_authorized_peer(peer) =>
            {
                Some((peer, Some(ApPeerPowerState::Active)))
            }
            _ => None,
        };
        if let Some((peer, power_state)) = payload_activity.or(null_data_activity) {
            self.protocol_actions
                .publisher()
                .try_publish(Esp32s31AccessPointProtocolAction::Control(
                    Esp32s31AccessPointControlAction::ObservePeerActivity {
                        peer,
                        at_micros: now_micros,
                        power_state,
                    },
                ))
                .map_err(|_| Esp32s31AccessPointControlError::ProtocolActionCapacity)?;
            if let Some(hardware) = hardware {
                self.apply_protocol_actions(hardware)?;
            }
        }
        if let Some(observation @ ApPowerSaveObservation::PsPoll { .. }) = power_save_observation {
            let action = self
                .mac
                .engine_mut()
                .observe_power_save(observation, now_micros)?;
            self.retain_power_save_action(action)?;
        }
        Ok(protocol_class)
    }

    fn apply_protocol_actions<H>(
        &mut self,
        hardware: &mut H,
    ) -> Result<(), Esp32s31AccessPointControlError>
    where
        H: RxBlockAckHardware,
    {
        while let Some(action) = self.protocol_actions.receiver().try_receive() {
            match action {
                Esp32s31AccessPointProtocolAction::Hardware(
                    Esp32s31AccessPointHardwareAction::ResetRxBlockAckWindow {
                        hardware_index,
                        tid,
                        starting_sequence,
                        window,
                    },
                ) => hardware.reset_rx_block_ack_window(
                    hardware_index,
                    tid,
                    starting_sequence,
                    window,
                )?,
                Esp32s31AccessPointProtocolAction::Control(
                    Esp32s31AccessPointControlAction::ObservePeerActivity {
                        peer,
                        at_micros,
                        power_state,
                    },
                ) => match power_state {
                    Some(ApPeerPowerState::Active) => {
                        let action = self.mac.engine_mut().observe_power_save(
                            ApPowerSaveObservation::Active { peer },
                            at_micros,
                        )?;
                        self.retain_power_save_action(action)?;
                    }
                    Some(ApPeerPowerState::Sleeping) => {
                        let action = self.mac.engine_mut().observe_power_save(
                            ApPowerSaveObservation::Sleeping { peer },
                            at_micros,
                        )?;
                        self.retain_power_save_action(action)?;
                    }
                    None => self
                        .mac
                        .engine_mut()
                        .observe_peer_activity(peer, at_micros)?,
                },
            }
        }
        Ok(())
    }

    fn service_management<H>(
        &mut self,
        hardware: &mut H,
        mpdu: &[u8],
        authenticator_nonce: [u8; 32],
        initial_replay_counter: u64,
        now_micros: u64,
    ) -> Result<bool, Esp32s31AccessPointControlError>
    where
        H: TxHardware + Esp32s31ApRuntimeHardware + RxBlockAckHardware,
    {
        let request = parse_ap_management_request(mpdu, self.mac.engine().service_address());
        if let Some(ApManagementRequest::BlockAck { peer, action }) = request {
            match action {
                BlockAckAction::AddbaRequest {
                    dialog_token,
                    tid,
                    immediate,
                    window,
                    timeout_tu,
                    starting_sequence,
                    ..
                } if self.mac.engine().is_authorized_peer(peer) => {
                    if self.mac.engine().security_mode() == WifiSecurityMode::Open {
                        self.publish_declined_rx_addba(
                            hardware,
                            peer,
                            dialog_token,
                            tid,
                            window,
                        )?;
                        return Ok(true);
                    }
                    let offered = self.rx_block_ack.offer(RxBlockAckRequest {
                        interface: MacInterface::AccessPoint,
                        peer,
                        dialog_token,
                        tid,
                        immediate,
                        requested_window: window,
                        timeout_tu,
                        starting_sequence,
                    });
                    if offered.is_err() {
                        self.publish_declined_rx_addba(hardware, peer, dialog_token, tid, window)?;
                        return Ok(true);
                    }
                    let activation = match self.rx_block_ack.begin_pending() {
                        Ok(Some(activation)) => activation,
                        Ok(None) => return Ok(false),
                        Err(RxBlockAckSessionsError::NoFreeHardwareBank) => {
                            let discarded = self.rx_block_ack.discard_pending(
                                MacInterface::AccessPoint,
                                peer,
                                tid,
                            );
                            debug_assert!(discarded);
                            self.publish_declined_rx_addba(
                                hardware,
                                peer,
                                dialog_token,
                                tid,
                                window,
                            )?;
                            return Ok(true);
                        }
                        Err(error) => return Err(error.into()),
                    };
                    self.start_rx_addba_response(hardware, activation, now_micros)?;
                    return Ok(true);
                }
                BlockAckAction::Delba {
                    tid,
                    initiator: true,
                    ..
                } => {
                    if let Some(agreement) =
                        self.rx_block_ack.stop(MacInterface::AccessPoint, peer, tid)
                    {
                        self.release_rx_reorder(agreement.identity(), now_micros)?;
                        hardware.clear_rx_block_ack(agreement.hardware_index)?;
                    }
                    return Ok(false);
                }
                _ => {}
            }
        }

        let processor = &mut *self;
        let outcome = processor.mac.publish_management(
            hardware,
            mpdu,
            authenticator_nonce,
            initial_replay_counter,
            now_micros,
            processor.tx_frame,
        )?;
        if let open_esp_radio_esp32s31_wifi_ap::engine::Esp32s31ApManagementOutcome::PeerRemoved {
            peer,
        } = outcome
        {
            self.discard_rx_peer(hardware, peer)?;
        }
        Ok(matches!(
            outcome,
            open_esp_radio_esp32s31_wifi_ap::engine::Esp32s31ApManagementOutcome::Response { .. }
        ))
    }

    fn publish_declined_rx_addba<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        peer: [u8; 6],
        dialog_token: u8,
        tid: u8,
        requested_window: u16,
    ) -> Result<(), Esp32s31AccessPointControlError> {
        let mut body = [0_u8; 9];
        write_declined_addba_response(&mut body, dialog_token, tid, requested_window)
            .map_err(RxBlockAckSessionsError::Response)?;
        let processor = &mut *self;
        processor
            .mac
            .publish_rx_block_ack_response(hardware, peer, &body, processor.tx_frame)?;
        Ok(())
    }

    fn start_rx_addba_response<H>(
        &mut self,
        hardware: &mut H,
        activation: RxBlockAckActivation,
        now_micros: u64,
    ) -> Result<(), Esp32s31AccessPointControlError>
    where
        H: TxHardware + Esp32s31ApRuntimeHardware + RxBlockAckHardware,
    {
        debug_assert!(self.rx_addba_in_flight.is_none());
        if let Some(replaced) = activation.replaced() {
            if let Err(error) = self.release_rx_reorder(replaced.identity(), now_micros) {
                self.rx_block_ack.cancel(activation)?;
                return Err(error);
            }
            if let Err(error) = hardware.clear_rx_block_ack(replaced.hardware_index) {
                self.rx_block_ack.cancel(activation)?;
                return Err(error.into());
            }
        }
        let negotiated = activation.negotiated();
        // SOURCE: complete vendor `ht_recv_action_ba_addba_request` first
        // enqueues the successful ADDBA response through
        // `ieee80211_send_action`, then publishes the receive agreement via
        // `ic_add_rx_ba`. The direct bank must not become
        // visible before the response publication edge.
        let processor = &mut *self;
        if let Err(error) = processor.mac.publish_rx_block_ack_response(
            hardware,
            negotiated.peer,
            activation.response_body(),
            processor.tx_frame,
        ) {
            self.rx_block_ack.cancel(activation)?;
            return Err(error.into());
        }
        if let Err(error) = hardware.program_rx_block_ack(activation.hardware()) {
            self.rx_block_ack.cancel(activation)?;
            return Err(error.into());
        }
        if let Err(error) = self.rx_reorder.start(negotiated, |_| {}) {
            let clear = hardware.clear_rx_block_ack(negotiated.hardware_index);
            self.rx_block_ack.cancel(activation)?;
            clear?;
            return Err(error.into());
        }
        self.rx_addba_in_flight = Some(activation);
        Ok(())
    }

    fn release_rx_reorder(
        &mut self,
        identity: open_esp_radio_esp32s31_wifi_mac::rx_ampdu::RxBlockAckIdentity,
        now_micros: u64,
    ) -> Result<(), Esp32s31AccessPointControlError> {
        let processor = &mut *self;
        let mac = &mut processor.mac;
        let data_rx = &mut processor.data_rx;
        #[cfg(any(feature = "diagnostics", test))]
        let report = &mut processor.observer.observation;
        let mut activity_peer = None;
        let mut sink = DeferredAccessPointRxSink::new(processor.rx_frame);
        let _ = processor.rx_reorder.stop(identity, |segment| {
            let peer = data_rx.reorder_key(segment).map(|key| key.peer);
            let outcome = data_rx.dispatch_at(
                segment,
                now_micros,
                |request| mac.engine_mut().admit_rx_data(request),
                &mut sink,
            );
            let _ = observe_protected_dispatch(
                outcome,
                peer,
                #[cfg(any(feature = "diagnostics", test))]
                report,
                &mut activity_peer,
            );
        });
        if sink.exhausted {
            return Err(Esp32s31AccessPointControlError::ReceiveBatchCapacity);
        }
        if let Some(peer) = activity_peer {
            processor
                .mac
                .engine_mut()
                .observe_peer_activity(peer, now_micros)?;
        }
        let used = sink.used();
        if used != 0 {
            processor.rx_batch_used = used;
            processor.rx_batch_offset = 0;
        }
        Ok(())
    }

    fn discard_rx_peer<H: RxBlockAckHardware>(
        &mut self,
        hardware: &mut H,
        peer: [u8; 6],
    ) -> Result<(), Esp32s31AccessPointControlError> {
        for agreement in self
            .rx_block_ack
            .stop_peer(MacInterface::AccessPoint, peer)
            .into_iter()
            .flatten()
        {
            let _ = self.rx_reorder.stop_discard(agreement.identity());
            hardware.clear_rx_block_ack(agreement.hardware_index)?;
        }
        self.data_rx.forget_peer(peer);
        Ok(())
    }

    fn service_rx_reorder_expiry(
        &mut self,
        now_micros: u64,
    ) -> Result<bool, Esp32s31AccessPointControlError> {
        let processor = &mut *self;
        let mac = &mut processor.mac;
        let data_rx = &mut processor.data_rx;
        #[cfg(any(feature = "diagnostics", test))]
        let report = &mut processor.observer.observation;
        let mut activity_peer = None;
        let mut sink = DeferredAccessPointRxSink::new(processor.rx_frame);
        let pending_dispatched = processor.rx_reorder.dispatch_pending(|segment| {
            let peer = data_rx.reorder_key(segment).map(|key| key.peer);
            let outcome = data_rx.dispatch_at(
                segment,
                now_micros,
                |request| mac.engine_mut().admit_rx_data(request),
                &mut sink,
            );
            let _ = observe_protected_dispatch(
                outcome,
                peer,
                #[cfg(any(feature = "diagnostics", test))]
                report,
                &mut activity_peer,
            );
        });
        let (dispatched, _gap_timeout) = if pending_dispatched {
            (1, false)
        } else {
            let dispatched = processor.rx_reorder.expire_due(now_micros, |segment| {
                let peer = data_rx.reorder_key(segment).map(|key| key.peer);
                let outcome = data_rx.dispatch_at(
                    segment,
                    now_micros,
                    |request| mac.engine_mut().admit_rx_data(request),
                    &mut sink,
                );
                let _ = observe_protected_dispatch(
                    outcome,
                    peer,
                    #[cfg(any(feature = "diagnostics", test))]
                    report,
                    &mut activity_peer,
                );
            });
            (dispatched, dispatched != 0)
        };
        observe_access_point!(processor, observation, {
            if _gap_timeout {
                observation.rx_reorder_gap_timeouts =
                    observation.rx_reorder_gap_timeouts.saturating_add(1);
            }
            observation.rx_reorder_dispatched_mpdus = observation
                .rx_reorder_dispatched_mpdus
                .saturating_add(u32::from(dispatched));
        });
        if sink.exhausted {
            return Err(Esp32s31AccessPointControlError::ReceiveBatchCapacity);
        }
        if let Some(peer) = activity_peer {
            processor
                .mac
                .engine_mut()
                .observe_peer_activity(peer, now_micros)?;
        }
        let used = sink.used();
        if used != 0 {
            processor.rx_batch_used = used;
            processor.rx_batch_offset = 0;
            return Ok(true);
        }
        Ok(dispatched != 0)
    }

    pub const fn rx_batch_pending(&self) -> bool {
        self.rx_batch_offset < self.rx_batch_used
    }

    #[cfg(any(feature = "diagnostics", test))]
    fn observe_ht_aggregate(&mut self, rate: HtRate) {
        observe_access_point!(self, observation, {
            observation.tx_ht_aggregates = observation.tx_ht_aggregates.saturating_add(1);
            if rate.channel_width == HtChannelWidth::Mhz40 && rate.mcs == HtMcs::Mcs7 {
                observation.tx_ht40_mcs7_aggregates =
                    observation.tx_ht40_mcs7_aggregates.saturating_add(1);
            }
        });
    }
}
