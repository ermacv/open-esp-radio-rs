//! ESP32-S31 ordinary TX ownership specialized for an access-point epoch.
//!
//! Frame construction remains in the portable AP/IEEE 802.11 layers. This
//! owner adds the chip queue, retry, power and descriptor policy and exposes
//! the same finite IRQ/deadline transaction used by STA. It deliberately does
//! not poll, spawn tasks or count a frame as transmitted before completion.

use open_esp_radio_esp32s31_hal::types::MacInterface;
use open_esp_radio_esp32s31_wifi::{
    ampdu_tx::{AmpduTxRoleAdapter, HtAmpduPublicationInputs, ht_ampdu_publication_config},
    ordinary_tx::{
        OrdinaryTxError, OrdinaryTxInterface, OrdinaryTxOutcome, OrdinaryTxOwner, OrdinaryTxPlan,
        TX_CCMP_MIC_SIZE, TX_METADATA_SIZE, WifiTxEntropy, WifiTxPowerProfile, WifiTxResources,
        WifiTxTimer,
    },
    tx::{WifiTxProgress, WifiTxWake},
};
use open_esp_radio_esp32s31_wifi_mac::tx::{
    HtAmpduTxConfig, HtChannelWidth, HtDuplicateCertificationRequest, HtDuplicateRate,
    HtDuplicateTxLinkCapabilities, HtDuplicateTxSelection, HtGuardInterval, HtMcs, HtRate,
    LegacyRate, LegacyTxQueue, TxHardware, TxPhyRate, select_esp32s31_ht_duplicate_tx,
};
use open_esp_radio_esp32s31_wifi_mac::tx_protection::{
    TxProtectionAdmissionError, TxProtectionReceiver, WifiTxProtectionPolicy,
};
use open_esp_radio_esp32s31_wifi_mac::tx_runtime::OrdinaryRetryRatePolicy;
use open_esp_radio_ieee80211::{
    channel::{WifiChannel, WifiChannelWidth},
    ht::HtPeerCapabilities,
};
use open_esp_radio_wifi_softmac::{MacTxPlan, MacTxQueueState};

/// Runtime-independent publication policy for the initial AP implementation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31ApTxConfig {
    /// Watchdog applied independently to each hardware publication.
    pub publication_timeout_micros: u64,
}

/// Semantic AP frame class translated into the private ESP32-S31 queue plan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31ApTxClass {
    /// Broadcast beacon; never retried for an acknowledgement.
    Beacon,
    /// Authentication, association or deauthentication response.
    Management,
    /// Unprotected EAPOL exchange during the four-way handshake.
    Eapol,
    /// Pairwise protected Ethernet data after the controlled port opens.
    Data,
    /// GTK-protected multicast/broadcast data. Hardware may report terminal
    /// publication success, but there is no receiver ACK and no retry series.
    GroupData,
}

impl Esp32s31ApTxClass {
    fn publication_limit(self, rate: LegacyRate) -> u8 {
        match self {
            Self::Beacon | Self::GroupData => 1,
            Self::Management | Self::Eapol | Self::Data => rate
                .vendor_retry_publication_limit()
                .expect("every AP legacy rate has a recovered Dot11G schedule"),
        }
    }

    const fn queue(self) -> LegacyTxQueue {
        // The recovered vendor management/control path uses VO. EAPOL is
        // deliberately kept on the same bounded pre-data path until the
        // controlled port opens.
        match self {
            Self::Beacon | Self::Management | Self::Eapol => LegacyTxQueue::Voice,
            Self::Data | Self::GroupData => LegacyTxQueue::BestEffort,
        }
    }

    const fn initial_rate(self) -> LegacyRate {
        match self {
            // The AP advertises an ERP+HT BSS, so every associated peer
            // still supports the mandatory 24 Mbit/s OFDM legacy rate. Keep
            // discovery and controlled-port setup on the maximally compatible
            // 1 Mbit/s path, but do not serialize ordinary data at that rate.
            Self::Data => LegacyRate::Ofdm24M,
            Self::GroupData => LegacyRate::Dsss1MLong,
            Self::Beacon | Self::Management | Self::Eapol => LegacyRate::Dsss1MLong,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31ApTxError {
    FrameTooLarge,
    Ordinary(OrdinaryTxError),
}

impl From<OrdinaryTxError> for Esp32s31ApTxError {
    fn from(error: OrdinaryTxError) -> Self {
        Self::Ordinary(error)
    }
}

/// Unique ordinary descriptor owned by one active AP epoch.
#[must_use = "an AP TX owner must be quiesced and recovered before role transition"]
pub struct Esp32s31ApTx<'slot, P, E, T, const BUFFER_SIZE: usize> {
    ordinary: OrdinaryTxOwner<'slot, P, E, T, BUFFER_SIZE>,
    config: Esp32s31ApTxConfig,
    ht_duplicate_certification: Option<HtDuplicateCertificationRequest>,
}

/// AP-local ordinary-TX policy retained while the physical descriptor owner
/// is lent to the station role.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31ApTxParked {
    config: Esp32s31ApTxConfig,
    ht_duplicate_certification: Option<HtDuplicateCertificationRequest>,
}

impl<'slot, P, E, T, const BUFFER_SIZE: usize> Esp32s31ApTx<'slot, P, E, T, BUFFER_SIZE>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
{
    pub fn new(
        resources: WifiTxResources<'slot, P, E, T, BUFFER_SIZE>,
        config: Esp32s31ApTxConfig,
    ) -> Self {
        Self {
            ordinary: OrdinaryTxOwner::new(resources),
            config,
            ht_duplicate_certification: None,
        }
    }

    /// Install the independent, fixed MCS32 certification request.
    ///
    /// Normal AP rate selection never consults this request as a ranked
    /// candidate. The peer planner reports its typed rejection and retains
    /// the ordinary HT fallback until the S31 formatter contract is reviewed.
    pub fn set_ht_duplicate_certification_request(
        &mut self,
        request: Option<HtDuplicateCertificationRequest>,
    ) {
        self.ht_duplicate_certification = request;
    }

    pub const fn ht_duplicate_certification_request(
        &self,
    ) -> Option<HtDuplicateCertificationRequest> {
        self.ht_duplicate_certification
    }

    pub fn queue_state(&self) -> MacTxQueueState {
        self.ordinary.queue_state()
    }

    pub const fn maximum_ht_aggregate_bytes(&self) -> u16 {
        self.ordinary.policy().ht_ampdu().maximum_aggregate_bytes()
    }

    pub fn install_tx_protection_policy(&mut self, policy: WifiTxProtectionPolicy) {
        self.ordinary.policy_mut().install_protection(policy);
    }

    /// Preflight one AP HT aggregate before its retained arena reaches DMA.
    pub fn require_unprotected_ht_aggregate(
        &self,
        rate: HtRate,
    ) -> Result<(), TxProtectionAdmissionError> {
        self.ordinary.policy().protection().require_unprotected(
            TxPhyRate::Ht(rate),
            TxProtectionReceiver::Individual,
            None,
        )
    }

    /// Preflight every initial/retry rate of one AP ordinary data MPDU.
    ///
    /// AP protocol encoders call this after their complete output-capacity
    /// admission but before advancing sequence or CCMP PN ownership. The
    /// common ordinary start edge repeats the same check before DMA.
    pub fn require_unprotected_data_retry_series(
        &self,
        rate: LegacyRate,
        group_receiver: bool,
    ) -> Result<(), Esp32s31ApTxError> {
        let class = if group_receiver {
            Esp32s31ApTxClass::GroupData
        } else {
            Esp32s31ApTxClass::Data
        };
        self.ordinary.require_unprotected_retry_series(
            TxPhyRate::Legacy(rate),
            OrdinaryRetryRatePolicy::Normal,
            class.publication_limit(rate),
            group_receiver,
        )?;
        Ok(())
    }

    pub fn now_micros(&self) -> u64 {
        self.ordinary.now_micros()
    }

    pub fn wait_until(
        &mut self,
        deadline_micros: u64,
    ) -> impl core::future::Future<Output = ()> + '_ {
        self.ordinary.wait_until(deadline_micros)
    }

    pub fn after_micros(&mut self, micros: u64) -> impl core::future::Future<Output = ()> + '_ {
        self.ordinary.after_micros(micros)
    }

    pub const fn publication_timeout_micros(&self) -> u64 {
        self.config.publication_timeout_micros
    }

    /// Build the AP-specific key/interface/power wrapper around the common
    /// HT A-MPDU formatter. Frame retention and BlockAck completion remain in
    /// the role-neutral aggregate owner.
    pub fn ht_ampdu_config(
        &mut self,
        rate: HtRate,
        aggregate_length: u16,
        subframes: u8,
        hardware_key_selector: u8,
    ) -> Option<HtAmpduTxConfig> {
        let queue = LegacyTxQueue::BestEffort;
        let (contention, contention_window) = self.ordinary.contention_publication(queue);
        let data_power = self.ordinary.power().power_pair(rate.power_lookup_code());
        let rts_power = self
            .ordinary
            .power()
            .power_pair(rate.vendor_rts_rate().code());
        ht_ampdu_publication_config(
            AmpduTxRoleAdapter {
                interface: MacInterface::AccessPoint,
                hardware_key_selector,
            },
            HtAmpduPublicationInputs {
                rate,
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
    }

    /// Advance the shared BE contention state before republishing a retained
    /// aggregate. The AP and STA paths must use the same recovered LMAC EDCA
    /// transition: a retry is not a fresh exchange at ECWmin.
    pub(crate) fn record_aggregate_retry_failure(&mut self) {
        self.ordinary
            .record_retry_failure(LegacyTxQueue::BestEffort);
    }

    /// Restore ECWmin after a fully acknowledged aggregate exchange.
    pub(crate) fn record_aggregate_success(&mut self) {
        self.ordinary.record_success(LegacyTxQueue::BestEffort);
    }

    /// Restore ECWmin after a terminal incomplete, collision, or timeout
    /// result before a different MSDU exchange can claim the queue.
    pub fn reset_aggregate_contention(&mut self) {
        self.ordinary
            .reset_terminal_exchange(LegacyTxQueue::BestEffort);
    }

    /// Copy one portable encoded MPDU into the only ordinary DMA slot and
    /// publish it. A successful return means hardware owns the descriptor,
    /// not that the frame reached the air.
    pub fn start_encoded<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        class: Esp32s31ApTxClass,
        frame: &[u8],
    ) -> Result<WifiTxProgress, Esp32s31ApTxError> {
        self.start_encoded_with_key(hardware, class, frame, 0, 0, None)
    }

    /// Publish one plaintext protected MPDU through hardware CCMP.
    pub fn start_protected_encoded<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        frame: &[u8],
        hardware_key_selector: u8,
        rate: LegacyRate,
    ) -> Result<WifiTxProgress, Esp32s31ApTxError> {
        self.start_encoded_with_key(
            hardware,
            Esp32s31ApTxClass::Data,
            frame,
            TX_CCMP_MIC_SIZE,
            hardware_key_selector,
            Some(rate),
        )
    }

    /// Publish one plaintext Open-network data MPDU. Zero MIC length is the
    /// authoritative security selector; the key field is ignored by the
    /// already-reviewed ordinary unprotected path used for EAPOL/management.
    pub fn start_unprotected_data_encoded<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        frame: &[u8],
        group: bool,
        rate: LegacyRate,
    ) -> Result<WifiTxProgress, Esp32s31ApTxError> {
        self.start_encoded_with_key(
            hardware,
            if group {
                Esp32s31ApTxClass::GroupData
            } else {
                Esp32s31ApTxClass::Data
            },
            frame,
            0,
            0,
            Some(rate),
        )
    }

    /// Publish one GTK-protected group MPDU through the ordinary basic-rate
    /// owner. The transaction has exactly one hardware publication and does
    /// not interpret completion as an acknowledgement.
    pub fn start_group_protected_encoded<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        frame: &[u8],
        hardware_key_selector: u8,
    ) -> Result<WifiTxProgress, Esp32s31ApTxError> {
        self.start_encoded_with_key(
            hardware,
            Esp32s31ApTxClass::GroupData,
            frame,
            TX_CCMP_MIC_SIZE,
            hardware_key_selector,
            Some(LegacyRate::Dsss1MLong),
        )
    }

    fn start_encoded_with_key<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        class: Esp32s31ApTxClass,
        frame: &[u8],
        hardware_mic_length: usize,
        hardware_key_selector: u8,
        rate: Option<LegacyRate>,
    ) -> Result<WifiTxProgress, Esp32s31ApTxError> {
        let end = TX_METADATA_SIZE
            .checked_add(frame.len())
            .ok_or(Esp32s31ApTxError::FrameTooLarge)?;
        let buffer = self.ordinary.buffer_mut()?;
        let destination = buffer
            .get_mut(TX_METADATA_SIZE..end)
            .ok_or(Esp32s31ApTxError::FrameTooLarge)?;
        destination.copy_from_slice(frame);

        let queue = class.queue();
        let initial_rate = rate.unwrap_or_else(|| class.initial_rate());
        Ok(self.ordinary.start(
            hardware,
            OrdinaryTxPlan {
                frame_length: frame.len(),
                descriptor_capacity: None,
                exchange: MacTxPlan {
                    access_category: queue.access_category(),
                    initial_rate: TxPhyRate::Legacy(initial_rate),
                    publication_limit: class.publication_limit(initial_rate),
                    publication_timeout_micros: self.config.publication_timeout_micros,
                },
                hardware_mic_length,
                hardware_key_selector,
                interface: OrdinaryTxInterface::AccessPoint,
                scheduler_priority: queue.vendor_data_scheduler_priority(),
                packet_priority: queue.vendor_data_packet_priority(),
            },
        )?)
    }

    /// Advance an active descriptor from one coalesced IRQ/deadline edge.
    pub async fn service<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        wake: WifiTxWake,
    ) -> Result<WifiTxProgress, Esp32s31ApTxError> {
        Ok(self.ordinary.service(hardware, wake)?)
    }

    pub fn take_last_outcome(&mut self) -> Option<OrdinaryTxOutcome> {
        self.ordinary.take_last_outcome()
    }

    pub fn wait_deadline(&mut self) -> impl core::future::Future<Output = ()> + '_ {
        self.ordinary.wait_deadline()
    }

    /// Lend the physical ordinary descriptor at an idle role boundary.
    #[allow(clippy::result_large_err)]
    pub fn try_park(
        self,
    ) -> Result<
        (
            WifiTxResources<'slot, P, E, T, BUFFER_SIZE>,
            Esp32s31ApTxParked,
        ),
        Self,
    > {
        let Self {
            ordinary,
            config,
            ht_duplicate_certification,
        } = self;
        match ordinary.try_into_resources() {
            Ok(resources) => Ok((
                resources,
                Esp32s31ApTxParked {
                    config,
                    ht_duplicate_certification,
                },
            )),
            Err(ordinary) => Err(Self {
                ordinary,
                config,
                ht_duplicate_certification,
            }),
        }
    }

    pub fn resume(
        resources: WifiTxResources<'slot, P, E, T, BUFFER_SIZE>,
        parked: Esp32s31ApTxParked,
    ) -> Self {
        Self {
            ordinary: OrdinaryTxOwner::new(resources),
            config: parked.config,
            ht_duplicate_certification: parked.ht_duplicate_certification,
        }
    }

    /// Recover the common TX capability only when no descriptor is owned by
    /// DMA and the queue is not quarantined for radio reset.
    #[allow(clippy::result_large_err)]
    pub fn try_into_resources(self) -> Result<WifiTxResources<'slot, P, E, T, BUFFER_SIZE>, Self> {
        self.try_park().map(|(resources, _)| resources)
    }
}

/// Translate the portable 500-kbit/s association rate into the exact legacy
/// hardware rate enum. The parser supplies only members of the AP's advertised
/// B/G rate set; the 1-Mbit/s fallback preserves safety for stale peer state.
pub const fn peer_legacy_rate(maximum_rate_500kbps: u8) -> LegacyRate {
    match maximum_rate_500kbps {
        108 => LegacyRate::Ofdm54M,
        96 => LegacyRate::Ofdm48M,
        72 => LegacyRate::Ofdm36M,
        48 => LegacyRate::Ofdm24M,
        36 => LegacyRate::Ofdm18M,
        24 => LegacyRate::Ofdm12M,
        18 => LegacyRate::Ofdm9M,
        12 => LegacyRate::Ofdm6M,
        22 => LegacyRate::Cck11MLong,
        11 => LegacyRate::Cck5M5Long,
        4 => LegacyRate::Dsss2MLong,
        _ => LegacyRate::Dsss1MLong,
    }
}

/// Select the fastest one-stream HT vector admitted by both BSS geometry and
/// the peer's observed receive capabilities.
pub fn peer_ht_rate(channel: WifiChannel, peer: HtPeerCapabilities) -> Option<HtRate> {
    let mcs = HtMcs::from_index(peer.highest_rx_mcs())?;
    let (channel_width, guard_width) = match channel.width() {
        WifiChannelWidth::Mhz20 => (HtChannelWidth::Mhz20, WifiChannelWidth::Mhz20),
        width @ (WifiChannelWidth::Mhz40Above | WifiChannelWidth::Mhz40Below)
            if peer.supports_40_mhz() =>
        {
            (HtChannelWidth::Mhz40, width)
        }
        WifiChannelWidth::Mhz40Above | WifiChannelWidth::Mhz40Below => {
            (HtChannelWidth::Mhz20, WifiChannelWidth::Mhz20)
        }
    };
    // The recovered retry table has a complete SGI schedule only for MCS7.
    // Lower peer maxima therefore stay on their complete LGI schedules.
    let guard_interval = if mcs == HtMcs::Mcs7 && peer.supports_short_guard_interval(guard_width) {
        HtGuardInterval::Short400Ns
    } else {
        HtGuardInterval::Long800Ns
    };
    Some(HtRate::new(mcs, guard_interval, channel_width))
}

/// Retain a peer-admitted HT Duplicate candidate without publishing it.
///
/// The returned value is protocol-valid, including peer capability and HT40
/// geometry. It cannot enter [`TxPhyRate`] until the ESP32-S31 queue/rate/power
/// encoding has a reviewed oracle; the explicit selector below reports that
/// hardware rejection without changing the ordinary peer rate.
pub fn peer_ht_duplicate_rate(
    channel: WifiChannel,
    peer: HtPeerCapabilities,
) -> Option<HtDuplicateRate> {
    peer.ht_duplicate_mcs32()?;
    let width = match channel.width() {
        width @ (WifiChannelWidth::Mhz40Above | WifiChannelWidth::Mhz40Below) => width,
        WifiChannelWidth::Mhz20 => return None,
    };
    let guard_interval = if peer.supports_short_guard_interval(width) {
        HtGuardInterval::Short400Ns
    } else {
        HtGuardInterval::Long800Ns
    };
    Some(HtDuplicateRate::new(guard_interval))
}

/// Evaluate the AP's explicit MCS32 request for one associated peer.
///
/// This is an observation/planning path only. A rejected request leaves
/// [`peer_ht_rate`] in sole ownership of the publishable HT fallback.
pub fn peer_ht_duplicate_tx_selection(
    channel: WifiChannel,
    peer: Option<HtPeerCapabilities>,
    request: Option<HtDuplicateCertificationRequest>,
) -> HtDuplicateTxSelection {
    let Some(peer) = peer else {
        return select_esp32s31_ht_duplicate_tx(
            request,
            HtDuplicateTxLinkCapabilities::new(None, false, false),
        );
    };
    let (channel_width, peer_supports_short_guard_interval) = match channel.width() {
        WifiChannelWidth::Mhz20 => (
            HtChannelWidth::Mhz20,
            peer.supports_short_guard_interval(WifiChannelWidth::Mhz20),
        ),
        width @ (WifiChannelWidth::Mhz40Above | WifiChannelWidth::Mhz40Below) => (
            HtChannelWidth::Mhz40,
            peer.supports_short_guard_interval(width),
        ),
    };
    select_esp32s31_ht_duplicate_tx(
        request,
        HtDuplicateTxLinkCapabilities::new(
            Some(channel_width),
            peer.supports_ht_duplicate_mcs32(),
            peer_supports_short_guard_interval,
        ),
    )
}

#[cfg(test)]
mod tests {
    use core::{
        future::{Future, ready},
        pin::pin,
        task::{Context, Poll},
    };

    use open_esp_radio_esp32s31_hal::types::{
        MacLegacyTxProgram, MacTxCompletionObservation, MacTxDetachOutcome, MacTxDetachReason,
        MacTxQueueDetached,
    };
    use open_esp_radio_esp32s31_wifi::ordinary_tx::WifiTxPowerPair;
    use open_esp_radio_esp32s31_wifi_mac::{
        MacInterface,
        tx::{HardwareOwnedTxDma, PreparedTxDma, TxSlot, TxSlotState},
        tx_runtime::WifiTxRuntimePolicy,
    };

    use super::*;

    fn block_on<F: Future>(future: F) -> F::Output {
        let mut future = pin!(future);
        let mut context = Context::from_waker(core::task::Waker::noop());
        loop {
            match future.as_mut().poll(&mut context) {
                Poll::Ready(output) => return output,
                Poll::Pending => std::thread::yield_now(),
            }
        }
    }

    #[derive(Default)]
    struct Hardware {
        prepare: bool,
        publications: u8,
        legacy_program: Option<MacLegacyTxProgram>,
        completion: Option<MacTxCompletionObservation>,
    }

    impl TxHardware for Hardware {
        fn prepare_bound_legacy_tx(
            &mut self,
            _dma: &dyn PreparedTxDma,
            _queue: u8,
            program: MacLegacyTxProgram,
        ) -> bool {
            self.legacy_program = Some(program);
            self.prepare
        }

        fn start_bound_legacy_tx(&mut self, _dma: &dyn HardwareOwnedTxDma, _queue: u8) {
            self.publications += 1;
        }

        fn take_tx_completion(&mut self, _queue: u8) -> Option<MacTxCompletionObservation> {
            self.completion.take()
        }

        fn begin_tx_timeout_abort(&mut self, _queue: u8) -> bool {
            false
        }

        fn with_tx_queue_detached<R>(
            &mut self,
            _queue: u8,
            expected_descriptor_head: u32,
            reason: MacTxDetachReason,
            detached: impl for<'detached> FnOnce(MacTxQueueDetached<'detached>) -> R,
        ) -> MacTxDetachOutcome<R> {
            match reason {
                MacTxDetachReason::Completed => MacTxDetachOutcome::Detached(detached(
                    MacTxQueueDetached::new_model(expected_descriptor_head),
                )),
                MacTxDetachReason::Timeout | MacTxDetachReason::Collision => {
                    MacTxDetachOutcome::NoEvent
                }
            }
        }
    }

    #[derive(Clone, Copy)]
    struct Power;

    impl WifiTxPowerProfile for Power {
        fn power_pair(&self, _rate_code: u8) -> WifiTxPowerPair {
            WifiTxPowerPair {
                primary: 1,
                alternate: 1,
            }
        }
    }

    struct Timer;

    impl WifiTxTimer for Timer {
        fn now_micros(&self) -> u64 {
            1
        }

        fn wait_until(&mut self, _deadline_micros: u64) -> impl Future<Output = ()> + '_ {
            ready(())
        }

        fn after_micros(&mut self, _micros: u64) -> impl Future<Output = ()> + '_ {
            ready(())
        }
    }

    #[test]
    fn peer_rate_mapping_preserves_every_advertised_bg_rate() {
        assert_eq!(Esp32s31ApTxClass::Data.initial_rate(), LegacyRate::Ofdm24M);
        assert_eq!(peer_legacy_rate(108), LegacyRate::Ofdm54M);
        assert_eq!(peer_legacy_rate(96), LegacyRate::Ofdm48M);
        assert_eq!(peer_legacy_rate(72), LegacyRate::Ofdm36M);
        assert_eq!(peer_legacy_rate(48), LegacyRate::Ofdm24M);
        assert_eq!(peer_legacy_rate(22), LegacyRate::Cck11MLong);
        assert_eq!(peer_legacy_rate(0), LegacyRate::Dsss1MLong);
        for class in [
            Esp32s31ApTxClass::Beacon,
            Esp32s31ApTxClass::Management,
            Esp32s31ApTxClass::Eapol,
        ] {
            assert_eq!(class.initial_rate(), LegacyRate::Dsss1MLong);
        }
        assert_eq!(
            Esp32s31ApTxClass::Beacon.publication_limit(LegacyRate::Dsss1MLong),
            1
        );
        assert_eq!(
            Esp32s31ApTxClass::Management.publication_limit(LegacyRate::Dsss1MLong),
            32
        );
        assert_eq!(
            Esp32s31ApTxClass::Data.publication_limit(LegacyRate::Ofdm54M),
            32
        );
    }

    #[test]
    fn peer_ht_rate_requires_matching_bss_and_peer_width() {
        use open_esp_radio_ieee80211::ht::{ht_capability_ie, ht_peer_capabilities};

        let ht40 = WifiChannel::new_2_4_ghz(6, WifiChannelWidth::Mhz40Above).unwrap();
        let wide_peer = ht_peer_capabilities(&ht_capability_ie(ht40)).unwrap();
        assert_eq!(
            peer_ht_rate(ht40, wide_peer),
            Some(HtRate::new(
                HtMcs::Mcs7,
                HtGuardInterval::Short400Ns,
                HtChannelWidth::Mhz40,
            ))
        );

        let narrow_peer =
            ht_peer_capabilities(&ht_capability_ie(WifiChannel::mhz20(6).unwrap())).unwrap();
        assert_eq!(
            peer_ht_rate(ht40, narrow_peer),
            Some(HtRate::new(
                HtMcs::Mcs7,
                HtGuardInterval::Short400Ns,
                HtChannelWidth::Mhz20,
            ))
        );
    }

    #[test]
    fn ap_mcs32_request_reaches_the_shared_frontier_without_replacing_fallback() {
        use open_esp_radio_esp32s31_wifi_mac::tx::{
            HtDuplicateTxEvidenceGaps, HtDuplicateTxRejection, HtDuplicateTxUnavailable,
        };
        use open_esp_radio_ieee80211::ht::{
            HtDuplicateMcs32, ht_capability_ie, ht_peer_capabilities,
        };

        let channel = WifiChannel::new_2_4_ghz(6, WifiChannelWidth::Mhz40Above).unwrap();
        let mut capability = ht_capability_ie(channel);
        HtDuplicateMcs32::new().advertise_receive_only(&mut capability);
        let peer = ht_peer_capabilities(&capability).unwrap();
        let fallback = peer_ht_rate(channel, peer).unwrap();
        assert_eq!(fallback.mcs, HtMcs::Mcs7);
        assert_eq!(fallback.channel_width, HtChannelWidth::Mhz40);
        assert_eq!(
            peer_ht_duplicate_rate(channel, peer),
            Some(HtDuplicateRate::new(HtGuardInterval::Short400Ns))
        );

        let request = HtDuplicateCertificationRequest::new(
            HtChannelWidth::Mhz40,
            HtGuardInterval::Short400Ns,
            5_484,
        );
        let selection = peer_ht_duplicate_tx_selection(channel, Some(peer), Some(request));
        assert_eq!(selection.plan(), None);
        assert_eq!(
            selection.rejection(),
            Some(HtDuplicateTxRejection::Hardware(
                HtDuplicateTxUnavailable::Esp32s31EvidenceIncomplete(
                    HtDuplicateTxEvidenceGaps::ESP32S31,
                )
            ))
        );
        assert_eq!(peer_ht_rate(channel, peer), Some(fallback));
    }

    #[test]
    fn idle_ap_tx_lends_and_resumes_the_exact_ordinary_owner() {
        let mut slot = pin!(TxSlot::<256>::new_model());
        let request = HtDuplicateCertificationRequest::new(
            HtChannelWidth::Mhz40,
            HtGuardInterval::Long800Ns,
            5_484,
        );
        let mut tx = Esp32s31ApTx::new(
            WifiTxResources {
                slot: slot.as_mut(),
                policy: WifiTxRuntimePolicy::vendor_defaults(),
                power: Power,
                entropy: || 0,
                timer: Timer,
            },
            Esp32s31ApTxConfig {
                publication_timeout_micros: 7_500,
            },
        );
        tx.set_ht_duplicate_certification_request(Some(request));

        let (resources, parked) = tx
            .try_park()
            .unwrap_or_else(|_| panic!("idle AP TX must lend its descriptor"));
        assert_eq!(resources.slot.state(), TxSlotState::Free);

        let tx = Esp32s31ApTx::resume(resources, parked);
        assert_eq!(tx.publication_timeout_micros(), 7_500);
        assert_eq!(tx.ht_duplicate_certification_request(), Some(request));
        assert_eq!(tx.queue_state(), MacTxQueueState::Ready);
        assert!(tx.try_into_resources().is_ok());
    }

    #[test]
    fn required_protection_blocks_ap_aggregate_and_ordinary_retry_series() {
        use open_esp_radio_esp32s31_wifi_mac::tx_protection::{
            ErpProtectionMode, HtProtectionMode, TxProtectionAdmissionError, TxProtectionMechanism,
            TxProtectionReason, TxProtectionRequest, WifiTxProtectionPolicy,
        };

        let mut slot = pin!(TxSlot::<256>::new_model());
        let mut tx = Esp32s31ApTx::new(
            WifiTxResources {
                slot: slot.as_mut(),
                policy: WifiTxRuntimePolicy::vendor_defaults(),
                power: Power,
                entropy: || 0,
                timer: Timer,
            },
            Esp32s31ApTxConfig {
                publication_timeout_micros: 1_000,
            },
        );
        tx.install_tx_protection_policy(WifiTxProtectionPolicy::new(
            ErpProtectionMode::None,
            HtProtectionMode::NonHtMixed,
            None,
        ));
        let rate = HtRate::new(
            HtMcs::Mcs7,
            HtGuardInterval::Long800Ns,
            HtChannelWidth::Mhz20,
        );

        assert_eq!(
            tx.require_unprotected_ht_aggregate(rate),
            Err(TxProtectionAdmissionError::PhysicalPublicationUnverified {
                request: TxProtectionRequest {
                    mechanism: TxProtectionMechanism::RtsCts,
                    reason: TxProtectionReason::Ht(HtProtectionMode::NonHtMixed),
                },
            })
        );
        tx.install_tx_protection_policy(WifiTxProtectionPolicy::new(
            ErpProtectionMode::CtsToSelf,
            HtProtectionMode::None,
            None,
        ));
        assert_eq!(
            tx.require_unprotected_data_retry_series(LegacyRate::Ofdm24M, false),
            Err(Esp32s31ApTxError::Ordinary(OrdinaryTxError::Protection(
                TxProtectionAdmissionError::PhysicalPublicationUnverified {
                    request: TxProtectionRequest {
                        mechanism: TxProtectionMechanism::CtsToSelf,
                        reason: TxProtectionReason::ErpUseProtection,
                    },
                },
            ),))
        );
        assert_eq!(tx.queue_state(), MacTxQueueState::Ready);
    }

    #[test]
    fn beacon_is_one_publication_and_resources_return_only_after_completion() {
        let mut slot = pin!(TxSlot::<256>::new_model());
        let resources = WifiTxResources {
            slot: slot.as_mut(),
            policy: WifiTxRuntimePolicy::vendor_defaults(),
            power: Power,
            entropy: || 0,
            timer: Timer,
        };
        let mut tx = Esp32s31ApTx::new(
            resources,
            Esp32s31ApTxConfig {
                publication_timeout_micros: 1_000,
            },
        );
        let mut hardware = Hardware {
            prepare: true,
            completion: Some(MacTxCompletionObservation::new_model(0, 0)),
            ..Hardware::default()
        };
        let mut beacon = [0; 24];
        beacon[4] = 0xff;
        assert_eq!(
            tx.start_encoded(&mut hardware, Esp32s31ApTxClass::Beacon, &beacon),
            Ok(WifiTxProgress::Pending)
        );
        assert_eq!(tx.queue_state(), MacTxQueueState::Backpressured);
        assert_eq!(hardware.publications, 1);
        assert_eq!(
            hardware.legacy_program.unwrap().interface(),
            MacInterface::AccessPoint
        );

        let progress = block_on(tx.service(
            &mut hardware,
            WifiTxWake::Interrupt {
                events: open_esp_radio_esp32s31_wifi_mac::irq::EVENT_TX_COMPLETE,
            },
        ));
        assert_eq!(progress, Ok(WifiTxProgress::Complete));
        assert!(tx.take_last_outcome().unwrap().is_success());
        assert!(tx.try_into_resources().is_ok());
    }

    #[test]
    fn aggregate_retry_uses_the_next_edca_contention_window() {
        let mut slot = pin!(TxSlot::<256>::new_model());
        let resources = WifiTxResources {
            slot: slot.as_mut(),
            policy: WifiTxRuntimePolicy::vendor_defaults(),
            power: Power,
            entropy: || u32::MAX,
            timer: Timer,
        };
        let mut tx = Esp32s31ApTx::new(
            resources,
            Esp32s31ApTxConfig {
                publication_timeout_micros: 1_000,
            },
        );
        let rate = HtRate::new(
            HtMcs::Mcs7,
            HtGuardInterval::Short400Ns,
            HtChannelWidth::Mhz40,
        );

        let initial = tx.ht_ampdu_config(rate, 8_000, 8, 1).unwrap();
        assert_eq!(initial.contention_window, 15);

        tx.record_aggregate_retry_failure();
        let retry = tx.ht_ampdu_config(rate, 8_000, 8, 1).unwrap();
        assert_eq!(retry.contention_window, 31);

        tx.record_aggregate_success();
        let next_exchange = tx.ht_ampdu_config(rate, 8_000, 8, 1).unwrap();
        assert_eq!(next_exchange.contention_window, 15);

        tx.record_aggregate_retry_failure();
        tx.reset_aggregate_contention();
        let after_terminal_failure = tx.ht_ampdu_config(rate, 8_000, 8, 1).unwrap();
        assert_eq!(after_terminal_failure.contention_window, 15);
    }
}
