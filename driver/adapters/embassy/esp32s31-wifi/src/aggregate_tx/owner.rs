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
    pub fn new(
        ordinary: Esp32s31SingleMpduTx<'slot, P, E, T, ORDINARY_BUFFER_SIZE>,
        ampdu: AggregateTxResources<
            'ampdu,
            PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
            SLOTS,
            AMPDU_BUFFER_SIZE,
        >,
        config: AggregateTxConfig,
    ) -> Result<Self, AggregateTxError> {
        if SLOTS == 0
            || SLOTS > 32
            || config.frame_limit == 0
            || usize::from(config.frame_limit) > SLOTS
        {
            return Err(AggregateTxError::InvalidFrameLimit {
                limit: config.frame_limit,
                capacity: SLOTS,
            });
        }
        if !ordinary.config().peer_qos {
            return Err(AggregateTxError::PeerDoesNotSupportQos);
        }
        if config.attempt_limit == 0 {
            return Err(AmpduRetryError::ZeroAttemptLimit.into());
        }
        let AggregateTxResources {
            primary,
            primary_retention,
            standby,
            standby_retention,
        } = ampdu;
        let mut ampdu = RetainedDmaAmpduTx::new(primary, primary_retention);
        ampdu.configure_max_aggregate_bytes(
            ordinary.policy().ht_ampdu().maximum_aggregate_bytes(),
        )?;
        let standby_ampdu = standby
            .zip(standby_retention)
            .map(|(resources, retention)| {
                let mut owner = RetainedDmaAmpduTx::new(resources, retention);
                owner
                    .configure_max_aggregate_bytes(
                        ordinary.policy().ht_ampdu().maximum_aggregate_bytes(),
                    )
                    .expect("idle standby arena accepts validated association byte limit");
                owner
            });
        Ok(Self {
            ordinary: TeardownResource::new(ordinary),
            ampdu: TeardownResource::new(ampdu),
            standby_ampdu,
            cookie: None,
            standby_cookie: None,
            standby_prepared: None,
            standby_error: None,
            block_ack_windows: [0; 8],
            config,
            active: ConnectedTxActive::Idle,
            last_aggregate_status: None,
            pending_ordinary_retry: None,
            observer: None,
        })
    }

    /// Attach optional observations without changing TX
    /// scheduling or completion ownership.
    pub fn with_observer(mut self, observer: &'ampdu dyn AggregateTxObserver) -> Self {
        self.observer = Some(observer);
        self
    }

    pub fn ordinary(&self) -> &Esp32s31SingleMpduTx<'slot, P, E, T, ORDINARY_BUFFER_SIZE> {
        &self.ordinary
    }

    pub fn ordinary_mut(
        &mut self,
    ) -> &mut Esp32s31SingleMpduTx<'slot, P, E, T, ORDINARY_BUFFER_SIZE> {
        &mut self.ordinary
    }

    pub fn set_block_ack_window(&mut self, tid: u8, window: Option<u16>) {
        self.set_block_ack_agreement(tid, window.map(|window| (window, false)));
    }

    pub fn set_block_ack_agreement(&mut self, tid: u8, agreement: Option<(u16, bool)>) {
        // The S31 capability bounds negotiated TX windows to 32. Keep the
        // hot owner at the former boolean table size by storing the A-MSDU
        // capability in bit 7; an impossible wider value disables
        // aggregation instead of truncating the agreement.
        let window = agreement
            .map(|(window, _)| window)
            .and_then(|window| u8::try_from(window).ok())
            .filter(|window| *window <= 32)
            .unwrap_or(0);
        let amsdu = agreement.is_some_and(|(_, amsdu)| amsdu);
        if let Some(entry) = self.block_ack_windows.get_mut(usize::from(tid)) {
            let encoded = window | if amsdu && window != 0 { 0x80 } else { 0 };
            if *entry == encoded {
                return;
            }
            let was_operational = *entry & 0x7f != 0;
            *entry = encoded;
            let operational = window != 0;
            if let Some(observer) = self.observer {
                if was_operational != operational {
                    observer
                        .observe(AggregateTxObservation::BlockAckOperational { tid, operational });
                }
            }
        }
    }

    pub fn block_ack_amsdu(&self, tid: u8) -> bool {
        self.block_ack_windows
            .get(usize::from(tid))
            .is_some_and(|agreement| agreement & 0x80 != 0)
    }

    pub fn block_ack_operational(&self, tid: u8) -> bool {
        self.block_ack_window(tid).is_some()
    }

    pub fn block_ack_window(&self, tid: u8) -> Option<u16> {
        let window = self
            .block_ack_windows
            .get(usize::from(tid))
            .copied()
            .unwrap_or(0)
            & 0x7f;
        (window != 0).then_some(u16::from(window))
    }

    pub(super) fn aggregate_frame_limit(&self, tid: u8) -> usize {
        self.block_ack_window(tid).map_or(0, |window| {
            usize::from(window).min(usize::from(self.config.frame_limit))
        })
    }

    /// Take one terminal HMAC-visible aggregate exchange status.
    ///
    /// When HT retry policy detaches one missing MPDU into the ordinary owner,
    /// this remains `None` until that ordinary retry also reaches a terminal
    /// status.  Callers therefore cannot accidentally treat the intermediate
    /// BlockAck as completion of the logical exchange.
    pub fn take_last_aggregate_status(&mut self) -> Option<MacAmpduTxStatus<TxPhyRate>> {
        self.last_aggregate_status.take()
    }

    pub fn take_last_ordinary_outcome(&mut self) -> Option<SingleMpduTxOutcome> {
        self.ordinary.take_last_outcome()
    }

    pub fn active(&self) -> bool {
        !matches!(self.active, ConnectedTxActive::Idle)
    }

    pub fn has_prepared_network_tx(&self) -> bool {
        self.standby_prepared.is_some() || self.standby_error.is_some()
    }

    pub fn preferred_network_batch_size(&self) -> usize {
        self.aggregate_frame_limit(DATA_TID).max(1)
    }

    pub fn prepared_network_frame_count(&self) -> usize {
        self.standby_prepared
            .as_ref()
            .map_or(0, |prepared| usize::from(prepared.original_subframes))
    }

    /// Portable queue state across the aggregate and ordinary descriptor
    /// owners that form one connected TX service.
    pub fn queue_state(&self) -> MacTxQueueState {
        if self.ampdu.as_ref().get_ref().state() == TxSlotState::ResetRequired
            || self.ordinary.queue_state() == MacTxQueueState::ResetRequired
        {
            MacTxQueueState::ResetRequired
        } else if self.active()
            || self.has_prepared_network_tx()
            || self.ordinary.queue_state() == MacTxQueueState::Backpressured
        {
            MacTxQueueState::Backpressured
        } else {
            MacTxQueueState::Ready
        }
    }

    /// Exact ordinary descriptor state for ownership-failure diagnostics.
    pub fn ordinary_slot_state(&self) -> TxSlotState {
        self.ordinary.slot_state()
    }

    /// Hardware-visible ownership word of the ordinary descriptor.
    pub fn ordinary_descriptor_word0(&self) -> u32 {
        self.ordinary.descriptor_word0()
    }

    /// Primary aggregate metadata lifecycle state.
    pub fn aggregate_slot_state(&self) -> TxSlotState {
        self.ampdu.state()
    }

    pub fn aggregate_slot_state_code(&self) -> u8 {
        match self.ampdu.state() {
            TxSlotState::Free => 0,
            TxSlotState::Reserved => 1,
            TxSlotState::HardwareOwned => 2,
            TxSlotState::Completed => 3,
            TxSlotState::ResetRequired => 4,
        }
    }

    pub fn aggregate_metadata_is_free(&self) -> bool {
        self.ampdu.state() == TxSlotState::Free
    }

    /// Whether the primary aggregate DMA arena is independently idle.
    pub fn aggregate_dma_is_free(&self) -> bool {
        self.ampdu.dma_is_free()
    }

    /// Standby aggregate metadata/DMA idle state when pipelining is present.
    pub fn standby_aggregate_is_fully_free(&self) -> Option<bool> {
        self.standby_ampdu
            .as_ref()
            .map(|standby| standby.is_fully_free())
    }

    pub fn aggregate_held_backings(&self) -> usize {
        self.ampdu.held_backing_count()
    }

    pub fn aggregate_metadata_address(&self) -> usize {
        self.ampdu.metadata_address()
    }

    /// Whether either hardware-visible TX owner has been quarantined pending
    /// a platform radio reset.
    ///
    /// This is intentionally observational: a caller may report the terminal
    /// frontier, but cannot turn it back into an idle owner without the
    /// platform reset transaction.
    pub fn is_reset_required(&self) -> bool {
        self.queue_state() == MacTxQueueState::ResetRequired
    }

    /// Recover the ordinary connected owner and descriptor-only aggregate
    /// storage after every referenced network lease has been released.
    ///
    /// An active or partially detached aggregate is returned intact. Losing
    /// that value would leak pinned `embassy-net` leases or make DMA lifetime
    /// unknowable to an outer reconnect owner.
    #[allow(clippy::result_large_err, clippy::type_complexity)]
    pub fn try_into_parts(
        mut self,
    ) -> Result<
        (
            Esp32s31SingleMpduTx<'slot, P, E, T, ORDINARY_BUFFER_SIZE>,
            AggregateTxResources<
                'ampdu,
                PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
                SLOTS,
                AMPDU_BUFFER_SIZE,
            >,
        ),
        Self,
    > {
        if self.active()
            || self.ordinary.active()
            || self.ordinary.queue_state() != MacTxQueueState::Ready
            || !self.ampdu.is_fully_free()
            || self.cookie.is_some()
            || self.standby_prepared.is_some()
            || self.standby_cookie.is_some()
            || self.standby_error.is_some()
            || self
                .standby_ampdu
                .as_ref()
                .is_some_and(|owner| !owner.is_fully_free())
        {
            return Err(self);
        }
        let ordinary = self.ordinary.take();
        let (ampdu, primary_retention) = match self.ampdu.take().try_into_resources() {
            Ok(resources) => resources,
            Err(_) => unreachable!("idle retained DMA owner must return its storage"),
        };
        let standby = self.standby_ampdu.take().map(|owner| {
            owner
                .try_into_resources()
                .unwrap_or_else(|_| unreachable!("idle standby owner must return its storage"))
        });
        let (standby, standby_retention) = standby.map_or((None, None), |(storage, retention)| {
            (Some(storage), Some(retention))
        });
        Ok((
            ordinary,
            AggregateTxResources {
                primary: ampdu,
                primary_retention,
                standby,
                standby_retention,
            },
        ))
    }

    /// Return every idle connected-TX resource needed by station teardown.
    ///
    /// This composes aggregate and ordinary ownership into one fail-closed
    /// transition. The caller does not need to know that the pairwise key and
    /// sequence spaces are nested inside the ordinary fallback owner.
    #[allow(clippy::result_large_err)]
    pub fn try_into_teardown_parts(
        self,
    ) -> Result<
        (
            WifiTxResources<'slot, P, E, T, ORDINARY_BUFFER_SIZE>,
            ConnectedTxHandoff,
            AggregateTxResources<
                'ampdu,
                PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
                SLOTS,
                AMPDU_BUFFER_SIZE,
            >,
        ),
        Self,
    > {
        let (ordinary, ampdu) = self.try_into_parts()?;
        // `try_into_parts` checks both aggregate and ordinary active state
        // before transferring either field. No executor or hardware actor can
        // mutate the uniquely owned ordinary value between these two calls.
        let (resources, handoff) = match ordinary.try_into_parts() {
            Ok(parts) => parts,
            Err(_) => unreachable!("aggregate idle invariant admitted an active ordinary TX"),
        };
        Ok((resources, handoff, ampdu))
    }

    /// Return a named station-lifecycle owner instead of exposing the nested
    /// ordinary-TX handoff representation to application/HIL code.
    #[allow(clippy::result_large_err, clippy::type_complexity)]
    pub fn try_into_station_parts(
        self,
    ) -> Result<
        Esp32s31ConnectedTxTeardownParts<
            WifiTxResources<'slot, P, E, T, ORDINARY_BUFFER_SIZE>,
            AggregateTxResources<
                'ampdu,
                PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
                SLOTS,
                AMPDU_BUFFER_SIZE,
            >,
        >,
        Self,
    > {
        let (resources, handoff, aggregate) = self.try_into_teardown_parts()?;
        let ConnectedTxHandoff {
            key,
            sequences,
            config: _,
        } = handoff;
        Ok(Esp32s31ConnectedTxTeardownParts {
            resources,
            pairwise_key: key,
            sequences,
            aggregate,
        })
    }

    pub async fn wait_deadline(&mut self) {
        match &self.active {
            ConnectedTxActive::Aggregate(active) => {
                self.ordinary
                    .wait_until_micros(active.deadline_micros)
                    .await;
            }
            ConnectedTxActive::Idle | ConnectedTxActive::Ordinary => {
                self.ordinary.wait_deadline().await;
            }
        }
    }
}
