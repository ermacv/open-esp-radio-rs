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
        if self.active() || self.has_prepared_network_tx() {
            return Err(AggregateTxError::ActiveTransaction);
        }
        // Rate-control observations affect only the next fresh exchange.
        // An already published or retained retry keeps its original typed
        // rate and descriptor program until terminal completion.
        self.config.rate = self.rate_control.ampdu_tx_rate(self.aggregate_rate_policy);
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

        let preparation_started = self.observer.map(|_| self.ordinary.now_micros());
        let prepared = self.prepare_aggregate(first, network)?;
        self.observe_prepared(&prepared);
        if let (Some(observer), Some(started)) = (self.observer, preparation_started) {
            observer.observe(AggregateTxObservation::PreparationCompleted {
                micros: self.ordinary.now_micros().wrapping_sub(started),
            });
        }
        self.activate_prepared(prepared)?;
        let progress = self.publish_initial(hardware)?;
        self.prepare_standby(network);
        Ok(progress)
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
        if let Some(observer) = self.observer {
            observer.observe(AggregateTxObservation::NetworkSingleMpdu {
                reason,
                ethernet_length,
            });
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
        let hardware_mic_length = open_esp_radio_esp32s31_wifi::ordinary_tx::TX_CCMP_MIC_SIZE as u8;
        let frame_size = AmpduFrameSize::new(frame_length, hardware_mic_length);
        let maximum_aggregate_bytes = self.ordinary.policy().ht_ampdu().maximum_aggregate_bytes();
        match self.config.rate {
            TxPhyRate::Ht(rate) => Ok(self.ampdu.active().can_fit_fresh_referenced_ht_frame(
                frame_length,
                hardware_mic_length,
                rate,
                maximum_aggregate_bytes,
                dma_capacity,
            )?),
            TxPhyRate::He(rate) => Ok(self.ampdu.active().can_fit_fresh_referenced_he_frame(
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
    ) -> Result<AggregatePrepared<SLOTS>, AggregateTxError> {
        let first_sequence = self
            .ordinary
            .peek_qos_sequence(DATA_TID)
            .ok_or(AggregateTxError::MissingQosSequence(DATA_TID))?;
        // Association policy is owned outside the DMA-visible descriptor
        // arena. Reinstall its byte ceiling at every Free -> Reserved edge so
        // a new batch cannot depend on cold scalar contents retained beside
        // hardware-owned words.
        self.ampdu.active_mut().configure_max_aggregate_bytes(
            self.ordinary.policy().ht_ampdu().maximum_aggregate_bytes(),
        )?;
        let cookie = self.ampdu.active_mut().begin()?;
        self.cookie = Some(cookie);

        let result = self.prepare_reserved(first, network, first_sequence, cookie);
        if result.is_err() {
            self.cancel_current_reservation();
        }
        result
    }

    fn prepare_reserved(
        &mut self,
        first: PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
        network: &PinnedTxConsumer<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
        first_sequence: u16,
        cookie: TxCookie,
    ) -> Result<AggregatePrepared<SLOTS>, AggregateTxError> {
        self.push_candidate(first, network, AggregateFrameAdmission::FreshExact)?;
        let frame_limit = self.aggregate_frame_limit(DATA_TID);

        let build_stop = loop {
            if self.ampdu.active().held_backing_count() >= frame_limit {
                break AggregateBuildStop::FrameLimit;
            }
            if !self.can_push(FRAME_CAPACITY)? {
                break AggregateBuildStop::CapacityLimit;
            }
            let Some(frame) = network.try_receive() else {
                break AggregateBuildStop::QueueEmpty;
            };
            let admission = match self.config.rate {
                TxPhyRate::Ht(_) => AggregateFrameAdmission::HtQueueCapacity,
                TxPhyRate::He(_) => AggregateFrameAdmission::NeedsExactCheck,
                TxPhyRate::Legacy(_) => return Err(AggregateTxError::UnsupportedRate),
            };
            self.push_candidate(frame, network, admission)?;
        };

        let aggregate = self.ampdu.active().prepared_aggregate(cookie)?;
        let retry = AmpduRetryState::<SLOTS>::new(
            first_sequence,
            aggregate.subframes,
            AmpduRetryPolicy {
                attempt_limit: self.config.attempt_limit,
                // An A-MSDU can exceed the ordinary descriptor's copy
                // buffer. Retain even one missing A-MSDU in the aggregate
                // owner so retry preserves its pinned backing, sequence and
                // PN instead of attempting an impossible ordinary detach.
                retain_single_mpdu: matches!(self.config.rate, TxPhyRate::He(_))
                    || self.block_ack_amsdu(DATA_TID),
            },
        )?;
        let prepared = AggregatePrepared {
            aggregate_length: aggregate.bytes,
            retry,
            original_subframes: aggregate.subframes,
            first_sequence,
            build_stop,
            preparation_micros: 0,
        };
        Ok(prepared)
    }

    fn extend_reserved(
        &mut self,
        first: PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
        network: &PinnedTxConsumer<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
        mut prepared: AggregatePrepared<SLOTS>,
    ) -> Result<AggregatePrepared<SLOTS>, AggregateTxError> {
        let admission = match self.config.rate {
            TxPhyRate::Ht(_) => AggregateFrameAdmission::HtQueueCapacity,
            TxPhyRate::He(_) => AggregateFrameAdmission::NeedsExactCheck,
            TxPhyRate::Legacy(_) => return Err(AggregateTxError::UnsupportedRate),
        };
        self.push_candidate(first, network, admission)?;
        let frame_limit = self.aggregate_frame_limit(DATA_TID);
        let build_stop = loop {
            if self.ampdu.active().held_backing_count() >= frame_limit {
                break AggregateBuildStop::FrameLimit;
            }
            if !self.can_push(FRAME_CAPACITY)? {
                break AggregateBuildStop::CapacityLimit;
            }
            let Some(frame) = network.try_receive() else {
                break AggregateBuildStop::QueueEmpty;
            };
            self.push_candidate(frame, network, admission)?;
        };
        let cookie = self.cookie.ok_or(AggregateTxError::MissingCookie)?;
        let aggregate = self.ampdu.active().prepared_aggregate(cookie)?;
        prepared.aggregate_length = aggregate.bytes;
        prepared.original_subframes = aggregate.subframes;
        prepared.build_stop = build_stop;
        prepared.retry = AmpduRetryState::<SLOTS>::new(
            prepared.first_sequence,
            aggregate.subframes,
            AmpduRetryPolicy {
                attempt_limit: self.config.attempt_limit,
                retain_single_mpdu: matches!(self.config.rate, TxPhyRate::He(_))
                    || self.block_ack_amsdu(DATA_TID),
            },
        )?;
        Ok(prepared)
    }

    fn observe_prepared(&self, prepared: &AggregatePrepared<SLOTS>) {
        if let Some(observer) = self.observer {
            observer.observe(AggregateTxObservation::Prepared {
                subframes: prepared.original_subframes,
                stop: prepared.build_stop,
            });
        }
    }

    fn activate_prepared(
        &mut self,
        prepared: AggregatePrepared<SLOTS>,
    ) -> Result<(), AggregateTxError> {
        let config =
            self.publication_config(prepared.aggregate_length, prepared.original_subframes)?;
        self.active = ConnectedTxActive::Aggregate(AggregateActive {
            config,
            retry: prepared.retry,
            original_subframes: prepared.original_subframes,
            deadline_micros: 0,
            first_publication_micros: None,
        });
        Ok(())
    }

    /// Fill the software-owned second arena after the current aggregate has
    /// already been published. No descriptor from this arena becomes visible
    /// to MAC hardware at this edge.
    fn prepare_standby(
        &mut self,
        network: &PinnedTxConsumer<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
    ) {
        if !self.can_prepare_network_tx() {
            return;
        }
        let minimum_frames = if matches!(self.config.rate, TxPhyRate::Ht(_)) {
            2
        } else {
            1
        };
        if network.queue_len() < minimum_frames
            || !matches!(
                self.first_frame_fits_fresh_aggregate(FRAME_CAPACITY),
                Ok(true)
            )
        {
            return;
        }
        let Some(first) = network.try_receive() else {
            return;
        };

        self.prepare_network_standby(first, network);
    }

    pub(super) fn can_prepare_network_tx(&self) -> bool {
        // The second arena pipelines only the batch after an aggregate that
        // is already hardware-owned. An ordinary transaction may be a
        // control frame sharing sequence/key policy with this owner; letting
        // network preparation cross that boundary would mutate the next data
        // batch while connected control still owns the transaction.
        let base = matches!(self.active, ConnectedTxActive::Aggregate(_))
            && self.ampdu.has_standby()
            && self.standby_error.is_none()
            && self.block_ack_operational(DATA_TID)
            && !matches!(self.config.rate, TxPhyRate::Legacy(_));
        if !base {
            return false;
        }
        match self.standby_prepared.as_ref() {
            // The scheduler consumes a typed network frame only after this
            // predicate succeeds. Prove the largest frame admitted by that
            // queue fits a fresh aggregate before ownership crosses that
            // boundary; `prepare_network_standby` can then use `FreshExact`
            // without a lossy error path.
            None => matches!(
                self.first_frame_fits_fresh_aggregate(FRAME_CAPACITY),
                Ok(true)
            ),
            Some(_) => {
                let frame_limit = self.aggregate_frame_limit(DATA_TID);
                self.ampdu.standby().is_some_and(|owner| {
                    owner.held_backing_count() < frame_limit && owner.held_backing_count() < SLOTS
                }) && matches!(self.can_push_standby(FRAME_CAPACITY), Ok(true))
            }
        }
    }

    fn can_push_standby(&self, ethernet_length: usize) -> Result<bool, AggregateTxError> {
        let ampdu = self
            .ampdu
            .standby()
            .ok_or(AggregateTxError::InvalidPublicationState)?;
        let cookie = self.standby_cookie.ok_or(AggregateTxError::MissingCookie)?;
        let frame_length = ethernet_length
            .checked_add(STA_PROTECTED_QOS_ETHERNET_OVERHEAD)
            .ok_or(AggregateTxError::BufferSizeOverflow)?;
        let dma_capacity = HEADROOM + FRAME_CAPACITY + TRAILER;
        let hardware_mic_length = open_esp_radio_esp32s31_wifi::ordinary_tx::TX_CCMP_MIC_SIZE as u8;
        match self.config.rate {
            TxPhyRate::Ht(rate) => Ok(ampdu.can_commit_referenced_ht_frame(
                cookie,
                frame_length,
                hardware_mic_length,
                0,
                rate,
                dma_capacity,
            )?),
            TxPhyRate::He(rate) => Ok(ampdu.can_commit_referenced_he_frame(
                cookie,
                AmpduFrameSize::new(frame_length, hardware_mic_length),
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

    pub(super) fn prepare_network_standby(
        &mut self,
        first: PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
        network: &PinnedTxConsumer<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
    ) {
        if !self.can_prepare_network_tx() {
            drop(first);
            return;
        }

        let started = self.observer.map(|_| self.ordinary.now_micros());
        assert!(
            self.ampdu.swap_active_standby(),
            "standby presence checked before preparation"
        );
        core::mem::swap(&mut self.cookie, &mut self.standby_cookie);
        let previous = self.standby_prepared.take();
        let extending = previous.is_some();
        let result = match previous {
            Some(prepared) => self.extend_reserved(first, network, prepared),
            None => self.prepare_aggregate(first, network),
        };
        let elapsed = started.map(|started| self.ordinary.now_micros().wrapping_sub(started));
        if result.is_err() && self.cookie.is_some() {
            self.cancel_current_reservation();
        }
        core::mem::swap(&mut self.cookie, &mut self.standby_cookie);
        assert!(
            self.ampdu.swap_active_standby(),
            "temporary standby selection preserves both arenas"
        );

        match result {
            Ok(mut prepared) => {
                prepared.preparation_micros = prepared
                    .preparation_micros
                    .wrapping_add(elapsed.unwrap_or(0));
                self.standby_prepared = Some(prepared);
                if !extending && let Some(observer) = self.observer {
                    observer.observe(AggregateTxObservation::StandbyPrepared);
                }
            }
            Err(error) => self.standby_error = Some(error),
        }
    }

    pub(super) fn start_prepared_network<H: HtAmpduHardware>(
        &mut self,
        hardware: &mut H,
        network: &PinnedTxConsumer<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
    ) -> Result<WifiTxProgress, AggregateTxError> {
        if self.active() {
            return Err(AggregateTxError::ActiveTransaction);
        }
        if let Some(error) = self.standby_error.take() {
            return Err(error);
        }
        let prepared = self
            .standby_prepared
            .take()
            .ok_or(AggregateTxError::InvalidPublicationState)?;
        self.observe_prepared(&prepared);
        if let Some(observer) = self.observer {
            observer.observe(AggregateTxObservation::PreparationCompleted {
                micros: prepared.preparation_micros,
            });
        }
        assert!(
            self.ampdu.swap_active_standby(),
            "prepared standby retains its arena"
        );
        core::mem::swap(&mut self.cookie, &mut self.standby_cookie);
        if let Some(observer) = self.observer {
            observer.observe(AggregateTxObservation::StandbyPublished);
        }
        self.activate_prepared(prepared)?;
        let progress = self.publish_initial(hardware)?;
        self.prepare_standby(network);
        Ok(progress)
    }

    fn can_push(&self, ethernet_length: usize) -> Result<bool, AggregateTxError> {
        let cookie = self.cookie.ok_or(AggregateTxError::MissingCookie)?;
        let frame_length = ethernet_length
            .checked_add(STA_PROTECTED_QOS_ETHERNET_OVERHEAD)
            .ok_or(AggregateTxError::BufferSizeOverflow)?;
        let dma_capacity = HEADROOM + FRAME_CAPACITY + TRAILER;
        let hardware_mic_length = open_esp_radio_esp32s31_wifi::ordinary_tx::TX_CCMP_MIC_SIZE as u8;
        let frame_size = AmpduFrameSize::new(frame_length, hardware_mic_length);
        match self.config.rate {
            TxPhyRate::Ht(rate) => Ok(self.ampdu.active().can_commit_referenced_ht_frame(
                cookie,
                frame_length,
                hardware_mic_length,
                0,
                rate,
                dma_capacity,
            )?),
            TxPhyRate::He(rate) => Ok(self.ampdu.active().can_commit_referenced_he_frame(
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

    fn can_push_amsdu_pair(
        &self,
        first_ethernet_length: usize,
        second_ethernet_length: usize,
    ) -> Result<bool, AggregateTxError> {
        let cookie = self.cookie.ok_or(AggregateTxError::MissingCookie)?;
        let frame_length =
            sta_protected_amsdu_pair_frame_length(first_ethernet_length, second_ethernet_length)
                .map_err(AggregateTxError::Encode)?;
        let dma_capacity = HEADROOM + FRAME_CAPACITY + TRAILER;
        let hardware_mic_length = open_esp_radio_esp32s31_wifi::ordinary_tx::TX_CCMP_MIC_SIZE as u8;
        let frame_size = AmpduFrameSize::new(frame_length, hardware_mic_length);
        match self.config.rate {
            TxPhyRate::Ht(rate) => Ok(self.ampdu.active().can_commit_referenced_ht_frame(
                cookie,
                frame_length,
                hardware_mic_length,
                0,
                rate,
                dma_capacity,
            )?),
            TxPhyRate::He(rate) => Ok(self.ampdu.active().can_commit_referenced_he_frame(
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

    fn push_candidate(
        &mut self,
        first: PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
        network: &PinnedTxConsumer<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
        admission: AggregateFrameAdmission,
    ) -> Result<(), AggregateTxError> {
        if self.block_ack_amsdu(DATA_TID)
            && self.can_push_amsdu_pair(first.ethernet_length(), FRAME_CAPACITY)?
            && let Some(second) = network.try_receive()
        {
            return self.push_amsdu_pair(first, second);
        }
        self.push_frame(first, admission)
    }

    fn push_amsdu_pair(
        &mut self,
        mut first: PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
        second: PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
    ) -> Result<(), AggregateTxError> {
        if self.ampdu.active().held_backing_count() >= SLOTS {
            return Err(HtAmpduTxError::AggregateFull.into());
        }
        let metadata = self
            .ordinary
            .take_protected_metadata(DATA_TID)
            .ok_or(AggregateTxError::MissingQosSequence(DATA_TID))?;
        let ethernet_offset = first.ethernet_offset();
        let ethernet_length = first.ethernet_length();
        let encoded = metadata
            .encode_amsdu_pair_in_place(
                first.storage_mut(),
                ethernet_offset,
                ethernet_length,
                second.ethernet(),
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
        let hardware_mic_length = open_esp_radio_esp32s31_wifi::ordinary_tx::TX_CCMP_MIC_SIZE as u8;
        let layout = AmpduFrameLayout::new(
            dma_offset,
            AmpduFrameSize::new(encoded.length, hardware_mic_length),
        )
        .ok_or(AggregateTxError::DmaPrefixGeometry {
            encoded_offset: encoded.offset,
            metadata_size,
        })?;
        match self.config.rate {
            TxPhyRate::Ht(rate) => self.ampdu.active_mut().commit_ht(
                cookie,
                first,
                HtAmpduFrameRequest::new(layout, 0, rate),
            )?,
            TxPhyRate::He(rate) => self.ampdu.active_mut().commit_he(
                cookie,
                first,
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
        drop(second);
        Ok(())
    }

    fn push_frame(
        &mut self,
        mut frame: PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
        admission: AggregateFrameAdmission,
    ) -> Result<(), AggregateTxError> {
        if self.ampdu.active().held_backing_count() >= SLOTS
            || (admission == AggregateFrameAdmission::NeedsExactCheck
                && !self.can_push(frame.ethernet_length())?)
        {
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
        let hardware_mic_length = open_esp_radio_esp32s31_wifi::ordinary_tx::TX_CCMP_MIC_SIZE as u8;
        let frame_size = AmpduFrameSize::new(encoded.length, hardware_mic_length);
        let layout = AmpduFrameLayout::new(dma_offset, frame_size).ok_or(
            AggregateTxError::DmaPrefixGeometry {
                encoded_offset: encoded.offset,
                metadata_size,
            },
        )?;
        match self.config.rate {
            TxPhyRate::Ht(rate) => self.ampdu.active_mut().commit_ht(
                cookie,
                frame,
                HtAmpduFrameRequest::new(layout, 0, rate),
            )?,
            TxPhyRate::He(rate) => self.ampdu.active_mut().commit_he(
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
                let role_policy = self
                    .ht_role_policy(DATA_TID)?
                    .ok_or(AggregateTxError::InvalidPublicationState)?;
                let data_power = self
                    .ordinary
                    .power_profile()
                    .power_pair(rate.power_lookup_code());
                let rts_power = self
                    .ordinary
                    .power_profile()
                    .power_pair(rate.vendor_rts_rate().code());
                let config = ht_ampdu_publication_config(
                    role_policy.role(),
                    HtAmpduPublicationInputs {
                        rate: role_policy.rate(),
                        aggregate_length,
                        subframes,
                        protection_spacing: self.ordinary.policy().ht_ampdu().protection_spacing(),
                        data_power_primary: data_power.primary as u8,
                        data_power_alternate: data_power.alternate as u8,
                        rts_power_primary: rts_power.primary as u8,
                        rts_power_alternate: rts_power.alternate as u8,
                        aifsn: contention.aifsn(),
                        contention_window,
                        scheduler_priority: queue.vendor_data_scheduler_priority(),
                        packet_priority: queue.vendor_data_packet_priority(),
                    },
                )
                .ok_or(AggregateTxError::BufferSizeOverflow)?;
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
            AmpduTxConfig::Ht(config) => self.ampdu.active_mut().submit(
                hardware,
                self.cookie.ok_or(AggregateTxError::MissingCookie)?,
                LegacyTxQueue::BestEffort,
                config,
            )?,
            AmpduTxConfig::He(config) => self.ampdu.active_mut().submit_he(
                hardware,
                self.cookie.ok_or(AggregateTxError::MissingCookie)?,
                LegacyTxQueue::BestEffort,
                config,
            )?,
        }
        if let Some(observer) = self.observer {
            let publication_finished = self.ordinary.now_micros();
            if active.first_publication_micros.is_none() {
                active.first_publication_micros = Some(publication_started);
            }
            observer.observe(AggregateTxObservation::Published {
                at_micros: publication_started,
                program_micros: publication_finished.wrapping_sub(publication_started),
            });
        }
        active.deadline_micros = deadline;
        Ok(())
    }
}
