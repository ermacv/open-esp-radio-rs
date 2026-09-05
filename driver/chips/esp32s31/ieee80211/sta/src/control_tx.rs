//! Production owner for pre-connected management and EAPOL transmission.
//!
//! Scan, STA join and WPA2 use the same pinned ordinary TX descriptor. This
//! owner keeps its EDCA/retry/deadline state out of HIL fixtures and transfers
//! the complete resource into [`Esp32s31SingleMpduTx`] after M4 succeeds.

use core::future::Future;

pub use open_esp_radio_esp32s31_wifi::tx::ControlTxConfig;
use open_esp_radio_esp32s31_wifi_mac::{
    edca::EdcaParametersError,
    tx::{
        HtPeerAmpduParameters, LegacyRate, LegacyTxQueue, TxCompletion, TxError, TxHardware,
        TxPhyRate,
    },
    tx_protection::{TxProtectionAdmissionError, WifiTxProtectionPolicy},
    tx_runtime::{OrdinaryRetryError, OrdinaryRetryRatePolicy, WifiTxRuntimePolicy},
};
use open_esp_radio_ieee80211::{
    channel::WifiChannel,
    esp_now::EspNowRandomValue,
    management::{ProbeRequest, ProbeRequestError},
    station::{
        AssociationRequest, AssociationRequestError, OpenAuthenticationRequest, StaDataFrame,
        StaProtectedDataFrame, StaSequenceCounter, StationFrameError,
    },
    wmm::WmmParameterSet,
};
use open_esp_radio_wifi_softmac::{
    EspNowPeerId, EspNowProtocol, EspNowSendError, EspNowV2SendError, MacTxPlan,
    interface::BoundVirtualInterface,
};

use crate::{
    join::Esp32s31StaJoinTransmit,
    peer::Esp32s31StaPeerTransmit,
    single_mpdu_tx::{
        Esp32s31SingleMpduTx, SingleMpduEspNowTxError, SingleMpduTxError, SingleMpduTxOutcome,
    },
    wpa2::Esp32s31Wpa2Transmit,
};

use open_esp_radio_esp32s31_wifi::{
    esp_now::{
        Esp32s31EspNowTxConfig, Esp32s31EspNowTxError, start_esp_now_v1_plaintext,
        start_esp_now_v2_plaintext,
    },
    ordinary_tx::{
        OrdinaryTxError, OrdinaryTxOutcome, OrdinaryTxOwner, OrdinaryTxPlan, TX_CCMP_MIC_SIZE,
        TX_METADATA_SIZE, TxResetReason, WifiTxEntropy, WifiTxPowerProfile, WifiTxTimer,
    },
    tx::{WifiTxProgress, WifiTxWake},
};

pub use crate::single_mpdu_tx::ConnectedTxHandoff;
pub use open_esp_radio_esp32s31_wifi::ordinary_tx::WifiTxResources;

// SOURCE: complete `libnet80211.a[ieee80211_output.o]` passes
// coexistence events 5/6 for Probe and Authentication/Association. Complete
// `libcoexist.a[coexist_core.o]::coex_pti_tab` maps both to PTI one;
// complete `libpp.a[hal_mac.o,hal_coex.o]::
// {mac_tx_set_pti,hal_set_tx_pti}` retains scheduler and packet PTI one.
const MANAGEMENT_SCHEDULER_PRIORITY: u8 = 1;
const MANAGEMENT_PACKET_PRIORITY: u8 = 1;

/// Publication properties not encoded in an IEEE 802.11 frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Publication {
    queue: LegacyTxQueue,
    rate: TxPhyRate,
    attempt_limit: u8,
    hardware_mic_length: usize,
    hardware_key_selector: u8,
    descriptor_capacity: Option<u32>,
    scheduler_priority: u8,
    packet_priority: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlTxError {
    ProbeEncode(ProbeRequestError),
    StationEncode(StationFrameError),
    AssociationEncode(AssociationRequestError),
    Busy,
    BufferSizeOverflow,
    DeadlineOverflow,
    Tx(TxError),
    Retry(OrdinaryRetryError),
    Protection(TxProtectionAdmissionError),
    HardwareTimeout,
    CollisionLimit,
    RadioResetRequired(TxResetReason),
}

impl ControlTxError {
    /// Whether this failure proves that no ordinary TX publication remains
    /// owned by hardware.
    ///
    /// A cold active scan may safely continue as passive only for these
    /// terminal or pre-publication failures. `Busy`, an unclassified low-level
    /// `Tx` error and `RadioResetRequired` retain uncertain or quarantined
    /// descriptor ownership and must instead return to the radio lifecycle.
    pub const fn retains_quiescent_owner(self) -> bool {
        matches!(
            self,
            Self::ProbeEncode(_)
                | Self::StationEncode(_)
                | Self::AssociationEncode(_)
                | Self::BufferSizeOverflow
                | Self::DeadlineOverflow
                | Self::Retry(_)
                | Self::Protection(_)
                | Self::HardwareTimeout
                | Self::CollisionLimit
        )
    }
}

impl From<OrdinaryTxError> for ControlTxError {
    fn from(error: OrdinaryTxError) -> Self {
        match error {
            OrdinaryTxError::Busy => Self::Busy,
            OrdinaryTxError::BufferSizeOverflow => Self::BufferSizeOverflow,
            OrdinaryTxError::DeadlineOverflow => Self::DeadlineOverflow,
            OrdinaryTxError::Tx(error) => Self::Tx(error),
            OrdinaryTxError::Retry(error) => Self::Retry(error),
            OrdinaryTxError::Protection(error) => Self::Protection(error),
            OrdinaryTxError::RadioResetRequired(reason) => Self::RadioResetRequired(reason),
        }
    }
}

/// Unique pre-connected ordinary descriptor and contention-state owner.
pub struct Esp32s31ControlTx<'slot, P, E, T, const BUFFER_SIZE: usize> {
    ordinary: OrdinaryTxOwner<'slot, P, E, T, BUFFER_SIZE>,
    config: ControlTxConfig,
}

impl<'slot, P, E, T, const BUFFER_SIZE: usize> Esp32s31ControlTx<'slot, P, E, T, BUFFER_SIZE>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
{
    pub fn new(
        resources: WifiTxResources<'slot, P, E, T, BUFFER_SIZE>,
        config: ControlTxConfig,
    ) -> Self {
        Self {
            ordinary: OrdinaryTxOwner::new(resources),
            config,
        }
    }

    pub const fn policy(&self) -> &WifiTxRuntimePolicy {
        self.ordinary.policy()
    }

    pub fn policy_mut(&mut self) -> &mut WifiTxRuntimePolicy {
        self.ordinary.policy_mut()
    }

    pub const fn power_profile(&self) -> &P {
        self.ordinary.power()
    }

    /// Whether the pre-connected ordinary descriptor is hardware-owned.
    pub const fn active(&self) -> bool {
        self.ordinary.active()
    }

    pub fn now_micros(&self) -> u64 {
        self.ordinary.now_micros()
    }

    pub fn take_last_outcome(&mut self) -> Option<SingleMpduTxOutcome> {
        self.ordinary.take_last_outcome()
    }

    /// Resolve, encode and publish one standalone plaintext ESP-NOW v1
    /// Action MPDU through the pre-connected station ordinary descriptor.
    ///
    /// The standalone role owns the sole management/non-QoS sequence space;
    /// no association or WPA2 handoff is needed and no key slot is borrowed.
    #[allow(clippy::too_many_arguments)]
    pub fn start_esp_now_v1_plaintext<H: TxHardware, const PEERS: usize>(
        &mut self,
        hardware: &mut H,
        protocol: &EspNowProtocol<PEERS>,
        sequence: &mut StaSequenceCounter,
        peer: EspNowPeerId,
        random_value: EspNowRandomValue,
        payload: &[u8],
        active_channel: WifiChannel,
        active_station: BoundVirtualInterface,
        config: Esp32s31EspNowTxConfig,
    ) -> Result<WifiTxProgress, SingleMpduEspNowTxError> {
        if self.ordinary.active() {
            return Err(Esp32s31EspNowTxError::Tx(OrdinaryTxError::Busy).into());
        }
        let peer_channel = protocol
            .peers()
            .get(peer)
            .map_err(EspNowSendError::Peer)?
            .channel();
        if peer_channel != active_channel {
            return Err(Esp32s31EspNowTxError::ChannelMismatch {
                prepared: peer_channel,
                active: active_channel,
            }
            .into());
        }
        // Keep sequence publication transactional with ordinary-TX
        // ownership. The ordinary owner returns `Ok` only after its DMA
        // publication commit and infallible queue doorbell; a fail-closed PHY
        // or any pre-publication queue rejection returns the caller's exact
        // sequence frontier unchanged.
        let mut next_sequence = *sequence;
        let prepared = protocol.prepare_v1_tx(peer, &mut next_sequence, random_value, payload)?;
        let result = start_esp_now_v1_plaintext(
            &mut self.ordinary,
            hardware,
            prepared,
            active_channel,
            active_station,
            config,
        )
        .map_err(Into::into);
        if result.is_ok() {
            *sequence = next_sequence;
        }
        result
    }

    /// Resolve and publish one standalone plaintext ESP-NOW v2 Action MPDU.
    #[allow(clippy::too_many_arguments)]
    pub fn start_esp_now_v2_plaintext<H: TxHardware, const PEERS: usize>(
        &mut self,
        hardware: &mut H,
        protocol: &EspNowProtocol<PEERS>,
        sequence: &mut StaSequenceCounter,
        peer: EspNowPeerId,
        random_value: EspNowRandomValue,
        payload: &[u8],
        active_channel: WifiChannel,
        active_station: BoundVirtualInterface,
        config: Esp32s31EspNowTxConfig,
    ) -> Result<WifiTxProgress, SingleMpduEspNowTxError> {
        if self.ordinary.active() {
            return Err(Esp32s31EspNowTxError::Tx(OrdinaryTxError::Busy).into());
        }
        let peer_channel = protocol
            .peers()
            .get(peer)
            .map_err(EspNowV2SendError::Peer)?
            .channel();
        if peer_channel != active_channel {
            return Err(Esp32s31EspNowTxError::ChannelMismatch {
                prepared: peer_channel,
                active: active_channel,
            }
            .into());
        }
        let mut next_sequence = *sequence;
        let prepared = protocol.prepare_v2_tx(peer, &mut next_sequence, random_value, payload)?;
        let result = start_esp_now_v2_plaintext(
            &mut self.ordinary,
            hardware,
            prepared,
            active_channel,
            active_station,
            config,
        )
        .map_err(Into::into);
        if result.is_ok() {
            *sequence = next_sequence;
        }
        result
    }

    pub fn wait_deadline(&mut self) -> impl Future<Output = ()> + '_ {
        self.ordinary.wait_deadline()
    }

    /// Consume one IRQ/deadline edge for a standalone ordinary transaction.
    pub async fn service<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        wake: WifiTxWake,
    ) -> Result<WifiTxProgress, SingleMpduTxError> {
        self.ordinary.service(hardware, wake).map_err(Into::into)
    }

    /// Return the role-neutral ordinary descriptor while it is idle.
    ///
    /// This is the supervisor transition used when stopped Wi-Fi changes
    /// role. A live or quarantined transaction returns the complete STA owner
    /// unchanged, so AP cannot steal a descriptor from DMA.
    #[allow(clippy::result_large_err)]
    pub fn try_into_resources(self) -> Result<WifiTxResources<'slot, P, E, T, BUFFER_SIZE>, Self> {
        let Self { ordinary, config } = self;
        match ordinary.try_into_resources() {
            Ok(resources) => Ok(resources),
            Err(ordinary) => Err(Self { ordinary, config }),
        }
    }

    pub fn install_ht_ampdu_policy(&mut self, parameters: HtPeerAmpduParameters) {
        self.ordinary.policy_mut().install_ht_ampdu(parameters);
    }

    pub fn install_he_bss_color(&mut self, bss_color: u8) {
        self.ordinary.policy_mut().install_he_bss_color(bss_color);
    }

    pub fn install_wmm_edca(
        &mut self,
        parameters: WmmParameterSet,
    ) -> Result<(), EdcaParametersError> {
        self.ordinary.policy_mut().install_wmm(parameters)
    }

    pub fn install_tx_protection_policy(&mut self, policy: WifiTxProtectionPolicy) {
        self.ordinary.policy_mut().install_protection(policy);
    }

    /// Send one active-scan Probe Request. The optional current-channel IE is
    /// an explicit scan policy input rather than an opaque post-encode patch.
    pub async fn transmit_probe_request<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        request: ProbeRequest<'_>,
        current_channel: Option<u8>,
        descriptor_capacity: Option<u32>,
    ) -> Result<TxCompletion, ControlTxError> {
        let frame_length = request
            .encode(&mut self.ordinary.buffer_mut()?[TX_METADATA_SIZE..])
            .map_err(ControlTxError::ProbeEncode)?;
        let frame_length = if let Some(channel) = current_channel {
            let end = TX_METADATA_SIZE
                .checked_add(frame_length)
                .and_then(|end| end.checked_add(3))
                .ok_or(ControlTxError::BufferSizeOverflow)?;
            let buffer = self.ordinary.buffer_mut()?;
            let element = buffer
                .get_mut(TX_METADATA_SIZE + frame_length..end)
                .ok_or(ControlTxError::BufferSizeOverflow)?;
            element.copy_from_slice(&[3, 1, channel]);
            frame_length + 3
        } else {
            frame_length
        };
        self.transmit_prepared(
            hardware,
            frame_length,
            Publication {
                queue: LegacyTxQueue::Voice,
                rate: TxPhyRate::Legacy(LegacyRate::Dsss1MLong),
                attempt_limit: 1,
                hardware_mic_length: 0,
                hardware_key_selector: 0,
                descriptor_capacity,
                scheduler_priority: MANAGEMENT_SCHEDULER_PRIORITY,
                packet_priority: MANAGEMENT_PACKET_PRIORITY,
            },
        )
        .await
    }

    pub async fn transmit_open_authentication<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        request: OpenAuthenticationRequest,
    ) -> Result<TxCompletion, ControlTxError> {
        let frame_length = request
            .encode(&mut self.ordinary.buffer_mut()?[TX_METADATA_SIZE..])
            .map_err(ControlTxError::StationEncode)?;
        self.transmit_management_voice(hardware, frame_length).await
    }

    pub async fn transmit_association<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        request: AssociationRequest<'_>,
    ) -> Result<TxCompletion, ControlTxError> {
        let frame_length = request
            .encode(
                &mut self.ordinary.buffer_mut()?[TX_METADATA_SIZE..],
                &crate::profile::ASSOCIATION_CAPABILITIES,
            )
            .map_err(ControlTxError::AssociationEncode)?;
        self.transmit_management_voice(hardware, frame_length).await
    }

    pub async fn transmit_unprotected_data<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        frame: StaDataFrame<'_>,
    ) -> Result<TxCompletion, ControlTxError> {
        let frame_length = frame
            .encode(&mut self.ordinary.buffer_mut()?[TX_METADATA_SIZE..])
            .map_err(ControlTxError::StationEncode)?;
        self.transmit_prepared(
            hardware,
            frame_length,
            Publication {
                queue: LegacyTxQueue::Voice,
                rate: TxPhyRate::Legacy(LegacyRate::Dsss1MLong),
                attempt_limit: self.config.unicast_attempt_limit,
                hardware_mic_length: 0,
                hardware_key_selector: 0,
                descriptor_capacity: None,
                scheduler_priority: LegacyTxQueue::Voice.vendor_data_scheduler_priority(),
                packet_priority: LegacyTxQueue::Voice.vendor_data_packet_priority(),
            },
        )
        .await
    }

    pub async fn transmit_protected_data<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        frame: StaProtectedDataFrame<'_>,
        queue: LegacyTxQueue,
        rate: TxPhyRate,
        hardware_key_selector: u8,
    ) -> Result<TxCompletion, ControlTxError> {
        self.ordinary.require_unprotected_retry_series(
            rate,
            OrdinaryRetryRatePolicy::Normal,
            self.config.unicast_attempt_limit,
            frame.destination[0] & 1 != 0,
        )?;
        let frame_length = frame
            .encode(&mut self.ordinary.buffer_mut()?[TX_METADATA_SIZE..])
            .map_err(ControlTxError::StationEncode)?;
        self.transmit_prepared(
            hardware,
            frame_length,
            Publication {
                queue,
                rate,
                attempt_limit: self.config.unicast_attempt_limit,
                hardware_mic_length: TX_CCMP_MIC_SIZE,
                hardware_key_selector,
                descriptor_capacity: None,
                scheduler_priority: queue.vendor_data_scheduler_priority(),
                packet_priority: queue.vendor_data_packet_priority(),
            },
        )
        .await
    }

    /// Transfer the exact descriptor, EDCA state and platform capabilities to
    /// the connected data owner. Failure returns the original owner intact.
    /// Keeping that owner in the error is required for cancellation safety:
    /// a cancelled control-TX future may still own a hardware publication.
    /// The key and sequence/config bundle is returned with it so no typed
    /// crypto capability is lost on that edge.
    #[allow(clippy::result_large_err)]
    pub fn try_into_connected(
        self,
        handoff: ConnectedTxHandoff,
    ) -> Result<Esp32s31SingleMpduTx<'slot, P, E, T, BUFFER_SIZE>, (Self, ConnectedTxHandoff)> {
        if self.ordinary.queue_state() != open_esp_radio_wifi_softmac::MacTxQueueState::Ready {
            return Err((self, handoff));
        }
        let ConnectedTxHandoff {
            security,
            sequences,
            config,
        } = handoff;
        Ok(Esp32s31SingleMpduTx::from_ordinary(
            self.ordinary,
            security,
            sequences,
            config,
        ))
    }

    async fn transmit_management_voice<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        frame_length: usize,
    ) -> Result<TxCompletion, ControlTxError> {
        self.transmit_prepared(
            hardware,
            frame_length,
            Publication {
                queue: LegacyTxQueue::Voice,
                rate: TxPhyRate::Legacy(LegacyRate::Dsss1MLong),
                attempt_limit: self.config.unicast_attempt_limit,
                hardware_mic_length: 0,
                hardware_key_selector: 0,
                descriptor_capacity: None,
                scheduler_priority: MANAGEMENT_SCHEDULER_PRIORITY,
                packet_priority: MANAGEMENT_PACKET_PRIORITY,
            },
        )
        .await
    }

    async fn transmit_prepared<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        frame_length: usize,
        publication: Publication,
    ) -> Result<TxCompletion, ControlTxError> {
        self.ordinary.start(
            hardware,
            OrdinaryTxPlan {
                frame_length,
                descriptor_capacity: publication.descriptor_capacity,
                exchange: MacTxPlan {
                    access_category: publication.queue.access_category(),
                    initial_rate: publication.rate,
                    publication_limit: publication.attempt_limit,
                    publication_timeout_micros: self.config.completion_timeout_us,
                },
                hardware_mic_length: publication.hardware_mic_length,
                hardware_key_selector: publication.hardware_key_selector,
                interface: open_esp_radio_esp32s31_wifi::ordinary_tx::OrdinaryTxInterface::Station,
                scheduler_priority: publication.scheduler_priority,
                packet_priority: publication.packet_priority,
            },
        )?;
        loop {
            if self
                .ordinary
                .service_polling(hardware, self.config.poll_interval_us)
                .await?
                == WifiTxProgress::Pending
            {
                continue;
            }
            let outcome = self
                .ordinary
                .take_last_outcome()
                .expect("complete control TX retains one terminal outcome");
            return match outcome {
                OrdinaryTxOutcome::Success(report) | OrdinaryTxOutcome::HardwareFailure(report) => {
                    Ok(report
                        .completion
                        .expect("a detached hardware completion backs this outcome"))
                }
                OrdinaryTxOutcome::HardwareTimeout(_) => Err(ControlTxError::HardwareTimeout),
                OrdinaryTxOutcome::CollisionLimit(_) => Err(ControlTxError::CollisionLimit),
            };
        }
    }
}

impl<'slot, P, E, T, H, const BUFFER_SIZE: usize> Esp32s31StaJoinTransmit<H>
    for Esp32s31ControlTx<'slot, P, E, T, BUFFER_SIZE>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
    H: TxHardware,
{
    type Error = ControlTxError;
    type PowerProfile = P;

    fn power_profile(&self) -> &Self::PowerProfile {
        Self::power_profile(self)
    }

    fn transmit_open_authentication<'a>(
        &'a mut self,
        hardware: &'a mut H,
        request: OpenAuthenticationRequest,
    ) -> impl Future<Output = Result<TxCompletion, Self::Error>> + 'a {
        Self::transmit_open_authentication(self, hardware, request)
    }

    fn transmit_association<'a>(
        &'a mut self,
        hardware: &'a mut H,
        request: AssociationRequest<'a>,
    ) -> impl Future<Output = Result<TxCompletion, Self::Error>> + 'a {
        Self::transmit_association(self, hardware, request)
    }
}

impl<P, E, T, const BUFFER_SIZE: usize> Esp32s31StaPeerTransmit
    for Esp32s31ControlTx<'_, P, E, T, BUFFER_SIZE>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
{
    fn install_ht_ampdu_policy(&mut self, parameters: HtPeerAmpduParameters) {
        Esp32s31ControlTx::install_ht_ampdu_policy(self, parameters);
    }

    fn install_he_bss_color(&mut self, bss_color: u8) {
        Esp32s31ControlTx::install_he_bss_color(self, bss_color);
    }

    fn install_wmm_edca(&mut self, parameters: WmmParameterSet) -> Result<(), EdcaParametersError> {
        Esp32s31ControlTx::install_wmm_edca(self, parameters)
    }

    fn install_tx_protection_policy(&mut self, policy: WifiTxProtectionPolicy) {
        Esp32s31ControlTx::install_tx_protection_policy(self, policy);
    }
}

impl<'slot, P, E, T, H, const BUFFER_SIZE: usize> Esp32s31Wpa2Transmit<H>
    for Esp32s31ControlTx<'slot, P, E, T, BUFFER_SIZE>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
    H: TxHardware,
{
    type Error = ControlTxError;

    fn transmit_unprotected<'a>(
        &'a mut self,
        hardware: &'a mut H,
        frame: StaDataFrame<'a>,
    ) -> impl Future<Output = Result<TxCompletion, Self::Error>> + 'a {
        self.transmit_unprotected_data(hardware, frame)
    }

    fn transmit_protected<'a>(
        &'a mut self,
        hardware: &'a mut H,
        frame: StaProtectedDataFrame<'a>,
        queue: LegacyTxQueue,
        rate: TxPhyRate,
        hardware_key_selector: u8,
    ) -> impl Future<Output = Result<TxCompletion, Self::Error>> + 'a {
        self.transmit_protected_data(hardware, frame, queue, rate, hardware_key_selector)
    }
}

#[cfg(test)]
mod tests;
