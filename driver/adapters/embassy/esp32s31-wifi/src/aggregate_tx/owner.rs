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
        ampdu: HtAmpduTxResources<'ampdu, SLOTS, AMPDU_BUFFER_SIZE>,
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
        let mut ampdu = RetainedDmaAmpduTx::new(ampdu);
        ampdu.configure_max_aggregate_bytes(
            ordinary.policy().ht_ampdu().maximum_aggregate_bytes(),
        )?;
        Ok(Self {
            ordinary: TeardownResource::new(ordinary),
            ampdu: TeardownResource::new(ampdu),
            cookie: None,
            block_ack_operational: [false; 8],
            config,
            active: ConnectedTxActive::Idle,
            last_aggregate_status: None,
            pending_ordinary_retry: None,
            counters: None,
        })
    }

    /// Attach optional production/HIL observations without changing TX
    /// scheduling or completion ownership.
    pub fn with_counters(mut self, counters: &'ampdu AggregateTxCounters) -> Self {
        self.counters = Some(counters);
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

    pub fn set_block_ack_operational(&mut self, tid: u8, operational: bool) {
        if let Some(entry) = self.block_ack_operational.get_mut(usize::from(tid)) {
            *entry = operational;
        }
    }

    pub fn block_ack_operational(&self, tid: u8) -> bool {
        self.block_ack_operational
            .get(usize::from(tid))
            .copied()
            .unwrap_or(false)
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

    /// Portable queue state across the aggregate and ordinary descriptor
    /// owners that form one connected TX service.
    pub fn queue_state(&self) -> MacTxQueueState {
        if self.ampdu.as_ref().get_ref().state() == TxSlotState::ResetRequired
            || self.ordinary.queue_state() == MacTxQueueState::ResetRequired
        {
            MacTxQueueState::ResetRequired
        } else if self.active() || self.ordinary.queue_state() == MacTxQueueState::Backpressured {
            MacTxQueueState::Backpressured
        } else {
            MacTxQueueState::Ready
        }
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
    #[allow(clippy::result_large_err)]
    pub fn try_into_parts(
        mut self,
    ) -> Result<
        (
            Esp32s31SingleMpduTx<'slot, P, E, T, ORDINARY_BUFFER_SIZE>,
            HtAmpduTxResources<'ampdu, SLOTS, AMPDU_BUFFER_SIZE>,
        ),
        Self,
    > {
        if self.active()
            || self.ordinary.active()
            || self.ampdu.held_backing_count() != 0
            || self.cookie.is_some()
        {
            return Err(self);
        }
        let ordinary = self.ordinary.take();
        let ampdu = match self.ampdu.take().try_into_resources() {
            Ok(resources) => resources,
            Err(_) => unreachable!("idle retained DMA owner must return its storage"),
        };
        Ok((ordinary, ampdu))
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
            HtAmpduTxResources<'ampdu, SLOTS, AMPDU_BUFFER_SIZE>,
        ),
        Self,
    > {
        let (ordinary, ampdu) = match self.try_into_parts() {
            Ok(parts) => parts,
            Err(owner) => return Err(owner),
        };
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
            HtAmpduTxResources<'ampdu, SLOTS, AMPDU_BUFFER_SIZE>,
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
