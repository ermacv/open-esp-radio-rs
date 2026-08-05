use super::*;

impl<
    'slot,
    'ampdu,
    'resources,
    M,
    P,
    E,
    T,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
    const SLOTS: usize,
    const AMPDU_BUFFER_SIZE: usize,
    const ORDINARY_BUFFER_SIZE: usize,
>
    Esp32s31ConnectedTx<
        'slot,
        'ampdu,
        'resources,
        M,
        P,
        E,
        T,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        QUEUE_DEPTH,
        SLOTS,
        AMPDU_BUFFER_SIZE,
        ORDINARY_BUFFER_SIZE,
    >
where
    M: RawMutex,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
{
    pub fn start_network<H: HtAmpduHardware>(
        &mut self,
        hardware: &mut H,
        first: PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
        network: &PinnedTxConsumer<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
    ) -> Result<WifiTxProgress, AggregateTxError> {
        if self.active() {
            return Err(AggregateTxError::ActiveTransaction);
        }
        self.last_aggregate_status = None;
        self.pending_ordinary_retry = None;
        let aggregate_rate = !matches!(self.config.rate, TxPhyRate::Legacy(_));
        let ht_requires_pair = matches!(self.config.rate, TxPhyRate::Ht(_));
        if !aggregate_rate {
            return self.start_network_ordinary(
                hardware,
                first,
                NetworkSingleMpduReason::LegacyRate,
            );
        }
        if !self.block_ack_operational(DATA_TID) {
            return self.start_network_ordinary(
                hardware,
                first,
                NetworkSingleMpduReason::BlockAckUnavailable,
            );
        }
        if ht_requires_pair && network.queue_len() == 0 {
            return self.start_network_ordinary(
                hardware,
                first,
                NetworkSingleMpduReason::HtNeedsPair,
            );
        }

        // BlockAck eligibility does not imply that every network frame fits
        // the peer/rate/TXOP ceiling of a fresh aggregate. In particular,
        // control-plane traffic can arrive immediately after ADDBA. Such a
        // frame remains a valid ordinary QoS MPDU and must not terminate the
        // complete radio runner with `AggregateFull`.
        if !self.first_frame_fits_fresh_aggregate(first.ethernet_length())? {
            return self.start_network_ordinary(
                hardware,
                first,
                NetworkSingleMpduReason::FreshAggregateCapacity,
            );
        }

        let preparation_started = self.counters.map(|_| self.ordinary.now_micros());
        self.prepare_aggregate(first, network)?;
        if let (Some(counters), Some(started)) = (self.counters, preparation_started) {
            counters.record_preparation_time(self.ordinary.now_micros().wrapping_sub(started));
        }
        self.publish_initial(hardware)
    }

    fn start_network_ordinary<H: HtAmpduHardware>(
        &mut self,
        hardware: &mut H,
        first: PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
        reason: NetworkSingleMpduReason,
    ) -> Result<WifiTxProgress, AggregateTxError> {
        let ethernet_length = first.ethernet_length();
        let progress = self.ordinary.start(hardware, first.ethernet())?;
        drop(first);
        if let Some(counters) = self.counters {
            counters.record_network_single_mpdu(reason, ethernet_length);
        }
        self.active = ConnectedTxActive::Ordinary;
        Ok(progress)
    }

    fn first_frame_fits_fresh_aggregate(
        &self,
        ethernet_length: usize,
    ) -> Result<bool, AggregateTxError> {
        let frame_length = ethernet_length
            .checked_add(STA_PROTECTED_QOS_ETHERNET_OVERHEAD)
            .ok_or(AggregateTxError::BufferSizeOverflow)?;
        let dma_capacity = HEADROOM + FRAME_CAPACITY + TRAILER;
        let hardware_mic_length = crate::ordinary_tx::TX_CCMP_MIC_SIZE as u8;
        let frame_size = AmpduFrameSize::new(frame_length, hardware_mic_length);
        let maximum_aggregate_bytes = self.ordinary.policy().ht_ampdu().maximum_aggregate_bytes();
        match self.config.rate {
            TxPhyRate::Ht(rate) => Ok(self.ampdu.can_fit_fresh_referenced_ht_frame(
                frame_length,
                hardware_mic_length,
                rate,
                maximum_aggregate_bytes,
                dma_capacity,
            )?),
            TxPhyRate::He(rate) => Ok(self.ampdu.can_fit_fresh_referenced_he_frame(
                frame_size,
                HeAmpduPolicy::new(
                    rate,
                    self.ordinary.policy().ht_ampdu().density(),
                    self.config.he_txop_limit,
                ),
                maximum_aggregate_bytes,
                dma_capacity,
            )?),
            TxPhyRate::Legacy(_) => Err(AggregateTxError::UnsupportedRate),
        }
    }

    fn prepare_aggregate(
        &mut self,
        first: PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
        network: &PinnedTxConsumer<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
    ) -> Result<(), AggregateTxError> {
        let first_sequence = self
            .ordinary
            .peek_qos_sequence(DATA_TID)
            .ok_or(AggregateTxError::MissingQosSequence(DATA_TID))?;
        // Association policy is owned outside the DMA-visible descriptor
        // arena. Reinstall its byte ceiling at every Free -> Reserved edge so
        // a new batch cannot depend on cold scalar contents retained beside
        // hardware-owned words.
        self.ampdu.configure_max_aggregate_bytes(
            self.ordinary.policy().ht_ampdu().maximum_aggregate_bytes(),
        )?;
        let cookie = self.ampdu.begin()?;
        self.cookie = Some(cookie);

        let result = self.prepare_reserved(first, network, first_sequence, cookie);
        if result.is_err() {
            self.cancel_prepared();
        }
        result
    }

    fn prepare_reserved(
        &mut self,
        first: PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
        network: &PinnedTxConsumer<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
        first_sequence: u16,
        cookie: TxCookie,
    ) -> Result<(), AggregateTxError> {
        self.push_frame(first)?;

        let build_stop = loop {
            if self.ampdu.held_backing_count() >= usize::from(self.config.frame_limit) {
                break AggregateBuildStop::FrameLimit;
            }
            if !self.can_push(FRAME_CAPACITY)? {
                break AggregateBuildStop::CapacityLimit;
            }
            let Some(frame) = network.try_receive() else {
                break AggregateBuildStop::QueueEmpty;
            };
            self.push_frame(frame)?;
        };

        let aggregate = self.ampdu.prepared_aggregate(cookie)?;
        let retry = AmpduRetryState::<SLOTS>::new(
            first_sequence,
            aggregate.subframes,
            AmpduRetryPolicy {
                attempt_limit: self.config.attempt_limit,
                retain_single_mpdu: matches!(self.config.rate, TxPhyRate::He(_)),
            },
        )?;
        let config = self.publication_config(aggregate.bytes, aggregate.subframes)?;
        self.active = ConnectedTxActive::Aggregate(AggregateActive {
            config,
            retry,
            original_subframes: aggregate.subframes,
            deadline_micros: 0,
            first_publication_micros: None,
        });
        if let Some(counters) = self.counters {
            counters.record_prepared(aggregate.subframes, build_stop);
        }
        Ok(())
    }

    fn can_push(&self, ethernet_length: usize) -> Result<bool, AggregateTxError> {
        let cookie = self.cookie.ok_or(AggregateTxError::MissingCookie)?;
        let frame_length = ethernet_length
            .checked_add(STA_PROTECTED_QOS_ETHERNET_OVERHEAD)
            .ok_or(AggregateTxError::BufferSizeOverflow)?;
        let dma_capacity = HEADROOM + FRAME_CAPACITY + TRAILER;
        let hardware_mic_length = crate::ordinary_tx::TX_CCMP_MIC_SIZE as u8;
        let frame_size = AmpduFrameSize::new(frame_length, hardware_mic_length);
        match self.config.rate {
            TxPhyRate::Ht(rate) => Ok(self.ampdu.can_commit_referenced_ht_frame(
                cookie,
                frame_length,
                hardware_mic_length,
                0,
                rate,
                dma_capacity,
            )?),
            TxPhyRate::He(rate) => Ok(self.ampdu.can_commit_referenced_he_frame(
                cookie,
                frame_size,
                HeAmpduPolicy::new(
                    rate,
                    self.ordinary.policy().ht_ampdu().density(),
                    self.config.he_txop_limit,
                ),
                dma_capacity,
            )?),
            TxPhyRate::Legacy(_) => Err(AggregateTxError::UnsupportedRate),
        }
    }

    fn push_frame(
        &mut self,
        mut frame: PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
    ) -> Result<(), AggregateTxError> {
        if self.ampdu.held_backing_count() >= SLOTS || !self.can_push(frame.ethernet_length())? {
            return Err(HtAmpduTxError::AggregateFull.into());
        }
        let metadata = self
            .ordinary
            .take_protected_metadata(DATA_TID)
            .ok_or(AggregateTxError::MissingQosSequence(DATA_TID))?;
        let ethernet_offset = frame.ethernet_offset();
        let ethernet_length = frame.ethernet_length();
        let encoded = metadata
            .encode_in_place(
                frame.storage_mut(),
                ethernet_offset,
                ethernet_length,
                DataHeControl::Disabled,
            )
            .map_err(AggregateTxError::Encode)?;
        let metadata_size = open_esp_radio_esp32s31_wifi_mac::tx_ampdu::TX_AMPDU_METADATA_SIZE;
        let dma_offset = encoded.offset.checked_sub(metadata_size).ok_or(
            AggregateTxError::DmaPrefixGeometry {
                encoded_offset: encoded.offset,
                metadata_size,
            },
        )?;
        let cookie = self.cookie.ok_or(AggregateTxError::MissingCookie)?;
        let hardware_mic_length = crate::ordinary_tx::TX_CCMP_MIC_SIZE as u8;
        let frame_size = AmpduFrameSize::new(encoded.length, hardware_mic_length);
        let layout = AmpduFrameLayout::new(dma_offset, frame_size).ok_or(
            AggregateTxError::DmaPrefixGeometry {
                encoded_offset: encoded.offset,
                metadata_size,
            },
        )?;
        match self.config.rate {
            TxPhyRate::Ht(rate) => {
                self.ampdu
                    .commit_ht(cookie, frame, HtAmpduFrameRequest::new(layout, 0, rate))?
            }
            TxPhyRate::He(rate) => self.ampdu.commit_he(
                cookie,
                frame,
                HeAmpduFrameRequest::new(
                    layout,
                    HeAmpduPolicy::new(
                        rate,
                        self.ordinary.policy().ht_ampdu().density(),
                        self.config.he_txop_limit,
                    ),
                ),
            )?,
            TxPhyRate::Legacy(_) => return Err(AggregateTxError::UnsupportedRate),
        }
        Ok(())
    }

    fn publication_config(
        &mut self,
        aggregate_length: u16,
        subframes: u8,
    ) -> Result<AmpduTxConfig, AggregateTxError> {
        let queue = LegacyTxQueue::BestEffort;
        let key = self.ordinary.hardware_key_selector();
        let (contention, contention_window) = self.ordinary.contention_publication(queue);
        match self.config.rate {
            TxPhyRate::Ht(rate) => {
                let mut config = HtAmpduTxConfig::new(rate, aggregate_length, subframes)
                    .ok_or(AggregateTxError::BufferSizeOverflow)?;
                let data_power = self
                    .ordinary
                    .power_profile()
                    .power_pair(rate.power_lookup_code());
                let rts_power = self
                    .ordinary
                    .power_profile()
                    .power_pair(rate.vendor_rts_rate().code());
                config.data_power_primary = data_power.primary as u8;
                config.data_power_alternate = data_power.alternate as u8;
                config.rts_power_primary = rts_power.primary as u8;
                config.rts_power_alternate = rts_power.alternate as u8;
                config.protection_spacing = self.ordinary.policy().ht_ampdu().protection_spacing();
                config.aifsn = contention.aifsn();
                config.contention_window = contention_window;
                config.scheduler_priority = queue.vendor_data_scheduler_priority();
                config.pti = queue.vendor_data_packet_priority();
                config.pti_count = 1;
                config.hardware_key_selector = key;
                Ok(AmpduTxConfig::Ht(config))
            }
            TxPhyRate::He(rate) => {
                let mut config = HeAmpduTxConfig::new_with_txop(
                    rate,
                    self.ordinary.policy().he_bss_color(),
                    aggregate_length,
                    subframes,
                    self.ordinary.policy().ht_ampdu().density(),
                    self.config.he_txop_limit,
                )
                .ok_or(AggregateTxError::BufferSizeOverflow)?;
                let data_power = self
                    .ordinary
                    .power_profile()
                    .power_pair(rate.power_lookup_code());
                let rts_power = self
                    .ordinary
                    .power_profile()
                    .power_pair(rate.vendor_rts_rate().code());
                config.data_power_primary = data_power.primary as u8;
                config.data_power_alternate = data_power.alternate as u8;
                config.rts_power_primary = rts_power.primary as u8;
                config.rts_power_alternate = rts_power.alternate as u8;
                config.aifsn = contention.aifsn();
                config.contention_window = contention_window;
                config.scheduler_priority = queue.vendor_data_scheduler_priority();
                config.pti = queue.vendor_data_packet_priority();
                config.pti_count = 1;
                config.hardware_key_selector = key;
                Ok(AmpduTxConfig::He(config))
            }
            TxPhyRate::Legacy(_) => Err(AggregateTxError::UnsupportedRate),
        }
    }

    fn publish_initial<H: HtAmpduHardware>(
        &mut self,
        hardware: &mut H,
    ) -> Result<WifiTxProgress, AggregateTxError> {
        let active = mem::replace(&mut self.active, ConnectedTxActive::Idle);
        let ConnectedTxActive::Aggregate(mut active) = active else {
            return Err(AggregateTxError::InvalidPublicationState);
        };
        if let Err(error) = self.publish_attempt(hardware, &mut active) {
            self.cancel_prepared();
            return Err(error);
        }
        self.last_aggregate_status = None;
        self.pending_ordinary_retry = None;
        self.active = ConnectedTxActive::Aggregate(active);
        Ok(WifiTxProgress::Pending)
    }

    pub(super) fn publish_attempt<H: HtAmpduHardware>(
        &mut self,
        hardware: &mut H,
        active: &mut AggregateActive<SLOTS>,
    ) -> Result<(), AggregateTxError> {
        let publication_started = self.ordinary.now_micros();
        let deadline = publication_started
            .checked_add(self.config.completion_timeout_us)
            .ok_or(AggregateTxError::DeadlineOverflow)?;
        match active.config {
            AmpduTxConfig::Ht(config) => self.ampdu.submit(
                hardware,
                self.cookie.ok_or(AggregateTxError::MissingCookie)?,
                LegacyTxQueue::BestEffort,
                config,
            )?,
            AmpduTxConfig::He(config) => self.ampdu.submit_he(
                hardware,
                self.cookie.ok_or(AggregateTxError::MissingCookie)?,
                LegacyTxQueue::BestEffort,
                config,
            )?,
        }
        if let Some(counters) = self.counters {
            let publication_finished = self.ordinary.now_micros();
            if active.first_publication_micros.is_none() {
                active.first_publication_micros = Some(publication_started);
            }
            counters.record_publication(publication_finished.wrapping_sub(publication_started));
        }
        active.deadline_micros = deadline;
        Ok(())
    }
}
