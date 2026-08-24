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
    fn rx_batch_record(
        &self,
    ) -> Result<
        Option<crate::datapath::rx::ethernet::PackedEthernetRecord<'_>>,
        Esp32s31AccessPointControlError,
    > {
        crate::datapath::rx::ethernet::record_at(
            self.rx_frame,
            self.rx_batch_used,
            self.rx_batch_offset,
        )
        .map_err(|_| Esp32s31AccessPointControlError::ReceiveBatchCapacity)
    }

    fn commit_rx_batch_record(&mut self, next_offset: usize) {
        debug_assert!(next_offset > self.rx_batch_offset);
        debug_assert!(next_offset <= self.rx_batch_used);
        self.rx_batch_offset = next_offset;
        if self.rx_batch_offset == self.rx_batch_used {
            self.rx_batch_offset = 0;
            self.rx_batch_used = 0;
        }
    }

    fn service_eapol<H>(
        &mut self,
        hardware: &mut H,
        mpdu: &[u8],
        now_micros: u64,
    ) -> Result<bool, Esp32s31AccessPointControlError>
    where
        H: TxHardware + Esp32s31ApRuntimeHardware,
    {
        let header_length = if mpdu[0] & 0x80 != 0 {
            IEEE80211_QOS_DATA_HEADER_LEN
        } else {
            IEEE80211_LEGACY_DATA_HEADER_LEN
        };
        let Some(payload_length) = mpdu.len().checked_sub(header_length) else {
            observe_access_point!(self, observation, {
                observation.ignored_rx_frames = observation.ignored_rx_frames.saturating_add(1);
            });
            return Ok(false);
        };
        let Ok(plan) = plan_data_decapsulation(
            DataInterfaceRole::AccessPoint,
            mpdu,
            header_length,
            payload_length,
        ) else {
            observe_access_point!(self, observation, {
                observation.ignored_rx_frames = observation.ignored_rx_frames.saturating_add(1);
            });
            return Ok(false);
        };
        if plan.ether_type != EAPOL_ETHERTYPE
            || plan.destination != self.mac.engine().service_address()
            || self.mac.engine().peer_status(plan.source).is_none()
        {
            observe_access_point!(self, observation, {
                observation.ignored_rx_frames = observation.ignored_rx_frames.saturating_add(1);
            });
            return Ok(false);
        }
        let payload = &mpdu[plan.payload_offset..plan.payload_offset + plan.payload_length];
        let Ok(frame) = OwnedEapolFrame::<EAPOL_CAPACITY>::try_copy(
            Wpa2Interface::AccessPoint,
            plan.source,
            payload,
        ) else {
            observe_access_point!(self, observation, {
                observation.ignored_rx_frames = observation.ignored_rx_frames.saturating_add(1);
            });
            return Ok(false);
        };
        match self
            .mac
            .engine_mut()
            .handle_eapol(hardware, plan.source, frame, now_micros)?
        {
            Esp32s31ApWpa2Outcome::Transmit(frame) => {
                let processor = &mut *self;
                processor
                    .mac
                    .publish_eapol(hardware, plan.source, &frame, processor.tx_frame)?;
                observe_access_point!(self, observation, {
                    observation.control_frames_staged =
                        observation.control_frames_staged.saturating_add(1);
                });
            }
            Esp32s31ApWpa2Outcome::DeauthenticatePeer { peer } => {
                let close = self
                    .mac
                    .engine_mut()
                    .begin_wpa2_failure_close(peer)
                    .map_err(Esp32s31ApMacError::Engine)?;
                self.publish_peer_close(hardware, close)?;
            }
            Esp32s31ApWpa2Outcome::PeerAuthorized { peer } => {
                let processor = &mut *self;
                if processor.mac.publish_tx_block_ack_request(
                    hardware,
                    peer,
                    now_micros,
                    processor.tx_frame,
                )? {
                    observe_access_point!(self, observation, {
                        observation.control_frames_staged =
                            observation.control_frames_staged.saturating_add(1);
                    });
                }
            }
            Esp32s31ApWpa2Outcome::None => {}
        }
        Ok(true)
    }

    pub async fn service_tx<H>(
        &mut self,
        hardware: &mut H,
        wake: WifiTxWake,
    ) -> Result<WifiTxProgress, Esp32s31AccessPointControlError>
    where
        H: Esp32s31ApRuntimeHardware + TxHardware + RxBlockAckHardware,
    {
        let (progress, action) = self
            .mac
            .service_tx(hardware, wake, Instant::now().as_micros())
            .await?;
        if progress == WifiTxProgress::Complete {
            self.last_terminal_tx_succeeded = Some(
                action != Esp32s31ApTxCompletionAction::PublicationFailed,
            );
        }
        if progress == WifiTxProgress::Complete
            && let Some(activation) = self.rx_addba_in_flight.take()
        {
            let negotiated = activation.negotiated();
            if action == Esp32s31ApTxCompletionAction::PublicationFailed {
                hardware.clear_rx_block_ack(negotiated.hardware_index)?;
                let _ = self.rx_reorder.stop_discard(negotiated.identity());
                self.rx_block_ack.cancel(activation)?;
            } else {
                self.rx_block_ack.commit(activation)?;
            }
        }
        match action {
            Esp32s31ApTxCompletionAction::DtimGroupRelease { advertised_frames } => {
                if self.pending_dtim_group_frames.is_some() {
                    return Err(
                        Esp32s31AccessPointControlError::DtimGroupReleaseAlreadyPending,
                    );
                }
                self.pending_dtim_group_frames = Some(advertised_frames);
            }
            Esp32s31ApTxCompletionAction::BeginWpa2 { peer } => {
                let message1 = self.mac.engine().begin_wpa2::<EAPOL_CAPACITY>(peer)?;
                let processor = &mut *self;
                processor
                    .mac
                    .publish_eapol(hardware, peer, &message1, processor.tx_frame)?;
                observe_access_point!(self, observation, {
                    observation.control_frames_staged =
                        observation.control_frames_staged.saturating_add(1);
                });
                return Ok(WifiTxProgress::Pending);
            }
            Esp32s31ApTxCompletionAction::PeerDisconnectTerminal {
                close,
                stage: Esp32s31ApPeerDisconnectStage::Disassociation,
                ..
            } => {
                let processor = &mut *self;
                processor.mac.publish_peer_disconnect(
                    hardware,
                    close,
                    Esp32s31ApPeerDisconnectStage::Deauthentication,
                    processor.tx_frame,
                )?;
                observe_access_point!(self, observation, {
                    observation.control_frames_staged =
                        observation.control_frames_staged.saturating_add(1);
                });
                return Ok(WifiTxProgress::Pending);
            }
            Esp32s31ApTxCompletionAction::PeerDisconnectTerminal {
                close,
                stage: Esp32s31ApPeerDisconnectStage::Deauthentication,
                ..
            } => {
                self.discard_rx_peer(hardware, close.peer)?;
                self.mac.engine_mut().complete_peer_close(hardware, close)?;
            }
            Esp32s31ApTxCompletionAction::None
            | Esp32s31ApTxCompletionAction::PublicationFailed => {}
        }
        Ok(progress)
    }

    fn publish_peer_close<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        close: ApPeerClose,
    ) -> Result<(), Esp32s31AccessPointControlError> {
        let stage = if close.was_associated {
            Esp32s31ApPeerDisconnectStage::Disassociation
        } else {
            Esp32s31ApPeerDisconnectStage::Deauthentication
        };
        let processor = &mut *self;
        processor
            .mac
            .publish_peer_disconnect(hardware, close, stage, processor.tx_frame)?;
        observe_access_point!(self, observation, {
            observation.control_frames_staged = observation.control_frames_staged.saturating_add(1);
        });
        Ok(())
    }

    #[cfg(any(feature = "diagnostics", test))]
    pub fn observation(&self) -> Esp32s31AccessPointControlObservation {
        self.observer.observation
    }

    pub const fn serviced_rx_frames(&self) -> u64 {
        self.serviced_rx_frames
    }

    pub const fn serviced_rx_descriptors(&self) -> u64 {
        self.serviced_rx_descriptors
    }

    #[cfg(any(feature = "diagnostics", test))]
    pub fn mac_observation(&self) -> Esp32s31ApMacObservation {
        self.mac.observation()
    }

    /// Whether the AP owns at least one operational downlink Block Ack
    /// agreement and can therefore profitably collect a network TX batch.
    pub fn has_operational_tx_block_ack(&self) -> bool {
        self.mac.engine().has_operational_tx_block_ack()
    }

    pub fn smallest_operational_tx_block_ack_window(&self) -> Option<u16> {
        self.mac
            .engine()
            .smallest_operational_tx_block_ack_window()
    }

    pub const fn tx_pending(&self) -> bool {
        self.mac.tx_pending()
    }

    pub const fn next_beacon_delay(&self, now_micros: u32) -> Option<(u32, u32)> {
        self.mac.next_beacon_delay(now_micros)
    }

    pub const fn beacon_publication_due(&self, now_micros: u32) -> bool {
        self.mac.beacon_publication_due(now_micros)
    }

    pub fn wait_tx_deadline(&mut self) -> impl core::future::Future<Output = ()> + '_ {
        self.mac.wait_tx_deadline()
    }

    pub fn publish_beacon<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        now_micros: u64,
    ) -> Result<(), Esp32s31AccessPointControlError> {
        #[cfg(any(feature = "diagnostics", test))]
        {
            let (missed, lateness) = self.mac.beacon_publication_lateness(now_micros as u32);
            observe_access_point!(self, observation, {
                observation.missed_beacon_intervals =
                    observation.missed_beacon_intervals.saturating_add(missed);
                observation.maximum_beacon_lateness_micros =
                    observation.maximum_beacon_lateness_micros.max(lateness);
            });
        }
        self.mac.publish_beacon(hardware, now_micros)?;
        Ok(())
    }

    /// Copy one network-owned Ethernet frame into the AP's ordinary DMA slot
    /// and begin a pairwise protected publication.
    ///
    /// The caller may release its network lease after this method returns:
    /// the complete plaintext MPDU is then owned by `self` until terminal TX.
    pub fn publish_ethernet<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        peer: [u8; 6],
        ethernet: &[u8],
    ) -> Result<(), Esp32s31AccessPointControlError> {
        let processor = &mut *self;
        processor
            .mac
            .publish_ethernet(hardware, peer, ethernet, processor.tx_frame)?;
        Ok(())
    }

    fn publish_power_save_ethernet<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        peer: [u8; 6],
        ethernet: &[u8],
        more_data: bool,
    ) -> Result<(), Esp32s31AccessPointControlError> {
        let processor = &mut *self;
        processor.mac.publish_ethernet_with_more_data(
            hardware,
            peer,
            ethernet,
            processor.tx_frame,
            more_data,
        )?;
        Ok(())
    }

    fn start_network_tx<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        ethernet: &[u8],
    ) -> Result<WifiTxProgress, Esp32s31AccessPointControlError> {
        self.start_network_tx_with_more_data(hardware, ethernet, false)
    }

    /// Try to publish two ordered network frames as one AP QoS A-MSDU.
    ///
    /// `Ok(None)` is an exact non-consuming miss; the network owner keeps the
    /// second lease ahead of every frame still in the channel and transmits
    /// the first through the ordinary path.
    fn start_network_amsdu_pair<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        first: &[u8],
        second: &[u8],
    ) -> Result<Option<WifiTxProgress>, Esp32s31AccessPointControlError> {
        let processor = &mut *self;
        if !processor.mac.publish_amsdu_pair(
            hardware,
            first,
            second,
            processor.tx_frame,
        )? {
            return Ok(None);
        }
        observe_access_point!(self, observation, {
            observation.network_tx_frames_observed =
                observation.network_tx_frames_observed.saturating_add(2);
        });
        Ok(Some(WifiTxProgress::Pending))
    }

    fn start_network_tx_with_more_data<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        ethernet: &[u8],
        more_data: bool,
    ) -> Result<WifiTxProgress, Esp32s31AccessPointControlError> {
        observe_access_point!(self, observation, {
            observation.network_tx_frames_observed =
                observation.network_tx_frames_observed.saturating_add(1);
            match ethernet_protocol(ethernet) {
                Some(EthernetProtocol::ArpRequest) => {
                    observation.network_tx_arp_requests =
                        observation.network_tx_arp_requests.saturating_add(1);
                }
                Some(EthernetProtocol::ArpReply) => {
                    observation.network_tx_arp_replies =
                        observation.network_tx_arp_replies.saturating_add(1);
                }
                _ => {}
            }
        });
        if self.mac.engine().authorized_peer_count() == 0 {
            observe_access_point!(self, observation, {
                observation.network_tx_rejected_no_peer =
                    observation.network_tx_rejected_no_peer.saturating_add(1);
                observation.network_tx_frames_rejected =
                    observation.network_tx_frames_rejected.saturating_add(1);
            });
            return Ok(WifiTxProgress::Complete);
        }
        let Some(destination) = ethernet
            .get(..6)
            .and_then(|bytes| <[u8; 6]>::try_from(bytes).ok())
        else {
            observe_access_point!(self, observation, {
                observation.network_tx_rejected_destination = observation
                    .network_tx_rejected_destination
                    .saturating_add(1);
                observation.network_tx_frames_rejected =
                    observation.network_tx_frames_rejected.saturating_add(1);
            });
            return Ok(WifiTxProgress::Complete);
        };
        if destination[0] & 1 == 0 && !self.mac.engine().is_authorized_peer(destination) {
            observe_access_point!(self, observation, {
                observation.network_tx_rejected_destination = observation
                    .network_tx_rejected_destination
                    .saturating_add(1);
                observation.network_tx_frames_rejected =
                    observation.network_tx_frames_rejected.saturating_add(1);
            });
            return Ok(WifiTxProgress::Complete);
        }
        if more_data {
            self.publish_power_save_ethernet(hardware, destination, ethernet, true)?;
        } else {
            self.publish_ethernet(hardware, destination, ethernet)?;
        }
        Ok(WifiTxProgress::Pending)
    }

    fn take_last_terminal_tx_succeeded(&mut self) -> Option<bool> {
        self.last_terminal_tx_succeeded.take()
    }

    fn take_pending_dtim_group_frames(&mut self) -> Option<u16> {
        self.pending_dtim_group_frames.take()
    }

    fn role_status_revision(&self) -> u32 {
        self.mac.engine().service_status_revision()
    }

    fn role_status(&self) -> AccessPointServiceStatus {
        self.mac.engine().service_status()
    }

    /// Advance AP timer and peer policy by one finite DATAPATH control step.
    ///
    /// This method never waits. A published frame returns `TxPending`; the
    /// caller must drive the shared TX owner to a terminal edge before
    /// invoking another control transition.
    pub fn service_control<H>(
        &mut self,
        hardware: &mut H,
        now_micros: u64,
    ) -> Result<DatapathControlProgress<Infallible>, Esp32s31AccessPointControlError>
    where
        H: TxHardware + Esp32s31ApRuntimeHardware,
    {
        if self.tx_pending() {
            return Ok(DatapathControlProgress::TxPending);
        }
        if self.beacon_publication_due(now_micros as u32) {
            self.publish_beacon(hardware, now_micros)?;
            return Ok(DatapathControlProgress::TxPending);
        }
        self.mac.expire_tx_block_ack(now_micros)?;
        match self
            .mac
            .engine_mut()
            .take_due_wpa2_retry::<EAPOL_CAPACITY>(now_micros)?
        {
            ApWpa2RetryProgress::Transmit { peer, frame } => {
                let processor = &mut *self;
                processor
                    .mac
                    .publish_eapol(hardware, peer, &frame, processor.tx_frame)?;
                observe_access_point!(self, observation, {
                    observation.control_frames_staged =
                        observation.control_frames_staged.saturating_add(1);
                });
                return Ok(DatapathControlProgress::TxPending);
            }
            ApWpa2RetryProgress::Close(close) => {
                self.publish_peer_close(hardware, close)?;
                return Ok(DatapathControlProgress::TxPending);
            }
            ApWpa2RetryProgress::None => {}
        }
        if let Some(close) = self.mac.engine_mut().begin_due_peer_close(now_micros) {
            self.publish_peer_close(hardware, close)?;
            return Ok(DatapathControlProgress::TxPending);
        }
        Ok(DatapathControlProgress::Idle)
    }

    fn next_control_deadline_micros(
        &self,
        now_micros: u64,
    ) -> Result<u64, Esp32s31AccessPointControlError> {
        let (beacon_tick, _) = self
            .next_beacon_delay(now_micros as u32)
            .ok_or(Esp32s31AccessPointControlError::InvalidBeaconSchedule)?;
        let beacon_deadline = now_micros
            .saturating_add(u64::from(beacon_tick.wrapping_sub(now_micros as u32)));
        Ok(self
            .mac
            .engine()
            .next_peer_deadline()
            .into_iter()
            .chain(self.mac.engine().next_wpa2_retry_deadline())
            .chain(self.mac.next_tx_block_ack_deadline())
            .chain(self.rx_reorder.next_deadline())
            .fold(beacon_deadline, u64::min))
    }

    /// Advance AP shutdown by one finite DATAPATH transition.
    pub fn service_stop<H>(
        &mut self,
        hardware: &mut H,
    ) -> Result<DatapathStopProgress, Esp32s31AccessPointControlError>
    where
        H: TxHardware + RxBlockAckHardware,
    {
        // Shutdown deliberately drops a decoded-but-unpublished network
        // batch. Execute all already-accepted protocol actions before peer
        // teardown so `try_finish_paired` can prove a truly empty mailbox.
        self.rx_batch_used = 0;
        self.rx_batch_offset = 0;
        self.apply_protocol_actions(hardware)?;
        if let Some(close) = self.mac.engine_mut().begin_stop_peer() {
            self.publish_peer_close(hardware, close)?;
            Ok(DatapathStopProgress::TxPending)
        } else {
            Ok(DatapathStopProgress::Stopped)
        }
    }

    /// Consume a quiescent AP protocol role from the paired DATAPATH boundary.
    ///
    /// The common RX producer is intentionally not part of this transaction.
    /// Any pending protocol, reorder, BlockAck, or TX state returns the exact
    /// processor unchanged.
    #[allow(clippy::result_large_err, clippy::type_complexity)]
    pub fn try_finish_paired<H>(
        self,
        hardware: &mut H,
    ) -> Result<
        Esp32s31AccessPointProtocolStopped<
            'storage,
            'beacon,
            'slot,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
        Self,
    >
    where
        H: Esp32s31ApRuntimeHardware,
    {
        if self.rx_batch_pending()
            || self.rx_addba_in_flight.is_some()
            || !self.protocol_actions.is_empty()
            || !self.pending_buffered_releases.is_empty()
            || self.pending_dtim_group_frames.is_some()
            || self.rx_reorder.has_pending_release()
            || self
                .rx_block_ack
                .snapshots_for(MacInterface::AccessPoint)
                .into_iter()
                .any(|entry| entry.is_some())
        {
            return Err(self);
        }
        let Self {
            mac,
            rx_frame,
            tx_frame,
            data_rx,
            rx_block_ack,
            rx_reorder,
            rx_reorder_storage,
            rx_addba_in_flight: _,
            protocol_actions,
            pending_buffered_releases,
            pending_dtim_group_frames,
            rx_batch_used: _,
            rx_batch_offset: _,
            serviced_rx_frames,
            serviced_rx_descriptors,
            last_terminal_tx_succeeded,
            #[cfg(any(feature = "diagnostics", test))]
            observer,
            #[cfg(any(feature = "diagnostics", test))]
            terminal_observer,
        } = self;
        #[cfg(any(feature = "diagnostics", test))]
        let mac_observation = mac.observation();
        #[cfg(any(feature = "diagnostics", test))]
        let engine_observation = mac.engine().observation();
        #[cfg(any(feature = "diagnostics", test))]
        let control_observation = observer.observation;
        let parts = match mac.try_into_parts() {
            Ok(parts) => parts,
            Err(mac) => {
                return Err(Self {
                    mac,
                    rx_frame,
                    tx_frame,
                    data_rx,
                    rx_block_ack,
                    rx_reorder,
                    rx_reorder_storage,
                    rx_addba_in_flight: None,
                    protocol_actions,
                    pending_buffered_releases,
                    pending_dtim_group_frames,
                    rx_batch_used: 0,
                    rx_batch_offset: 0,
                    serviced_rx_frames,
                    serviced_rx_descriptors,
                    last_terminal_tx_succeeded,
                    #[cfg(any(feature = "diagnostics", test))]
                    observer,
                    #[cfg(any(feature = "diagnostics", test))]
                    terminal_observer,
                });
            }
        };
        let open_esp_radio_esp32s31_wifi_ap::mac::Esp32s31ApMacParts { engine, transmit } = parts;
        let engine = engine.stop(hardware);
        #[cfg(any(feature = "diagnostics", test))]
        if let Some(terminal_observer) = terminal_observer {
            terminal_observer.observe(AccessPointTerminalObservation {
                control: control_observation,
                mac: mac_observation,
                engine: engine_observation,
            });
        }
        Ok(Esp32s31AccessPointProtocolStopped {
            transmit,
            rx_frame,
            tx_frame,
            data_rx,
            rx_block_ack,
            rx_reorder,
            rx_reorder_storage,
            #[cfg(feature = "diagnostics")]
            observation_storage: observer,
            engine,
        })
    }
}
