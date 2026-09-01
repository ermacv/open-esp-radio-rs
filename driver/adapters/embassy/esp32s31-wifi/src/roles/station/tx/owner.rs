#![expect(
    clippy::type_complexity,
    reason = "TX shutdown exposes the exact ordinary and aggregate owner graph"
)]

use super::*;

#[cfg(feature = "tx-egress-scheduling")]
fn single_station_ht_demand(
    demand: WifiEgressDemand<open_esp_radio_embassy_net::EgressKey>,
) -> Result<(), DatapathEgressSnapshotRejection> {
    let Some(decoded) = DecodedEgressKey::decode(*demand.key()) else {
        return Err(DatapathEgressSnapshotRejection::Key);
    };
    let DecodedEgressKey::SingleRadioPeer {
        interface,
        schedule_epoch,
        traffic_class,
    } = decoded
    else {
        return Err(DatapathEgressSnapshotRejection::Key);
    };
    if interface != demand.vif() || schedule_epoch != demand.id().schedule_epoch() {
        return Err(DatapathEgressSnapshotRejection::Identity);
    }
    if traffic_class != 0 {
        return Err(DatapathEgressSnapshotRejection::TrafficClass);
    }
    Ok(())
}

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
        rate_control: StaRateControlAssociation,
        aggregate_rate_policy: StaTxRatePolicy,
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
            ampdu: TeardownResource::new(AggregateTxArenaPair::new(ampdu, standby_ampdu)),
            cookie: None,
            standby_cookie: None,
            standby_prepared: None,
            standby_error: None,
            deferred_network: None,
            block_ack_windows: [0; 8],
            block_ack_generations: [0; 8],
            block_ack_generation_exhausted: 0,
            config,
            rate_control: TeardownResource::new(rate_control),
            aggregate_rate_policy,
            he_trigger_based: None,
            active: ConnectedTxActive::Idle,
            last_aggregate_status: None,
            pending_ordinary_retry: None,
            #[cfg(any(feature = "diagnostics", test))]
            observer: None,
            block_ack_status_sink: None,
        })
    }

    #[cfg(test)]
    pub(super) fn new_for_test(
        ordinary: Esp32s31SingleMpduTx<'slot, P, E, T, ORDINARY_BUFFER_SIZE>,
        ampdu: AggregateTxResources<
            'ampdu,
            PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
            SLOTS,
            AMPDU_BUFFER_SIZE,
        >,
        config: AggregateTxConfig,
    ) -> Result<Self, AggregateTxError> {
        use open_esp_radio_esp32s31_wifi_mac::rate_control::{
            HeLowMetricReportFeatures, StaLinkMetric, StaRateControlAssociationInput,
            StaRateControlPhy,
        };
        use open_esp_radio_ieee80211::{he::HeDcmConstellation, station::StaAssociationPhy};

        let (
            association_phy,
            phy,
            ht_mcs_override,
            ht_guard_interval_override,
            he_mcs_override,
            he_guard_interval_and_ltf_override,
        ) = match config.rate {
            TxPhyRate::Ht(rate) => (
                match rate.channel_width {
                    open_esp_radio_esp32s31_wifi_mac::tx::HtChannelWidth::Mhz20 => {
                        StaAssociationPhy::Ht20
                    }
                    open_esp_radio_esp32s31_wifi_mac::tx::HtChannelWidth::Mhz40 => {
                        StaAssociationPhy::Ht40
                    }
                },
                StaRateControlPhy::Ht,
                Some(rate.mcs),
                Some(rate.guard_interval),
                None,
                None,
            ),
            TxPhyRate::He(rate) => (
                StaAssociationPhy::He20,
                StaRateControlPhy::He,
                None,
                None,
                Some(rate.mcs()),
                Some(rate.guard_interval_and_ltf()),
            ),
            TxPhyRate::Legacy(_) => (
                StaAssociationPhy::Legacy,
                StaRateControlPhy::Dot11G,
                None,
                None,
                None,
                None,
            ),
        };
        let rate_control = StaRateControlAssociation::new(StaRateControlAssociationInput {
            phy,
            link_metric: StaLinkMetric::from_estimator(50),
            p2p: false,
            peer_highest_rate: None,
            long_range_rates_present: false,
            he_low_metric_report: HeLowMetricReportFeatures::default(),
        });
        let rate_policy = StaTxRatePolicy {
            association_phy,
            high_throughput_enabled: !matches!(config.rate, TxPhyRate::Legacy(_)),
            fallback_legacy_rate: open_esp_radio_esp32s31_wifi_mac::tx::LegacyRate::Ofdm54M,
            fallback_ht_mcs: open_esp_radio_esp32s31_wifi_mac::tx::HtMcs::Mcs7,
            fallback_ht_guard_interval:
                open_esp_radio_esp32s31_wifi_mac::tx::HtGuardInterval::Long800Ns,
            ht_mcs_override,
            ht_guard_interval_override,
            he_mcs_override,
            he_guard_interval_and_ltf_override,
            he_dcm_override: None,
            he_800ns_gi_ltf:
                open_esp_radio_esp32s31_wifi_mac::rx::HeGuardIntervalAndLtf::TwoLtf800Ns,
            peer_supports_ht_short_guard_interval: true,
            peer_supports_ldpc: false,
            peer_dcm_receive: HeDcmConstellation::NotSupported,
        };
        Self::new(ordinary, ampdu, config, rate_control, rate_policy)
    }

    /// Attach optional observations without changing TX
    /// scheduling or completion ownership.
    #[cfg(any(feature = "diagnostics", test))]
    pub fn with_observer(mut self, observer: &'ampdu dyn AggregateTxObserver) -> Self {
        self.observer = Some(observer);
        self
    }

    /// Publish only negotiated BlockAck state changes to the application
    /// status boundary. This does not enable aggregate diagnostics.
    pub fn with_block_ack_status_sink(mut self, sink: StationTxBlockAckStatusSink) -> Self {
        self.block_ack_status_sink = Some(sink);
        self
    }

    /// Prepare every fresh HE A-MPDU queue for AP-triggered uplink service.
    ///
    /// The setting remains dormant while rate control selects HT or legacy.
    /// It only installs the already recovered queue/MPLEN/BSR transaction; it
    /// does not fabricate a trigger or turn the immediately submitted HE-SU
    /// aggregate into a TB PPDU.
    pub fn with_he_trigger_based(mut self, trigger_based: Option<HeTriggerBasedTxConfig>) -> Self {
        self.he_trigger_based = trigger_based;
        self
    }

    pub const fn he_trigger_based(&self) -> Option<HeTriggerBasedTxConfig> {
        self.he_trigger_based
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
        let agreement = if self.ordinary.security_mode()
            == open_esp_radio_ieee80211::security::WifiSecurityMode::Open
        {
            None
        } else {
            agreement
        };
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
            let generation_bit = 1_u8 << tid;
            let encoded = window | if amsdu && window != 0 { 0x80 } else { 0 };
            if *entry == encoded {
                return;
            }
            let was_exhausted = self.block_ack_generation_exhausted & generation_bit != 0;
            let was_operational = !was_exhausted && *entry & 0x7f != 0;
            *entry = encoded;
            if !was_exhausted {
                match self.block_ack_generations[usize::from(tid)].checked_add(1) {
                    Some(generation) => {
                        self.block_ack_generations[usize::from(tid)] = generation;
                    }
                    None => self.block_ack_generation_exhausted |= generation_bit,
                }
            }
            let operational =
                self.block_ack_generation_exhausted & generation_bit == 0 && window != 0;
            if was_operational != operational
                && let Some(sink) = self.block_ack_status_sink
            {
                sink(tid, operational);
            }
            #[cfg(any(feature = "diagnostics", test))]
            if let Some(observer) = self.observer
                && was_operational != operational
            {
                observer.observe(AggregateTxObservation::BlockAckOperational { tid, operational });
            }
        }
    }

    pub fn block_ack_amsdu(&self, tid: u8) -> bool {
        self.block_ack_generation(tid).is_some()
            && self
                .block_ack_windows
                .get(usize::from(tid))
                .is_some_and(|agreement| agreement & 0x80 != 0)
    }

    pub fn block_ack_operational(&self, tid: u8) -> bool {
        self.block_ack_window(tid).is_some()
    }

    pub fn block_ack_window(&self, tid: u8) -> Option<u16> {
        self.block_ack_generation(tid)?;
        let window = self
            .block_ack_windows
            .get(usize::from(tid))
            .copied()
            .unwrap_or(0)
            & 0x7f;
        (window != 0).then_some(u16::from(window))
    }

    /// Revalidate the sole station egress queue against the fresh aggregate
    /// rate and current TID-0 BlockAck agreement.
    ///
    /// Traffic-class mapping is intentionally fail-closed until the generic
    /// class-to-WMM contract exists. The current Xarxa UDP path publishes
    /// class zero, which maps to the production TID-0 aggregate path.
    #[cfg(feature = "tx-egress-scheduling")]
    pub fn egress_radio_snapshot(
        &self,
        demand: WifiEgressDemand<open_esp_radio_embassy_net::EgressKey>,
    ) -> Option<DatapathHtEgressSnapshot> {
        if let Err(reason) = single_station_ht_demand(demand) {
            return rejected_ht_egress_snapshot(reason);
        }

        let TxPhyRate::Ht(rate) = self.rate_control.ampdu_tx_rate(self.aggregate_rate_policy)
        else {
            return rejected_ht_egress_snapshot(DatapathEgressSnapshotRejection::NonHtRate);
        };
        let Some(block_ack_window) = self.block_ack_window(0) else {
            return rejected_ht_egress_snapshot(DatapathEgressSnapshotRejection::NoBlockAck);
        };
        let maximum_frames = usize::from(block_ack_window)
            .min(usize::from(self.config.frame_limit))
            .min(SLOTS);
        let Some(maximum_frames) = u8::try_from(maximum_frames)
            .ok()
            .and_then(core::num::NonZeroU8::new)
        else {
            return rejected_ht_egress_snapshot(DatapathEgressSnapshotRejection::InvalidGeometry);
        };
        Some(DatapathHtEgressSnapshot::new(
            rate,
            maximum_frames,
            FRAME_CAPACITY,
            self.ordinary.policy().ht_ampdu().maximum_aggregate_bytes(),
        ))
    }

    pub(super) fn block_ack_generation(&self, tid: u8) -> Option<u32> {
        let generation = self.block_ack_generations.get(usize::from(tid)).copied()?;
        let bit = 1_u8.checked_shl(u32::from(tid))?;
        (self.block_ack_generation_exhausted & bit == 0).then_some(generation)
    }

    pub(super) fn aggregate_frame_limit(&self, tid: u8) -> usize {
        if self.ordinary.security_mode()
            == open_esp_radio_ieee80211::security::WifiSecurityMode::Open
        {
            return 0;
        }
        if matches!(self.config.rate, TxPhyRate::Ht(_)) {
            return self
                .ht_role_policy(tid)
                .ok()
                .flatten()
                .map_or(0, |policy| usize::from(policy.frame_limit()));
        }
        self.block_ack_window(tid).map_or(0, |window| {
            usize::from(window).min(usize::from(self.config.frame_limit))
        })
    }

    pub(super) fn ht_role_policy(
        &self,
        tid: u8,
    ) -> Result<Option<HtAmpduTxRolePolicy>, AggregateTxError> {
        if self.ordinary.security_mode()
            == open_esp_radio_ieee80211::security::WifiSecurityMode::Open
        {
            return Ok(None);
        }
        let TxPhyRate::Ht(rate) = self.config.rate else {
            return Ok(None);
        };
        let Some(window) = self.block_ack_window(tid) else {
            return Ok(None);
        };
        Ok(Some(HtAmpduTxRolePolicy::new(
            AmpduTxRoleAdapter {
                interface: MacInterface::Station,
                hardware_key_selector: self.ordinary.hardware_key_selector(),
            },
            rate,
            window,
            self.config.frame_limit,
            SLOTS,
        )?))
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

    /// Network leases captured by the currently active fresh exchange.
    /// Retries keep this original count: fairness charges leases once when
    /// they leave the network queue, not once per hardware publication.
    pub fn active_network_frame_count(&self) -> usize {
        match &self.active {
            ConnectedTxActive::Idle => 1,
            ConnectedTxActive::Ordinary => 1,
            ConnectedTxActive::Aggregate(active) => usize::from(active.original_subframes),
        }
    }

    pub fn has_prepared_network_tx(&self) -> bool {
        self.deferred_network.is_some()
            || self.standby_prepared.is_some()
            || self.standby_error.is_some()
    }

    pub fn preferred_network_batch_size(&self) -> usize {
        (0_u8..8)
            .map(|tid| self.aggregate_frame_limit(tid))
            .max()
            .unwrap_or(0)
            .max(1)
    }

    pub fn prepared_network_frame_count(&self) -> usize {
        if let Some(prepared) = self.standby_prepared.as_ref() {
            // A prepared standby contains FIFO predecessors of a frame that
            // may have been deferred while extending it. Publish this arena
            // first so the retained successor cannot overtake those leases.
            return usize::from(prepared.original_subframes);
        }
        usize::from(self.deferred_network.is_some())
    }

    /// Portable queue state across the aggregate and ordinary descriptor
    /// owners that form one connected TX service.
    pub fn queue_state(&self) -> MacTxQueueState {
        if self.ampdu.active().as_ref().get_ref().state() == TxSlotState::ResetRequired
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
        self.ampdu.active().state()
    }

    pub fn aggregate_slot_state_code(&self) -> u8 {
        match self.ampdu.active().state() {
            TxSlotState::Free => 0,
            TxSlotState::Reserved => 1,
            TxSlotState::HardwareOwned => 2,
            TxSlotState::Completed => 3,
            TxSlotState::ResetRequired => 4,
        }
    }

    pub fn aggregate_metadata_is_free(&self) -> bool {
        self.ampdu.active().state() == TxSlotState::Free
    }

    /// Whether the primary aggregate DMA arena is independently idle.
    pub fn aggregate_dma_is_free(&self) -> bool {
        self.ampdu.active().dma_is_free()
    }

    /// Standby aggregate metadata/DMA idle state when pipelining is present.
    pub fn standby_aggregate_is_fully_free(&self) -> Option<bool> {
        self.ampdu.standby().map(|standby| standby.is_fully_free())
    }

    pub fn aggregate_held_backings(&self) -> usize {
        self.ampdu.active().held_backing_count()
    }

    pub fn aggregate_metadata_address(&self) -> usize {
        self.ampdu.active().metadata_address()
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

    /// Lend both physical TX owners to another logical role at a completely
    /// idle scheduling boundary while retaining every station-local policy
    /// and observation state.
    #[allow(clippy::result_large_err, clippy::type_complexity)]
    pub fn try_park(
        mut self,
    ) -> Result<
        (
            WifiTxResources<'slot, P, E, T, ORDINARY_BUFFER_SIZE>,
            AggregateTxResources<
                'ampdu,
                PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
                SLOTS,
                AMPDU_BUFFER_SIZE,
            >,
            Esp32s31ConnectedTxParked<'ampdu, SLOTS>,
        ),
        Self,
    > {
        if self.queue_state() != MacTxQueueState::Ready {
            return Err(self);
        }

        let rate_control = self.rate_control.take();
        let block_ack_windows = self.block_ack_windows;
        let block_ack_generations = self.block_ack_generations;
        let block_ack_generation_exhausted = self.block_ack_generation_exhausted;
        let config = self.config;
        let aggregate_rate_policy = self.aggregate_rate_policy;
        let he_trigger_based = self.he_trigger_based;
        let last_aggregate_status = self.last_aggregate_status;
        let pending_ordinary_retry = self.pending_ordinary_retry;
        #[cfg(any(feature = "diagnostics", test))]
        let observer = self.observer;
        let block_ack_status_sink = self.block_ack_status_sink;

        let (ordinary, aggregate) = self.try_into_parts()?;
        let (resources, ordinary) = ordinary
            .try_park()
            .unwrap_or_else(|_| unreachable!("idle ordinary owner must park its role state"));
        Ok((
            resources,
            aggregate,
            Esp32s31ConnectedTxParked {
                ordinary,
                block_ack_windows,
                block_ack_generations,
                block_ack_generation_exhausted,
                config,
                rate_control,
                aggregate_rate_policy,
                he_trigger_based,
                last_aggregate_status,
                pending_ordinary_retry,
                #[cfg(any(feature = "diagnostics", test))]
                observer,
                #[cfg(not(any(feature = "diagnostics", test)))]
                observer_lifetime: PhantomData,
                block_ack_status_sink,
            },
        ))
    }

    /// Rejoin station-local policy with the exact physical owners returned by
    /// the AP role. No association, key, sequence, rate-control or BlockAck
    /// state is reconstructed from values outside the parked capability.
    pub fn resume(
        resources: WifiTxResources<'slot, P, E, T, ORDINARY_BUFFER_SIZE>,
        aggregate: AggregateTxResources<
            'ampdu,
            PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
            SLOTS,
            AMPDU_BUFFER_SIZE,
        >,
        parked: Esp32s31ConnectedTxParked<'ampdu, SLOTS>,
    ) -> Self {
        let Esp32s31ConnectedTxParked {
            ordinary,
            block_ack_windows,
            block_ack_generations,
            block_ack_generation_exhausted,
            config,
            rate_control,
            aggregate_rate_policy,
            he_trigger_based,
            last_aggregate_status,
            pending_ordinary_retry,
            #[cfg(any(feature = "diagnostics", test))]
            observer,
            #[cfg(not(any(feature = "diagnostics", test)))]
                observer_lifetime: _,
            block_ack_status_sink,
        } = parked;
        let ordinary = Esp32s31SingleMpduTx::resume(resources, ordinary);
        let mut owner = match Self::new(
            ordinary,
            aggregate,
            config,
            rate_control,
            aggregate_rate_policy,
        ) {
            Ok(owner) => owner,
            Err(_) => unreachable!(
                "a private parked connected-TX state preserves its validated configuration"
            ),
        };
        owner.block_ack_windows = block_ack_windows;
        owner.block_ack_generations = block_ack_generations;
        owner.block_ack_generation_exhausted = block_ack_generation_exhausted;
        owner.last_aggregate_status = last_aggregate_status;
        owner.pending_ordinary_retry = pending_ordinary_retry;
        owner.he_trigger_based = he_trigger_based;
        #[cfg(any(feature = "diagnostics", test))]
        {
            owner.observer = observer;
        }
        owner.block_ack_status_sink = block_ack_status_sink;
        owner
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
            || !self.ampdu.active().is_fully_free()
            || self.cookie.is_some()
            || self.deferred_network.is_some()
            || self.standby_prepared.is_some()
            || self.standby_cookie.is_some()
            || self.standby_error.is_some()
            || self
                .ampdu
                .standby()
                .is_some_and(|owner| !owner.is_fully_free())
        {
            return Err(self);
        }
        let ordinary = self.ordinary.take();
        let (active, standby) = self.ampdu.take().into_parts();
        let (ampdu, primary_retention) = match active.try_into_resources() {
            Ok(resources) => resources,
            Err(_) => unreachable!("idle retained DMA owner must return its storage"),
        };
        let standby = standby.map(|owner| {
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
            security,
            sequences,
            config: _,
        } = handoff;
        Ok(Esp32s31ConnectedTxTeardownParts {
            resources,
            security,
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

#[cfg(feature = "tx-egress-scheduling")]
impl<const SLOTS: usize> Esp32s31ConnectedTxParked<'_, SLOTS> {
    pub fn egress_radio_snapshot(
        &self,
        demand: WifiEgressDemand<open_esp_radio_embassy_net::EgressKey>,
        maximum_ethernet_bytes: usize,
        maximum_aggregate_bytes: u16,
    ) -> Option<DatapathHtEgressSnapshot> {
        if let Err(reason) = single_station_ht_demand(demand) {
            return rejected_ht_egress_snapshot(reason);
        }
        let TxPhyRate::Ht(rate) = self.rate_control.ampdu_tx_rate(self.aggregate_rate_policy)
        else {
            return rejected_ht_egress_snapshot(DatapathEgressSnapshotRejection::NonHtRate);
        };
        let Some(agreement) = self.block_ack_windows.first().copied() else {
            return rejected_ht_egress_snapshot(DatapathEgressSnapshotRejection::NoBlockAck);
        };
        if self.block_ack_generation_exhausted & 1 != 0 || agreement & 0x7f == 0 {
            return rejected_ht_egress_snapshot(DatapathEgressSnapshotRejection::NoBlockAck);
        }
        let maximum_frames = usize::from(agreement & 0x7f)
            .min(usize::from(self.config.frame_limit))
            .min(SLOTS);
        let Some(maximum_frames) = u8::try_from(maximum_frames)
            .ok()
            .and_then(core::num::NonZeroU8::new)
        else {
            return rejected_ht_egress_snapshot(DatapathEgressSnapshotRejection::InvalidGeometry);
        };
        Some(DatapathHtEgressSnapshot::new(
            rate,
            maximum_frames,
            maximum_ethernet_bytes,
            maximum_aggregate_bytes,
        ))
    }
}
