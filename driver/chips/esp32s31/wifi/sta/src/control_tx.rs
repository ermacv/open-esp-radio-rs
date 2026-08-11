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
    tx_runtime::{UnicastRetryError, WifiTxRuntimePolicy},
};
use open_esp_radio_ieee80211::{
    management::{ProbeRequest, ProbeRequestError},
    station::{
        AssociationRequest, AssociationRequestError, OpenAuthenticationRequest, StaDataFrame,
        StaProtectedDataFrame, StationFrameError,
    },
    wmm::WmmParameterSet,
};
use open_esp_radio_wifi_softmac::MacTxPlan;

use crate::{
    join::Esp32s31StaJoinTransmit, peer::Esp32s31StaPeerTransmit,
    single_mpdu_tx::Esp32s31SingleMpduTx, wpa2::Esp32s31Wpa2Transmit,
};

use open_esp_radio_esp32s31_wifi::{
    ordinary_tx::{
        OrdinaryTxError, OrdinaryTxOutcome, OrdinaryTxOwner, OrdinaryTxPlan, TX_CCMP_MIC_SIZE,
        TX_METADATA_SIZE, TxResetReason, WifiTxEntropy, WifiTxPowerProfile, WifiTxTimer,
    },
    tx::WifiTxProgress,
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
    UnsupportedHeOrdinaryMpdu,
    BufferSizeOverflow,
    DeadlineOverflow,
    Tx(TxError),
    Retry(UnicastRetryError),
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
                | Self::UnsupportedHeOrdinaryMpdu
                | Self::BufferSizeOverflow
                | Self::DeadlineOverflow
                | Self::Retry(_)
                | Self::HardwareTimeout
                | Self::CollisionLimit
        )
    }
}

impl From<OrdinaryTxError> for ControlTxError {
    fn from(error: OrdinaryTxError) -> Self {
        match error {
            OrdinaryTxError::Busy => Self::Busy,
            OrdinaryTxError::UnsupportedHeOrdinaryMpdu => Self::UnsupportedHeOrdinaryMpdu,
            OrdinaryTxError::BufferSizeOverflow => Self::BufferSizeOverflow,
            OrdinaryTxError::DeadlineOverflow => Self::DeadlineOverflow,
            OrdinaryTxError::Tx(error) => Self::Tx(error),
            OrdinaryTxError::Retry(error) => Self::Retry(error),
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
            .encode(&mut self.ordinary.buffer_mut()?[TX_METADATA_SIZE..])
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
            key,
            sequences,
            config,
        } = handoff;
        Ok(Esp32s31SingleMpduTx::from_ordinary(
            self.ordinary,
            key,
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
mod tests {
    use core::{
        future::{Future, ready},
        pin::Pin,
    };

    use open_esp_radio_esp32s31_pac::{
        MacKeyInstallOutcome, MacLegacyTxProgram, MacTxCompletionRegisters, MacTxDetachOutcome,
        MacTxDetachReason, MacTxQueueDetached,
    };
    use open_esp_radio_esp32s31_wifi_mac::{
        crypto::{CcmpKeyHardware, install_sta_pairwise_ccmp},
        tx::{HardwareOwnedTxDma, PreparedTxDma, TxSlot, TxSlotState},
    };
    use open_esp_radio_ieee80211::station::StaTxSequenceCounters;

    use super::*;
    use crate::single_mpdu_tx::SingleMpduTxConfig;
    use open_esp_radio_esp32s31_wifi::ordinary_tx::WifiTxPowerPair;

    #[derive(Default)]
    struct Hardware {
        prepare: bool,
        publications: u8,
        completions: [Option<MacTxCompletionRegisters>; 2],
        completion_index: usize,
        timeout: bool,
        legacy: Option<(u8, MacLegacyTxProgram)>,
    }

    impl TxHardware for Hardware {
        fn prepare_bound_legacy_tx(
            &mut self,
            _dma: &dyn PreparedTxDma,
            queue: u8,
            program: MacLegacyTxProgram,
        ) -> bool {
            self.legacy = Some((queue, program));
            self.prepare
        }

        fn start_bound_legacy_tx(
            &mut self,
            _dma: &dyn HardwareOwnedTxDma,
            _queue: u8,
            _plcp0: u32,
        ) {
            self.publications += 1;
        }

        fn take_tx_completion(&mut self, _queue: u8) -> Option<MacTxCompletionRegisters> {
            let completion = self.completions.get_mut(self.completion_index)?.take();
            if completion.is_some() {
                self.completion_index += 1;
            }
            completion
        }

        fn begin_tx_timeout_abort(&mut self, _queue: u8) -> bool {
            self.timeout
        }

        fn with_tx_queue_detached<R>(
            &mut self,
            _queue: u8,
            expected_descriptor_head: u32,
            reason: MacTxDetachReason,
            detached: impl for<'detached> FnOnce(MacTxQueueDetached<'detached>) -> R,
        ) -> MacTxDetachOutcome<R> {
            match reason {
                MacTxDetachReason::Timeout if !self.timeout => MacTxDetachOutcome::NoEvent,
                MacTxDetachReason::Timeout => {
                    self.timeout = false;
                    MacTxDetachOutcome::Detached(detached(MacTxQueueDetached::new_model(
                        expected_descriptor_head,
                    )))
                }
                MacTxDetachReason::Completed => MacTxDetachOutcome::Detached(detached(
                    MacTxQueueDetached::new_model(expected_descriptor_head),
                )),
                MacTxDetachReason::Collision => MacTxDetachOutcome::NoEvent,
            }
        }
    }

    impl CcmpKeyHardware for Hardware {
        fn install_sta_ccmp_entry(&mut self, _index: u8, _words: [u32; 6]) -> MacKeyInstallOutcome {
            MacKeyInstallOutcome::Installed
        }

        fn clear_ccmp_entry(&mut self, _index: u8) {}
    }

    #[derive(Clone, Copy)]
    struct Power;

    impl WifiTxPowerProfile for Power {
        fn power_pair(&self, _rate_code: u8) -> WifiTxPowerPair {
            WifiTxPowerPair {
                primary: 5,
                alternate: 6,
            }
        }
    }

    #[derive(Default)]
    struct Timer {
        now: u64,
        settled: u64,
    }

    impl WifiTxTimer for Timer {
        fn now_micros(&self) -> u64 {
            self.now
        }

        fn wait_until(&mut self, deadline_micros: u64) -> impl Future<Output = ()> + '_ {
            self.now = deadline_micros;
            ready(())
        }

        fn after_micros(&mut self, micros: u64) -> impl Future<Output = ()> + '_ {
            self.now += micros;
            self.settled += micros;
            ready(())
        }
    }

    fn completion(status: u8) -> MacTxCompletionRegisters {
        MacTxCompletionRegisters {
            aux_a: 0,
            aux_b: 0,
            aux_c: 0,
            primary: u32::from(status) << 12,
            alternate: 0,
            trigger_flow: false,
        }
    }

    fn make_tx<'a>(
        slot: Pin<&'a mut TxSlot<256>>,
    ) -> Esp32s31ControlTx<'a, Power, fn() -> u32, Timer, 256> {
        fn entropy() -> u32 {
            0x1234_5678
        }
        Esp32s31ControlTx::new(
            WifiTxResources {
                slot,
                policy: WifiTxRuntimePolicy::vendor_defaults(),
                power: Power,
                entropy,
                timer: Timer::default(),
            },
            ControlTxConfig {
                unicast_attempt_limit: 2,
                completion_timeout_us: 10,
                poll_interval_us: 1,
            },
        )
    }

    #[test]
    fn authentication_is_encoded_and_completed_by_the_shared_owner() {
        let mut slot = core::pin::pin!(TxSlot::<256>::new_model());
        let mut hardware = Hardware {
            prepare: true,
            completions: [Some(completion(0)), None],
            ..Hardware::default()
        };
        let mut tx = make_tx(slot.as_mut());

        let result = crate::test_support::block_on(tx.transmit_open_authentication(
            &mut hardware,
            OpenAuthenticationRequest {
                source: [2, 3, 4, 5, 6, 7],
                bssid: [0x20, 0x21, 0x22, 0x23, 0x24, 0x25],
                sequence_number: 7,
            },
        ));

        assert!(matches!(result, Ok(TxCompletion { status: 0, .. })));
        assert_eq!(hardware.publications, 1);
        let (_, program) = hardware.legacy.expect("management publication");
        assert_eq!(program.scheduler_priority, 1);
        assert_eq!(program.packet_priority, 1);
        assert_eq!(tx.ordinary.slot.state(), TxSlotState::Free);
        let bytes = tx.ordinary.slot.as_mut().buffer_mut().unwrap();
        assert_eq!(&bytes[TX_METADATA_SIZE..TX_METADATA_SIZE + 2], &[0xb0, 0]);
        assert_eq!(
            &bytes[TX_METADATA_SIZE + 22..TX_METADATA_SIZE + 24],
            &[0x70, 0]
        );
    }

    #[test]
    fn ack_timeout_reuses_sequence_and_marks_the_retry_bit() {
        let mut slot = core::pin::pin!(TxSlot::<256>::new_model());
        let mut hardware = Hardware {
            prepare: true,
            completions: [Some(completion(5)), Some(completion(0))],
            ..Hardware::default()
        };
        let mut tx = make_tx(slot.as_mut());

        let result = crate::test_support::block_on(tx.transmit_open_authentication(
            &mut hardware,
            OpenAuthenticationRequest {
                source: [2, 3, 4, 5, 6, 7],
                bssid: [0x20, 0x21, 0x22, 0x23, 0x24, 0x25],
                sequence_number: 11,
            },
        ));

        assert!(result.is_ok());
        assert_eq!(hardware.publications, 2);
        let bytes = tx.ordinary.slot.as_mut().buffer_mut().unwrap();
        assert_ne!(bytes[TX_METADATA_SIZE + 1] & 0x08, 0);
        assert_eq!(
            &bytes[TX_METADATA_SIZE + 22..TX_METADATA_SIZE + 24],
            &[0xb0, 0]
        );
    }

    #[test]
    fn eapol_uses_the_recovered_voice_data_priority() {
        let mut slot = core::pin::pin!(TxSlot::<256>::new_model());
        let mut hardware = Hardware {
            prepare: true,
            completions: [Some(completion(0)), None],
            ..Hardware::default()
        };
        let mut tx = make_tx(slot.as_mut());

        let result = crate::test_support::block_on(tx.transmit_unprotected_data(
            &mut hardware,
            StaDataFrame {
                source: [2, 3, 4, 5, 6, 7],
                bssid: [0x20, 0x21, 0x22, 0x23, 0x24, 0x25],
                destination: [0x20, 0x21, 0x22, 0x23, 0x24, 0x25],
                sequence_number: 8,
                ether_type: 0x888e,
                payload: &[1, 2, 3, 4],
            },
        ));

        assert!(result.is_ok());
        let (_, program) = hardware.legacy.expect("EAPOL publication");
        assert_eq!(program.scheduler_priority, 3);
        assert_eq!(program.packet_priority, 3);
    }

    #[test]
    fn missing_hardware_timeout_edge_quarantines_without_drop_panic() {
        let mut slot = std::boxed::Box::pin(TxSlot::<256>::new_model());
        let mut hardware = Hardware {
            prepare: true,
            ..Hardware::default()
        };
        let mut tx = make_tx(slot.as_mut());

        let result = crate::test_support::block_on(tx.transmit_open_authentication(
            &mut hardware,
            OpenAuthenticationRequest {
                source: [2, 3, 4, 5, 6, 7],
                bssid: [0x20, 0x21, 0x22, 0x23, 0x24, 0x25],
                sequence_number: 13,
            },
        ));

        assert_eq!(
            result,
            Err(ControlTxError::RadioResetRequired(
                TxResetReason::ExecutorDeadline
            ))
        );
        assert_eq!(tx.ordinary.slot.state(), TxSlotState::ResetRequired);
        assert!(tx.ordinary.slot.as_mut().reserve(64, 32).is_err());
        drop(tx);
        let drop_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(slot)));
        assert!(drop_result.is_ok());
    }

    #[test]
    fn passive_fallback_requires_a_proven_quiescent_tx_owner() {
        assert!(ControlTxError::HardwareTimeout.retains_quiescent_owner());
        assert!(ControlTxError::CollisionLimit.retains_quiescent_owner());
        assert!(!ControlTxError::Busy.retains_quiescent_owner());
        assert!(
            !ControlTxError::RadioResetRequired(TxResetReason::ExecutorDeadline)
                .retains_quiescent_owner()
        );
    }

    #[test]
    fn connected_handoff_preserves_the_descriptor_and_association_policy() {
        let mut slot = core::pin::pin!(TxSlot::<256>::new_model());
        let mut hardware = Hardware::default();
        let mut tx = make_tx(slot.as_mut());
        tx.install_he_bss_color(37);
        let key = install_sta_pairwise_ccmp(
            &mut hardware,
            [0x20, 0x21, 0x22, 0x23, 0x24, 0x25],
            &[0x5a; 16],
        )
        .unwrap();

        let connected = tx
            .try_into_connected(ConnectedTxHandoff {
                key,
                sequences: StaTxSequenceCounters::new(9),
                config: SingleMpduTxConfig {
                    station_address: [2, 3, 4, 5, 6, 7],
                    bssid: [0x20, 0x21, 0x22, 0x23, 0x24, 0x25],
                    peer_qos: true,
                    exchange: MacTxPlan {
                        access_category: LegacyTxQueue::BestEffort.access_category(),
                        initial_rate: TxPhyRate::Legacy(LegacyRate::Ofdm54M),
                        publication_limit: 2,
                        publication_timeout_micros: 10,
                    },
                },
            })
            .unwrap_or_else(|_| panic!("idle owner must transfer"));

        assert_eq!(connected.policy().he_bss_color(), 37);
        assert!(!connected.active());
    }

    #[test]
    fn active_handoff_returns_tx_and_crypto_resources_for_later_retry() {
        let mut slot = core::pin::pin!(TxSlot::<256>::new_model());
        let mut hardware = Hardware {
            prepare: true,
            ..Hardware::default()
        };
        let mut tx = make_tx(slot.as_mut());
        let frame_length = OpenAuthenticationRequest {
            source: [2, 3, 4, 5, 6, 7],
            bssid: [0x20, 0x21, 0x22, 0x23, 0x24, 0x25],
            sequence_number: 15,
        }
        .encode(&mut tx.ordinary.buffer_mut().unwrap()[TX_METADATA_SIZE..])
        .unwrap();
        tx.ordinary
            .start(
                &mut hardware,
                OrdinaryTxPlan {
                    frame_length,
                    descriptor_capacity: None,
                    exchange: MacTxPlan {
                        access_category: LegacyTxQueue::Voice.access_category(),
                        initial_rate: TxPhyRate::Legacy(LegacyRate::Dsss1MLong),
                        publication_limit: 1,
                        publication_timeout_micros: 10,
                    },
                    hardware_mic_length: 0,
                    hardware_key_selector: 0,
                    scheduler_priority: 1,
                    packet_priority: 1,
                },
            )
            .unwrap();
        let key = install_sta_pairwise_ccmp(
            &mut hardware,
            [0x20, 0x21, 0x22, 0x23, 0x24, 0x25],
            &[0x5a; 16],
        )
        .unwrap();
        let key_index = key.hardware_index();
        let handoff = ConnectedTxHandoff {
            key,
            sequences: StaTxSequenceCounters::new(9),
            config: SingleMpduTxConfig {
                station_address: [2, 3, 4, 5, 6, 7],
                bssid: [0x20, 0x21, 0x22, 0x23, 0x24, 0x25],
                peer_qos: true,
                exchange: MacTxPlan {
                    access_category: LegacyTxQueue::BestEffort.access_category(),
                    initial_rate: TxPhyRate::Legacy(LegacyRate::Ofdm54M),
                    publication_limit: 2,
                    publication_timeout_micros: 10,
                },
            },
        };

        let (mut tx, handoff) = match tx.try_into_connected(handoff) {
            Err(resources) => resources,
            Ok(_) => panic!("hardware-owned descriptor must reject handoff"),
        };
        assert_eq!(tx.ordinary.slot.state(), TxSlotState::HardwareOwned);
        assert_eq!(handoff.key.hardware_index(), key_index);

        hardware.completions[0] = Some(completion(0));
        assert_eq!(
            crate::test_support::block_on(tx.ordinary.service_polling(&mut hardware, 1)),
            Ok(WifiTxProgress::Complete)
        );
        assert!(tx.try_into_connected(handoff).is_ok());
    }
}
