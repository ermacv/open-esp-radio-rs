//! Production owner for one connected-station legacy/HT MPDU transaction.
//!
//! This module owns the executor-independent transaction between an encoded
//! station frame and the ESP32-S31 ordinary TX queue. It deliberately owns no logging,
//! benchmark statistics, NVS state or RTOS adapter. Every PAC borrow ends
//! before a timer is awaited.

use core::future::Future;

use open_esp_radio_esp32s31_wifi_mac::{
    crypto::{CcmpTxPacketNumberError, StaPairwiseCcmpSlot},
    tx::{LegacyTxQueue, TxError, TxHardware, TxPhyRate},
    tx_protection::TxProtectionAdmissionError,
    tx_runtime::{OrdinaryRetryError, WifiTxRuntimePolicy, WifiTxTraffic, WifiTxTrafficError},
};
use open_esp_radio_ieee80211::management::ProbeRequest;
use open_esp_radio_ieee80211::station::{
    StaActionFrame, StaDataFrame, StaProtectedDataFrame, StaProtectedEthernetFrame,
    StaTxSequenceCounters, StationFrameError,
};
use open_esp_radio_ieee80211::station_power_save::{
    StaAssociationId, StaNullDataFrame, StaPowerManagement, StaPsPollFrame,
};
use open_esp_radio_ieee80211::{
    channel::WifiChannel, esp_now::EspNowRandomValue, security::WifiSecurityMode,
};
use open_esp_radio_wifi_softmac::{
    EspNowPeerId, EspNowProtocol, EspNowSendError, EspNowV2SendError, MacTxPlan, MacTxQueueState,
    interface::BoundVirtualInterface,
};

use open_esp_radio_esp32s31_wifi::esp_now::{
    Esp32s31EspNowTxConfig, Esp32s31EspNowTxError, start_esp_now_v1_plaintext,
    start_esp_now_v2_plaintext,
};
use open_esp_radio_esp32s31_wifi::ordinary_tx::{
    OrdinaryTxError, OrdinaryTxOwner, OrdinaryTxParked, OrdinaryTxPlan, TX_CCMP_MIC_SIZE,
    TX_METADATA_SIZE,
};
pub use open_esp_radio_esp32s31_wifi::ordinary_tx::{
    OrdinaryTxOutcome as SingleMpduTxOutcome, OrdinaryTxReport as SingleMpduTxReport,
    TxResetReason, WifiTxEntropy, WifiTxPowerPair, WifiTxPowerProfile, WifiTxResources,
    WifiTxTimer,
};
use open_esp_radio_esp32s31_wifi::tx::{WifiTxProgress, WifiTxWake};

/// Association-derived inputs for the first ordinary connected-data slice.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SingleMpduTxConfig {
    pub station_address: [u8; 6],
    pub bssid: [u8; 6],
    pub peer_qos: bool,
    /// Chip-independent exchange policy selected at the association handoff.
    pub exchange: MacTxPlan<TxPhyRate>,
}

impl SingleMpduTxConfig {
    /// Derive the effective frame geometry for the selected security mode.
    /// The current Open encoder is intentionally non-QoS; keeping a peer's
    /// WMM bit here would consume the wrong sequence space and imply an
    /// unsupported plaintext A-MPDU path.
    pub const fn for_security(mut self, security: WifiSecurityMode) -> Self {
        if matches!(security, WifiSecurityMode::Open) {
            self.peer_qos = false;
        }
        self
    }
}

/// Protocol resources installed at the WPA2-to-connected TX handoff.
///
/// Keeping the key token, all independent sequence spaces and the negotiated
/// publication policy in one value prevents a partial transition from
/// constructing a connected transmitter with mismatched session state.
pub struct ConnectedTxHandoff {
    pub security: ConnectedTxSecurity,
    pub sequences: StaTxSequenceCounters,
    pub config: SingleMpduTxConfig,
}

/// Pairwise TX ownership for one connected station epoch.
pub enum ConnectedTxSecurity {
    Open,
    Wpa2Personal(StaPairwiseCcmpSlot),
}

impl ConnectedTxSecurity {
    pub const fn mode(&self) -> WifiSecurityMode {
        match self {
            Self::Open => WifiSecurityMode::Open,
            Self::Wpa2Personal(_) => WifiSecurityMode::Wpa2Personal,
        }
    }

    pub const fn hardware_key_selector(&self) -> u8 {
        match self {
            Self::Open => 0,
            Self::Wpa2Personal(key) => key.hardware_index(),
        }
    }
}

/// Queue priorities for one unprotected connected Action frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ActionTxConfig {
    pub scheduler_priority: u8,
    pub packet_priority: u8,
}

impl ActionTxConfig {
    /// Neutral profile for standards-defined unprotected management actions.
    /// It intentionally retains the ordinary queue-zero priorities without
    /// implying that a vendor-specific wire format is being sent.
    pub const STANDARD_MANAGEMENT: Self = Self {
        scheduler_priority: 1,
        packet_priority: 1,
    };

    /// Profile used by ordinary management frames recovered from the vendor
    /// queue-zero path.
    pub const VENDOR_MANAGEMENT: Self = Self::STANDARD_MANAGEMENT;

    /// Profile retained by the recovered connected RX ADDBA response path.
    pub const RX_ADDBA_RESPONSE: Self = Self {
        scheduler_priority: 0,
        packet_priority: 0,
    };
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SingleMpduTxError {
    Busy,
    /// The abstract control owner has no PS-Poll publication implementation.
    PsPollUnsupported,
    EthernetFrameTooShort,
    SecurityModeMismatch,
    PacketNumber(CcmpTxPacketNumberError),
    BufferSizeOverflow,
    DeadlineOverflow,
    ProbeEncode,
    Encode(StationFrameError),
    Tx(TxError),
    Retry(OrdinaryRetryError),
    Protection(TxProtectionAdmissionError),
    Traffic(WifiTxTrafficError),
    TrafficSelectionMismatch {
        expected: WifiTxTraffic,
        provided: WifiTxTraffic,
    },
    RadioResetRequired(TxResetReason),
}

/// Failure before one plaintext ESP-NOW request acquires the connected ordinary-TX
/// transaction. Protocol admission and chip publication remain separate so
/// applications can distinguish a stale peer from a hardware/PHY rejection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SingleMpduEspNowTxError {
    Protocol(EspNowSendError),
    V2Protocol(EspNowV2SendError),
    Backend(Esp32s31EspNowTxError),
}

impl From<EspNowSendError> for SingleMpduEspNowTxError {
    fn from(error: EspNowSendError) -> Self {
        Self::Protocol(error)
    }
}

impl From<EspNowV2SendError> for SingleMpduEspNowTxError {
    fn from(error: EspNowV2SendError) -> Self {
        Self::V2Protocol(error)
    }
}

impl From<Esp32s31EspNowTxError> for SingleMpduEspNowTxError {
    fn from(error: Esp32s31EspNowTxError) -> Self {
        Self::Backend(error)
    }
}

impl From<TxError> for SingleMpduTxError {
    fn from(error: TxError) -> Self {
        Self::Tx(error)
    }
}

impl From<OrdinaryRetryError> for SingleMpduTxError {
    fn from(error: OrdinaryRetryError) -> Self {
        Self::Retry(error)
    }
}

impl From<WifiTxTrafficError> for SingleMpduTxError {
    fn from(error: WifiTxTrafficError) -> Self {
        Self::Traffic(error)
    }
}

impl From<CcmpTxPacketNumberError> for SingleMpduTxError {
    fn from(error: CcmpTxPacketNumberError) -> Self {
        Self::PacketNumber(error)
    }
}

impl From<OrdinaryTxError> for SingleMpduTxError {
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

/// Unique ordinary-MPDU descriptor, crypto PN and retry owner.
pub struct Esp32s31SingleMpduTx<'slot, P, E, T, const BUFFER_SIZE: usize> {
    ordinary: OrdinaryTxOwner<'slot, P, E, T, BUFFER_SIZE>,
    security: ConnectedTxSecurity,
    sequences: StaTxSequenceCounters,
    config: SingleMpduTxConfig,
}

/// Opaque station-local state retained while another VIF owns physical TX.
pub struct Esp32s31SingleMpduTxParked {
    ordinary: OrdinaryTxParked,
    handoff: ConnectedTxHandoff,
}

impl<'slot, P, E, T, const BUFFER_SIZE: usize> Esp32s31SingleMpduTx<'slot, P, E, T, BUFFER_SIZE>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
{
    pub fn new(
        resources: WifiTxResources<'slot, P, E, T, BUFFER_SIZE>,
        handoff: ConnectedTxHandoff,
    ) -> Self {
        let ConnectedTxHandoff {
            security,
            sequences,
            config,
        } = handoff;
        Self {
            ordinary: OrdinaryTxOwner::new(resources),
            security,
            sequences,
            config,
        }
    }

    /// Resume from the exact opaque station state produced by
    /// [`Self::try_park`].
    pub fn resume(
        resources: WifiTxResources<'slot, P, E, T, BUFFER_SIZE>,
        parked: Esp32s31SingleMpduTxParked,
    ) -> Self {
        let Esp32s31SingleMpduTxParked { ordinary, handoff } = parked;
        let ConnectedTxHandoff {
            security,
            sequences,
            config,
        } = handoff;
        Self {
            ordinary: OrdinaryTxOwner::resume(resources, ordinary),
            security,
            sequences,
            config,
        }
    }

    pub(crate) fn from_ordinary(
        ordinary: OrdinaryTxOwner<'slot, P, E, T, BUFFER_SIZE>,
        security: ConnectedTxSecurity,
        sequences: StaTxSequenceCounters,
        config: SingleMpduTxConfig,
    ) -> Self {
        Self {
            ordinary,
            security,
            sequences,
            config,
        }
    }

    pub const fn active(&self) -> bool {
        self.ordinary.active()
    }

    pub fn queue_state(&self) -> MacTxQueueState {
        self.ordinary.queue_state()
    }

    /// Exact ordinary descriptor lifecycle retained for bounded diagnostics.
    pub fn slot_state(&self) -> open_esp_radio_esp32s31_wifi_mac::tx::TxSlotState {
        self.ordinary.slot_state()
    }

    /// Current ordinary descriptor ownership word retained for diagnostics.
    pub fn descriptor_word0(&self) -> u32 {
        self.ordinary.descriptor_word0()
    }

    pub const fn policy(&self) -> &WifiTxRuntimePolicy {
        self.ordinary.policy()
    }

    pub fn policy_mut(&mut self) -> &mut WifiTxRuntimePolicy {
        self.ordinary.policy_mut()
    }

    pub fn select_network_traffic(
        &self,
        ethernet: &[u8],
    ) -> Result<WifiTxTraffic, WifiTxTrafficError> {
        self.policy()
            .select_network_traffic(ethernet, self.config.peer_qos)
    }

    pub fn take_last_outcome(&mut self) -> Option<SingleMpduTxOutcome> {
        self.ordinary.take_last_outcome()
    }

    pub const fn last_outcome(&self) -> Option<SingleMpduTxOutcome> {
        self.ordinary.last_outcome()
    }

    pub const fn power_profile(&self) -> &P {
        self.ordinary.power()
    }

    pub fn contention_publication(
        &mut self,
        queue: LegacyTxQueue,
    ) -> (
        open_esp_radio_esp32s31_wifi_mac::edca::EdcaContentionParameters,
        u16,
    ) {
        self.ordinary.contention_publication(queue)
    }

    pub fn record_retry_failure(&mut self, queue: LegacyTxQueue) {
        self.ordinary.record_retry_failure(queue);
    }

    pub fn record_success(&mut self, queue: LegacyTxQueue) {
        self.ordinary.record_success(queue);
    }

    pub fn reset_terminal_exchange(&mut self, queue: LegacyTxQueue) {
        self.ordinary.reset_terminal_exchange(queue);
    }

    pub fn after_micros(&mut self, micros: u64) -> impl Future<Output = ()> + '_ {
        self.ordinary.after_micros(micros)
    }

    /// Split an idle connected transmitter back into reusable descriptor
    /// resources and its association-owned key/sequence handoff.
    ///
    /// This is the inverse ownership edge of
    /// [`crate::control_tx::Esp32s31ControlTx::try_into_connected`]. It does
    /// not clear the hardware key: an outer station teardown must consume the
    /// returned token through `StaPairwiseCcmpSlot::clear` using the unique
    /// hardware owner.
    #[allow(clippy::result_large_err)]
    pub fn try_into_parts(
        self,
    ) -> Result<
        (
            WifiTxResources<'slot, P, E, T, BUFFER_SIZE>,
            ConnectedTxHandoff,
        ),
        Self,
    > {
        let Self {
            ordinary,
            security,
            sequences,
            config,
        } = self;
        match ordinary.try_into_resources() {
            Ok(resources) => Ok((
                resources,
                ConnectedTxHandoff {
                    security,
                    sequences,
                    config,
                },
            )),
            Err(ordinary) => Err(Self {
                ordinary,
                security,
                sequences,
                config,
            }),
        }
    }

    /// Separate idle physical resources from opaque station-local callback,
    /// key and sequence state without losing a terminal TX observation.
    #[allow(clippy::result_large_err)]
    pub fn try_park(
        self,
    ) -> Result<
        (
            WifiTxResources<'slot, P, E, T, BUFFER_SIZE>,
            Esp32s31SingleMpduTxParked,
        ),
        Self,
    > {
        let Self {
            ordinary,
            security,
            sequences,
            config,
        } = self;
        match ordinary.try_park() {
            Ok((resources, ordinary)) => Ok((
                resources,
                Esp32s31SingleMpduTxParked {
                    ordinary,
                    handoff: ConnectedTxHandoff {
                        security,
                        sequences,
                        config,
                    },
                },
            )),
            Err(ordinary) => Err(Self {
                ordinary,
                security,
                sequences,
                config,
            }),
        }
    }

    pub fn peek_qos_sequence(&self, tid: u8) -> Option<u16> {
        self.sequences.peek_qos(tid)
    }

    pub fn take_protected_metadata(
        &mut self,
        tid: u8,
    ) -> Result<Option<StaProtectedEthernetFrame>, CcmpTxPacketNumberError> {
        let ConnectedTxSecurity::Wpa2Personal(key) = &mut self.security else {
            return Ok(None);
        };
        if self.sequences.peek_qos(tid).is_none() {
            return Ok(None);
        }
        let ccmp_header = key.next_tx_ccmp_header()?;
        let sequence_number = self
            .sequences
            .take_data(Some(tid))
            .expect("validated QoS sequence space remains owned");
        Ok(Some(StaProtectedEthernetFrame {
            bssid: self.config.bssid,
            sequence_number,
            user_priority: tid,
            peer_qos: self.config.peer_qos,
            ccmp_header,
        }))
    }

    pub const fn hardware_key_selector(&self) -> u8 {
        self.security.hardware_key_selector()
    }

    pub const fn security_mode(&self) -> WifiSecurityMode {
        self.security.mode()
    }

    pub const fn config(&self) -> SingleMpduTxConfig {
        self.config
    }

    /// Publish one exact protected MPDU copied from a detached aggregate.
    /// Sequence Control and CCMP PN are already present and must not be
    /// allocated again; only the IEEE Retry bit is added.
    pub fn copy_encoded_retry(&mut self, encoded: &[u8]) -> Result<usize, SingleMpduTxError> {
        if self.security_mode() == WifiSecurityMode::Open {
            return Err(SingleMpduTxError::SecurityModeMismatch);
        }
        if self.ordinary.active() {
            return Err(SingleMpduTxError::Busy);
        }
        let frame_length = encoded.len();
        if frame_length < 2 {
            return Err(SingleMpduTxError::EthernetFrameTooShort);
        }
        let output = self.ordinary.buffer_mut()?;
        let end = TX_METADATA_SIZE
            .checked_add(frame_length)
            .ok_or(SingleMpduTxError::BufferSizeOverflow)?;
        output
            .get_mut(TX_METADATA_SIZE..end)
            .ok_or(SingleMpduTxError::BufferSizeOverflow)?
            .copy_from_slice(encoded);
        output[TX_METADATA_SIZE + 1] |= 0x08;
        Ok(frame_length)
    }

    pub fn start_prepared_encoded_retry<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        frame_length: usize,
        hardware_mic_length: usize,
        rate: TxPhyRate,
    ) -> Result<WifiTxProgress, SingleMpduTxError> {
        self.start_prepared_encoded_retry_for_category(
            hardware,
            frame_length,
            hardware_mic_length,
            rate,
            self.config.exchange.access_category,
        )
    }

    pub fn start_prepared_encoded_retry_for_category<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        frame_length: usize,
        hardware_mic_length: usize,
        rate: TxPhyRate,
        access_category: open_esp_radio_ieee80211::wmm::WmmAccessCategory,
    ) -> Result<WifiTxProgress, SingleMpduTxError> {
        if self.security_mode() == WifiSecurityMode::Open || hardware_mic_length == 0 {
            return Err(SingleMpduTxError::SecurityModeMismatch);
        }
        let queue = LegacyTxQueue::from_access_category(access_category);
        self.ordinary
            .start(
                hardware,
                OrdinaryTxPlan {
                    frame_length,
                    descriptor_capacity: None,
                    exchange: MacTxPlan {
                        access_category,
                        initial_rate: rate,
                        publication_limit: self.config.exchange.publication_limit,
                        publication_timeout_micros: self.config.exchange.publication_timeout_micros,
                    },
                    hardware_mic_length,
                    hardware_key_selector: self.security.hardware_key_selector(),
                    interface:
                        open_esp_radio_esp32s31_wifi::ordinary_tx::OrdinaryTxInterface::Station,
                    scheduler_priority: queue.vendor_data_scheduler_priority(),
                    packet_priority: queue.vendor_data_packet_priority(),
                },
            )
            .map_err(Into::into)
    }

    pub fn now_micros(&self) -> u64 {
        self.ordinary.now_micros()
    }

    pub fn wait_until_micros(&mut self, deadline_micros: u64) -> impl Future<Output = ()> + '_ {
        self.ordinary.wait_until(deadline_micros)
    }

    /// Copy and encode one Ethernet frame, then publish the first DMA attempt.
    pub fn start<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        ethernet: &[u8],
    ) -> Result<WifiTxProgress, SingleMpduTxError> {
        let traffic = self.select_network_traffic(ethernet)?;
        self.start_with_traffic(hardware, ethernet, traffic)
    }

    /// Publish one classified network MPDU through the matching EDCA queue
    /// and QoS sequence space.
    pub fn start_with_traffic<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        ethernet: &[u8],
        traffic: WifiTxTraffic,
    ) -> Result<WifiTxProgress, SingleMpduTxError> {
        if self.ordinary.active() {
            return Err(SingleMpduTxError::Busy);
        }
        if ethernet.len() < 14 {
            return Err(SingleMpduTxError::EthernetFrameTooShort);
        }
        let expected = self.select_network_traffic(ethernet)?;
        if traffic != expected {
            return Err(SingleMpduTxError::TrafficSelectionMismatch {
                expected,
                provided: traffic,
            });
        }

        let destination: [u8; 6] = ethernet[..6]
            .try_into()
            .expect("validated Ethernet destination");
        self.ordinary.require_unprotected_retry_series(
            self.config.exchange.initial_rate,
            open_esp_radio_esp32s31_wifi_mac::tx_runtime::OrdinaryRetryRatePolicy::Normal,
            self.config.exchange.publication_limit,
            destination[0] & 1 != 0,
        )?;
        let source = ethernet[6..12]
            .try_into()
            .expect("validated Ethernet source");
        let ether_type = u16::from_be_bytes([ethernet[12], ethernet[13]]);
        let sequence_number = self
            .sequences
            .take_data(self.config.peer_qos.then_some(traffic.tid()))
            .expect("classified TID is a valid sequence space");
        let (frame_length, hardware_mic_length, hardware_key_selector) = {
            let buffer = self.ordinary.buffer_mut()?;
            match &mut self.security {
                ConnectedTxSecurity::Open => (
                    StaDataFrame {
                        source,
                        bssid: self.config.bssid,
                        destination,
                        sequence_number,
                        ether_type,
                        payload: &ethernet[14..],
                    }
                    .encode(&mut buffer[TX_METADATA_SIZE..])
                    .map_err(SingleMpduTxError::Encode)?,
                    0,
                    0,
                ),
                ConnectedTxSecurity::Wpa2Personal(key) => (
                    StaProtectedDataFrame {
                        source,
                        bssid: self.config.bssid,
                        destination,
                        sequence_number,
                        user_priority: traffic.tid(),
                        peer_qos: self.config.peer_qos,
                        ccmp_header: key.next_tx_ccmp_header()?,
                        ether_type,
                        payload: &ethernet[14..],
                    }
                    .encode(&mut buffer[TX_METADATA_SIZE..])
                    .map_err(SingleMpduTxError::Encode)?,
                    TX_CCMP_MIC_SIZE,
                    key.hardware_index(),
                ),
            }
        };
        let queue = traffic.queue();
        self.ordinary
            .start(
                hardware,
                OrdinaryTxPlan {
                    frame_length,
                    descriptor_capacity: None,
                    exchange: MacTxPlan {
                        access_category: traffic.access_category,
                        ..self.config.exchange
                    },
                    hardware_mic_length,
                    hardware_key_selector,
                    interface:
                        open_esp_radio_esp32s31_wifi::ordinary_tx::OrdinaryTxInterface::Station,
                    scheduler_priority: queue.vendor_data_scheduler_priority(),
                    packet_priority: queue.vendor_data_packet_priority(),
                },
            )
            .map_err(Into::into)
    }

    /// Publish a protected EAPOL packet through the connected ordinary-TX
    /// owner. Group-key responses share this transaction with management and
    /// network traffic, so they cannot bypass DMA ownership or IRQ ordering.
    pub fn start_protected_eapol<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        payload: &[u8],
    ) -> Result<WifiTxProgress, SingleMpduTxError> {
        if self.ordinary.active() {
            return Err(SingleMpduTxError::Busy);
        }
        let ConnectedTxSecurity::Wpa2Personal(key) = &mut self.security else {
            return Err(SingleMpduTxError::SecurityModeMismatch);
        };
        let sequence_number = self
            .sequences
            .take_data(self.config.peer_qos.then_some(0))
            .expect("selected EAPOL sequence-number owner exists");
        let ccmp_header = key.next_tx_ccmp_header()?;
        let frame_length = {
            let buffer = self.ordinary.buffer_mut()?;
            StaProtectedDataFrame {
                source: self.config.station_address,
                bssid: self.config.bssid,
                destination: self.config.bssid,
                sequence_number,
                user_priority: 7,
                peer_qos: self.config.peer_qos,
                ccmp_header,
                ether_type: 0x888e,
                payload,
            }
            .encode(&mut buffer[TX_METADATA_SIZE..])
            .map_err(SingleMpduTxError::Encode)?
        };
        self.ordinary
            .start(
                hardware,
                OrdinaryTxPlan {
                    frame_length,
                    descriptor_capacity: None,
                    exchange: MacTxPlan {
                        access_category: LegacyTxQueue::Voice.access_category(),
                        initial_rate: TxPhyRate::Legacy(
                            open_esp_radio_esp32s31_wifi_mac::tx::LegacyRate::Dsss1MLong,
                        ),
                        publication_limit: self.config.exchange.publication_limit,
                        publication_timeout_micros: self.config.exchange.publication_timeout_micros,
                    },
                    hardware_mic_length: TX_CCMP_MIC_SIZE,
                    hardware_key_selector: key.hardware_index(),
                    interface:
                        open_esp_radio_esp32s31_wifi::ordinary_tx::OrdinaryTxInterface::Station,
                    scheduler_priority: LegacyTxQueue::Voice.vendor_data_scheduler_priority(),
                    packet_priority: LegacyTxQueue::Voice.vendor_data_packet_priority(),
                },
            )
            .map_err(Into::into)
    }

    /// Encode and publish one unprotected connected Action management frame.
    ///
    /// The same pinned descriptor is shared with network data, so this method
    /// fails while any prior transaction is active. The runner enforces that
    /// control work is started only after the current network lease has lost
    /// hardware ownership.
    pub fn start_action<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        body: &[u8],
        config: ActionTxConfig,
    ) -> Result<WifiTxProgress, SingleMpduTxError> {
        if self.ordinary.active() {
            return Err(SingleMpduTxError::Busy);
        }
        let sequence_number = self.sequences.take_non_qos();
        let frame_length = {
            let buffer = self.ordinary.buffer_mut()?;
            StaActionFrame {
                source: self.config.station_address,
                bssid: self.config.bssid,
                sequence_number,
                body,
            }
            .encode(&mut buffer[TX_METADATA_SIZE..])
            .map_err(SingleMpduTxError::Encode)?
        };
        self.ordinary
            .start(
                hardware,
                OrdinaryTxPlan {
                    frame_length,
                    descriptor_capacity: None,
                    exchange: MacTxPlan {
                        access_category: LegacyTxQueue::Voice.access_category(),
                        initial_rate: TxPhyRate::Legacy(
                            open_esp_radio_esp32s31_wifi_mac::tx::LegacyRate::Dsss1MLong,
                        ),
                        publication_limit: self.config.exchange.publication_limit,
                        publication_timeout_micros: self.config.exchange.publication_timeout_micros,
                    },
                    hardware_mic_length: 0,
                    hardware_key_selector: 0,
                    interface:
                        open_esp_radio_esp32s31_wifi::ordinary_tx::OrdinaryTxInterface::Station,
                    scheduler_priority: config.scheduler_priority,
                    packet_priority: config.packet_priority,
                },
            )
            .map_err(Into::into)
    }

    /// Resolve, encode and publish one plaintext ESP-NOW v1 Action MPDU
    /// through the connected station's sole ordinary descriptor.
    ///
    /// `active_channel` and `active_station` must come from the same live
    /// connected owner that supplied this transmitter. The portable protocol
    /// keeps peer and requested-PHY policy typed; the chip backend remains the
    /// only authority which may admit that PHY mode.
    #[allow(clippy::too_many_arguments)]
    pub fn start_esp_now_v1_plaintext<H: TxHardware, const PEERS: usize>(
        &mut self,
        hardware: &mut H,
        protocol: &EspNowProtocol<PEERS>,
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
        // Build against a copied sequence frontier. The real shared
        // management/non-QoS counter advances only after the backend creates
        // a live ordinary transaction. Ordinary TX has no fallible step after
        // TxDmaPublication::commit changes ownership and rings the infallible
        // doorbell, so `Ok` is the exact publication edge; PHY, buffer, queue
        // and LR-frontier rejection cannot burn a sequence number.
        let mut next_sequence = *self.sequences.non_qos_mut();
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
            *self.sequences.non_qos_mut() = next_sequence;
        }
        result
    }

    /// Resolve and publish one plaintext v2 Action MPDU through the same
    /// connected ordinary transaction as v1.
    #[allow(clippy::too_many_arguments)]
    pub fn start_esp_now_v2_plaintext<H: TxHardware, const PEERS: usize>(
        &mut self,
        hardware: &mut H,
        protocol: &EspNowProtocol<PEERS>,
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
        let mut next_sequence = *self.sequences.non_qos_mut();
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
            *self.sequences.non_qos_mut() = next_sequence;
        }
        result
    }

    /// Encode and publish one directed AP reachability Probe Request.
    ///
    /// TX completion is not reachability evidence: connected control waits
    /// for a BSSID-validated Probe Response or beacon before cancelling its
    /// bounded probe sequence.
    pub fn start_beacon_probe<H: TxHardware>(
        &mut self,
        hardware: &mut H,
    ) -> Result<WifiTxProgress, SingleMpduTxError> {
        const BASIC_RATES: &[u8] = &[0x82, 0x84, 0x8b, 0x96];

        if self.ordinary.active() {
            return Err(SingleMpduTxError::Busy);
        }
        let sequence_number = self.sequences.take_non_qos();
        let frame_length = {
            let buffer = self.ordinary.buffer_mut()?;
            ProbeRequest {
                destination: self.config.bssid,
                source: self.config.station_address,
                bssid: self.config.bssid,
                sequence_number,
                ssid: b"",
                supported_rates: BASIC_RATES,
            }
            .encode(&mut buffer[TX_METADATA_SIZE..])
            .map_err(|_| SingleMpduTxError::ProbeEncode)?
        };
        self.ordinary
            .start(
                hardware,
                OrdinaryTxPlan {
                    frame_length,
                    descriptor_capacity: None,
                    exchange: MacTxPlan {
                        access_category: LegacyTxQueue::Voice.access_category(),
                        initial_rate: TxPhyRate::Legacy(
                            open_esp_radio_esp32s31_wifi_mac::tx::LegacyRate::Dsss1MLong,
                        ),
                        publication_limit: 1,
                        publication_timeout_micros: self.config.exchange.publication_timeout_micros,
                    },
                    hardware_mic_length: 0,
                    hardware_key_selector: 0,
                    interface:
                        open_esp_radio_esp32s31_wifi::ordinary_tx::OrdinaryTxInterface::Station,
                    scheduler_priority: ActionTxConfig::VENDOR_MANAGEMENT.scheduler_priority,
                    packet_priority: ActionTxConfig::VENDOR_MANAGEMENT.packet_priority,
                },
            )
            .map_err(Into::into)
    }

    /// Encode and publish a station power-management Null Data frame.
    ///
    /// A successful return only means that the MPDU owns the hardware TX
    /// transaction. Callers must wait for [`SingleMpduTxOutcome::Success`]
    /// before treating `PowerSave` as acknowledged by the access point.
    pub fn start_power_management_null<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        power_management: StaPowerManagement,
    ) -> Result<WifiTxProgress, SingleMpduTxError> {
        if self.ordinary.active() {
            return Err(SingleMpduTxError::Busy);
        }
        let sequence_number = self.sequences.take_non_qos();
        let frame_length = {
            let buffer = self.ordinary.buffer_mut()?;
            StaNullDataFrame {
                station_address: self.config.station_address,
                bssid: self.config.bssid,
                sequence_number,
                power_management,
            }
            .encode(&mut buffer[TX_METADATA_SIZE..])
            .map_err(SingleMpduTxError::Encode)?
        };
        self.ordinary
            .start(
                hardware,
                OrdinaryTxPlan {
                    frame_length,
                    descriptor_capacity: None,
                    exchange: MacTxPlan {
                        access_category: LegacyTxQueue::Voice.access_category(),
                        initial_rate: TxPhyRate::Legacy(
                            open_esp_radio_esp32s31_wifi_mac::tx::LegacyRate::Dsss1MLong,
                        ),
                        publication_limit: self.config.exchange.publication_limit,
                        publication_timeout_micros: self.config.exchange.publication_timeout_micros,
                    },
                    hardware_mic_length: 0,
                    hardware_key_selector: 0,
                    interface:
                        open_esp_radio_esp32s31_wifi::ordinary_tx::OrdinaryTxInterface::Station,
                    scheduler_priority: ActionTxConfig::VENDOR_MANAGEMENT.scheduler_priority,
                    packet_priority: ActionTxConfig::VENDOR_MANAGEMENT.packet_priority,
                },
            )
            .map_err(Into::into)
    }

    /// Encode and publish one legacy PS-Poll control frame.
    ///
    /// The ordinary unicast transaction owns ACK detection, bounded retry and
    /// terminal completion exactly as it does for PM Null Data. A successful
    /// return is not buffered-data evidence; connected control must still
    /// await one BSSID-validated delivery observation.
    pub fn start_ps_poll<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        association_id: StaAssociationId,
    ) -> Result<WifiTxProgress, SingleMpduTxError> {
        if self.ordinary.active() {
            return Err(SingleMpduTxError::Busy);
        }
        let frame_length = {
            let buffer = self.ordinary.buffer_mut()?;
            StaPsPollFrame {
                station_address: self.config.station_address,
                bssid: self.config.bssid,
                association_id,
            }
            .encode(&mut buffer[TX_METADATA_SIZE..])
            .map_err(SingleMpduTxError::Encode)?
        };
        self.ordinary
            .start(
                hardware,
                OrdinaryTxPlan {
                    frame_length,
                    descriptor_capacity: None,
                    exchange: MacTxPlan {
                        access_category: LegacyTxQueue::Voice.access_category(),
                        initial_rate: TxPhyRate::Legacy(
                            open_esp_radio_esp32s31_wifi_mac::tx::LegacyRate::Dsss1MLong,
                        ),
                        publication_limit: self.config.exchange.publication_limit,
                        publication_timeout_micros: self.config.exchange.publication_timeout_micros,
                    },
                    hardware_mic_length: 0,
                    hardware_key_selector: 0,
                    interface:
                        open_esp_radio_esp32s31_wifi::ordinary_tx::OrdinaryTxInterface::Station,
                    scheduler_priority: ActionTxConfig::VENDOR_MANAGEMENT.scheduler_priority,
                    packet_priority: ActionTxConfig::VENDOR_MANAGEMENT.packet_priority,
                },
            )
            .map_err(Into::into)
    }

    pub fn wait_deadline(&mut self) -> impl Future<Output = ()> + '_ {
        self.ordinary.wait_deadline()
    }

    pub fn next_deadline_micros(&self) -> Option<u64> {
        self.ordinary.next_deadline_micros()
    }

    /// Consume one IRQ/deadline edge and retain or release DMA ownership.
    pub fn service<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        wake: WifiTxWake,
    ) -> Result<WifiTxProgress, SingleMpduTxError> {
        self.ordinary.service(hardware, wake).map_err(Into::into)
    }
}
#[cfg(test)]
mod tests;
