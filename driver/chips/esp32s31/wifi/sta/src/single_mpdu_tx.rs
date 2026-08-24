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

        let destination = ethernet[..6]
            .try_into()
            .expect("validated Ethernet destination");
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

    /// Consume one IRQ/deadline edge and retain or release DMA ownership.
    pub async fn service<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        wake: WifiTxWake,
    ) -> Result<WifiTxProgress, SingleMpduTxError> {
        self.ordinary
            .service(hardware, wake)
            .await
            .map_err(Into::into)
    }
}
#[cfg(test)]
mod tests {
    use core::{
        future::{Future, ready},
        pin::Pin,
    };

    use open_esp_radio_esp32s31_hal::types::{
        MacKeyInstallOutcome, MacLegacyTxProgram, MacTxCompletionRegisters, MacTxDetachOutcome,
        MacTxDetachReason, MacTxQueueDetached,
    };
    use open_esp_radio_esp32s31_wifi_mac::{
        MacInterface,
        crypto::{CcmpKeyHardware, install_sta_pairwise_ccmp},
        tx::{HardwareOwnedTxDma, LegacyRate, PreparedTxDma, TxCompletion, TxSlot, TxSlotState},
        tx_runtime::VENDOR_SHORT_RETRY_LIMIT,
    };
    use open_esp_radio_ieee80211::{
        channel::WifiChannel,
        esp_now::{EspNowDestination, EspNowRandomValue, EspNowUnicastAddress},
    };
    use open_esp_radio_wifi_softmac::{
        EspNowConfig, EspNowPeerConfig, EspNowPhyMode, EspNowProtocol, MacTxResult,
        interface::{BoundVirtualInterface, ChannelContextId, VifId, VifRole, VirtualInterface},
    };

    use super::*;

    const BSSID: [u8; 6] = [0x20, 0x21, 0x22, 0x23, 0x24, 0x25];

    #[derive(Default)]
    struct Hardware {
        prepare: bool,
        publications: u8,
        completion: Option<MacTxCompletionRegisters>,
        timeout: bool,
        collision: bool,
        legacy: Option<(u8, MacLegacyTxProgram)>,
        cleared_key: Option<u8>,
    }

    impl CcmpKeyHardware for Hardware {
        fn install_sta_ccmp_entry(
            &mut self,
            _index: u8,
            _words: &[u32; 6],
        ) -> MacKeyInstallOutcome {
            MacKeyInstallOutcome::Installed
        }

        fn clear_ccmp_entry(&mut self, index: u8) {
            self.cleared_key = Some(index);
        }
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
            self.completion.take()
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
                MacTxDetachReason::Collision if !self.collision => MacTxDetachOutcome::NoEvent,
                MacTxDetachReason::Timeout => {
                    self.timeout = false;
                    MacTxDetachOutcome::Detached(detached(MacTxQueueDetached::new_model(
                        expected_descriptor_head,
                    )))
                }
                MacTxDetachReason::Collision => {
                    self.collision = false;
                    MacTxDetachOutcome::Detached(detached(MacTxQueueDetached::new_model(
                        expected_descriptor_head,
                    )))
                }
                MacTxDetachReason::Completed => MacTxDetachOutcome::Detached(detached(
                    MacTxQueueDetached::new_model(expected_descriptor_head),
                )),
            }
        }
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
    struct TestTimer {
        now: u64,
        settled: u64,
    }

    impl WifiTxTimer for TestTimer {
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

    fn ethernet() -> [u8; 18] {
        [
            0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x08, 0x00, 1,
            2, 3, 4,
        ]
    }

    fn entropy() -> u32 {
        0x1234_5678
    }

    fn make_tx<'a>(
        slot: Pin<&'a mut TxSlot<512>>,
        hardware: &mut Hardware,
        attempt_limit: u8,
    ) -> Esp32s31SingleMpduTx<'a, Power, fn() -> u32, TestTimer, 512> {
        let key = install_sta_pairwise_ccmp(hardware, BSSID, &[0x5a; 16]).unwrap();
        Esp32s31SingleMpduTx::new(
            WifiTxResources {
                slot,
                policy: WifiTxRuntimePolicy::vendor_defaults(),
                power: Power,
                entropy,
                timer: TestTimer::default(),
            },
            ConnectedTxHandoff {
                security: ConnectedTxSecurity::Wpa2Personal(key),
                sequences: StaTxSequenceCounters::new(7),
                config: SingleMpduTxConfig {
                    station_address: [2, 3, 4, 5, 6, 7],
                    bssid: BSSID,
                    peer_qos: true,
                    exchange: MacTxPlan {
                        access_category: LegacyTxQueue::BestEffort.access_category(),
                        initial_rate: TxPhyRate::Legacy(LegacyRate::Ofdm54M),
                        publication_limit: attempt_limit,
                        publication_timeout_micros: 250_000,
                    },
                },
            },
        )
    }

    fn esp_now_protocol(
        phy_mode: EspNowPhyMode,
    ) -> (
        EspNowProtocol<1>,
        open_esp_radio_wifi_softmac::EspNowPeerId,
        BoundVirtualInterface,
        WifiChannel,
    ) {
        let station = BoundVirtualInterface::new(
            VirtualInterface::new(VifId::PRIMARY, VifRole::Station, [2, 3, 4, 5, 6, 7]),
            ChannelContextId::PRIMARY,
        );
        let channel = WifiChannel::mhz20(1).unwrap();
        let mut protocol = EspNowProtocol::new(EspNowConfig::new(station, channel).unwrap());
        let peer = protocol
            .add_peer(
                EspNowPeerConfig::plaintext(
                    EspNowDestination::Unicast(
                        EspNowUnicastAddress::new([0x30, 0x31, 0x32, 0x33, 0x34, 0x35]).unwrap(),
                    ),
                    channel,
                )
                .with_phy_mode(phy_mode),
            )
            .unwrap();
        (protocol, peer, station, channel)
    }

    #[test]
    fn completion_releases_the_slot_and_network_lease_boundary() {
        let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
        let mut hardware = Hardware {
            prepare: true,
            ..Hardware::default()
        };
        let mut tx = make_tx(slot.as_mut(), &mut hardware, 4);

        assert_eq!(tx.queue_state(), MacTxQueueState::Ready);

        assert_eq!(
            tx.start(&mut hardware, &ethernet()),
            Ok(WifiTxProgress::Pending)
        );
        assert_eq!(tx.queue_state(), MacTxQueueState::Backpressured);
        assert_eq!(hardware.publications, 1);
        hardware.completion = Some(completion(0));
        assert_eq!(
            crate::test_support::block_on(tx.service(
                &mut hardware,
                WifiTxWake::Interrupt {
                    events: open_esp_radio_esp32s31_wifi_mac::irq::MAC_INT_TX_COMPLETE,
                },
            )),
            Ok(WifiTxProgress::Complete)
        );
        assert!(matches!(
            tx.take_last_outcome(),
            Some(SingleMpduTxOutcome::Success(report))
                if matches!(report.completion, Some(TxCompletion { status: 0, .. }))
                    && report.status.attempts == 1
        ));
        assert_eq!(tx.ordinary.slot.state(), TxSlotState::Free);
        assert_eq!(tx.queue_state(), MacTxQueueState::Ready);
    }

    #[test]
    fn rejected_lr_frontier_does_not_consume_the_shared_sequence() {
        let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
        let mut hardware = Hardware::default();
        let mut tx = make_tx(slot.as_mut(), &mut hardware, 4);
        let (protocol, peer, station, channel) = esp_now_protocol(EspNowPhyMode::LongRange);
        let config = Esp32s31EspNowTxConfig::new(4, 250_000).unwrap();

        let error = tx
            .start_esp_now_v1_plaintext(
                &mut hardware,
                &protocol,
                peer,
                EspNowRandomValue::new([1, 2, 3, 4]),
                &[9, 8, 7],
                channel,
                station,
                config,
            )
            .unwrap_err();
        assert!(matches!(
            error,
            SingleMpduEspNowTxError::Backend(Esp32s31EspNowTxError::LongRangeUnsupported(_))
        ));
        assert_eq!(tx.sequences.peek_non_qos(), 7);
        assert_eq!(hardware.publications, 0);
        assert_eq!(tx.ordinary.slot.state(), TxSlotState::Free);
    }

    #[test]
    fn successful_esp_now_publication_commits_one_sequence_exactly_once() {
        let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
        let mut hardware = Hardware {
            prepare: true,
            ..Hardware::default()
        };
        let mut tx = make_tx(slot.as_mut(), &mut hardware, 4);
        let (protocol, peer, station, channel) = esp_now_protocol(EspNowPhyMode::LegacyDsss1M);

        assert_eq!(
            tx.start_esp_now_v1_plaintext(
                &mut hardware,
                &protocol,
                peer,
                EspNowRandomValue::new([1, 2, 3, 4]),
                &[9, 8, 7],
                channel,
                station,
                Esp32s31EspNowTxConfig::new(4, 250_000).unwrap(),
            ),
            Ok(WifiTxProgress::Pending)
        );
        assert_eq!(tx.sequences.peek_non_qos(), 8);
        assert_eq!(hardware.publications, 1);
    }

    #[test]
    fn dscp_selects_the_matching_hardware_queue_qos_tid_and_sequence_space() {
        let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
        let mut hardware = Hardware {
            prepare: true,
            ..Hardware::default()
        };
        let mut tx = make_tx(slot.as_mut(), &mut hardware, 4);
        let mut frame = ethernet();
        frame[14] = 0x45;
        frame[15] = 46 << 2;

        assert_eq!(tx.start(&mut hardware, &frame), Ok(WifiTxProgress::Pending));
        let (queue, program) = hardware.legacy.expect("classified legacy queue image");
        assert_eq!(queue, LegacyTxQueue::Voice.hardware_index());
        assert_eq!(
            program.scheduler_priority,
            LegacyTxQueue::Voice.vendor_data_scheduler_priority()
        );
        assert_eq!(
            program.packet_priority,
            LegacyTxQueue::Voice.vendor_data_packet_priority()
        );
        assert_eq!(tx.sequences.peek_qos(6), Some(8));
        assert_eq!(tx.sequences.peek_qos(0), Some(7));

        hardware.completion = Some(completion(5));
        assert_eq!(
            crate::test_support::block_on(tx.service(
                &mut hardware,
                WifiTxWake::Interrupt {
                    events: open_esp_radio_esp32s31_wifi_mac::irq::MAC_INT_TX_COMPLETE,
                },
            )),
            Ok(WifiTxProgress::Pending)
        );
        assert_eq!(tx.policy().contention_exponent(LegacyTxQueue::Voice), 3);
        assert_eq!(
            tx.policy().contention_exponent(LegacyTxQueue::BestEffort),
            4
        );

        hardware.completion = Some(completion(0));
        assert_eq!(
            crate::test_support::block_on(tx.service(
                &mut hardware,
                WifiTxWake::Interrupt {
                    events: open_esp_radio_esp32s31_wifi_mac::irq::MAC_INT_TX_COMPLETE,
                },
            )),
            Ok(WifiTxProgress::Complete)
        );
        assert_eq!(tx.policy().contention_exponent(LegacyTxQueue::Voice), 2);
        let bytes = tx.ordinary.slot.as_mut().buffer_mut().unwrap();
        assert_eq!(bytes[TX_METADATA_SIZE + 24] & 0x0f, 6);

        let voice = tx.select_network_traffic(&frame).unwrap();
        assert!(matches!(
            tx.start_with_traffic(&mut hardware, &ethernet(), voice),
            Err(SingleMpduTxError::TrafficSelectionMismatch { provided, .. })
                if provided == voice
        ));
    }

    #[test]
    fn idle_connected_owner_returns_descriptor_key_and_sequences_for_teardown() {
        let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
        let mut hardware = Hardware::default();
        let tx = make_tx(slot.as_mut(), &mut hardware, 4);

        let (resources, handoff) = match tx.try_into_parts() {
            Ok(parts) => parts,
            Err(_) => panic!("idle connected TX must decompose"),
        };

        assert_eq!(resources.slot.state(), TxSlotState::Free);
        assert_eq!(handoff.sequences.peek_non_qos(), 7);
        let ConnectedTxSecurity::Wpa2Personal(key) = handoff.security else {
            panic!("WPA2 test owner must return its pairwise key");
        };
        let key_index = key.hardware_index();
        key.clear(&mut hardware);
        assert_eq!(hardware.cleared_key, Some(key_index));
    }

    #[test]
    fn active_connected_owner_rejects_teardown_without_losing_transaction() {
        let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
        let mut hardware = Hardware {
            prepare: true,
            ..Hardware::default()
        };
        let mut tx = make_tx(slot.as_mut(), &mut hardware, 4);
        assert_eq!(
            tx.start(&mut hardware, &ethernet()),
            Ok(WifiTxProgress::Pending)
        );

        let mut tx = match tx.try_into_parts() {
            Err(tx) => tx,
            Ok(_) => panic!("hardware-owned TX must reject decomposition"),
        };
        assert!(tx.active());
        hardware.completion = Some(completion(0));
        assert_eq!(
            crate::test_support::block_on(tx.service(
                &mut hardware,
                WifiTxWake::Interrupt {
                    events: open_esp_radio_esp32s31_wifi_mac::irq::MAC_INT_TX_COMPLETE,
                },
            )),
            Ok(WifiTxProgress::Complete)
        );
        assert!(tx.try_into_parts().is_ok());
    }

    #[test]
    fn connected_action_uses_the_shared_slot_as_plaintext_voice_tx() {
        let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
        let mut hardware = Hardware {
            prepare: true,
            ..Hardware::default()
        };
        let mut tx = make_tx(slot.as_mut(), &mut hardware, 4);
        let body = [3, 2, 0, 0, 37, 0];

        assert_eq!(
            tx.start_action(&mut hardware, &body, ActionTxConfig::RX_ADDBA_RESPONSE,),
            Ok(WifiTxProgress::Pending)
        );
        let (queue, program) = hardware.legacy.expect("legacy queue image");
        assert_eq!(queue, 0);
        assert_eq!(program.interface, MacInterface::Station);
        assert_eq!(program.scheduler_priority, 0);
        assert_eq!(program.packet_priority, 0);
        assert_eq!(program.plcp1 & 0xfff, 34);

        hardware.completion = Some(completion(0));
        assert_eq!(
            crate::test_support::block_on(tx.service(
                &mut hardware,
                WifiTxWake::Interrupt {
                    events: open_esp_radio_esp32s31_wifi_mac::irq::MAC_INT_TX_COMPLETE,
                },
            )),
            Ok(WifiTxProgress::Complete)
        );
        assert_eq!(tx.ordinary.slot.state(), TxSlotState::Free);
        let bytes = tx.ordinary.slot.as_mut().buffer_mut().unwrap();
        assert_eq!(u32::from_le_bytes(bytes[..4].try_into().unwrap()), 34);
        assert_eq!(&bytes[TX_METADATA_SIZE..TX_METADATA_SIZE + 2], &[0xd0, 0]);
        assert_eq!(
            &bytes[TX_METADATA_SIZE + 10..TX_METADATA_SIZE + 16],
            &[2, 3, 4, 5, 6, 7]
        );
        assert_eq!(&bytes[TX_METADATA_SIZE + 24..TX_METADATA_SIZE + 30], &body);
    }

    #[test]
    fn power_save_null_uses_shared_retried_tx_and_exact_pm_bit() {
        let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
        let mut hardware = Hardware {
            prepare: true,
            ..Hardware::default()
        };
        let mut tx = make_tx(slot.as_mut(), &mut hardware, 4);

        assert_eq!(
            tx.start_power_management_null(&mut hardware, StaPowerManagement::PowerSave),
            Ok(WifiTxProgress::Pending)
        );
        let (queue, program) = hardware.legacy.expect("legacy queue image");
        assert_eq!(queue, 0);
        assert_eq!(program.scheduler_priority, 1);
        assert_eq!(program.packet_priority, 1);
        // PLCP length includes the four-byte hardware FCS.
        assert_eq!(program.plcp1 & 0xfff, 28);

        hardware.completion = Some(completion(0));
        assert_eq!(
            crate::test_support::block_on(tx.service(
                &mut hardware,
                WifiTxWake::Interrupt {
                    events: open_esp_radio_esp32s31_wifi_mac::irq::MAC_INT_TX_COMPLETE,
                },
            )),
            Ok(WifiTxProgress::Complete)
        );
        assert!(matches!(
            tx.take_last_outcome(),
            Some(SingleMpduTxOutcome::Success(_))
        ));
        let bytes = tx.ordinary.slot.as_mut().buffer_mut().unwrap();
        assert_eq!(
            u16::from_le_bytes(
                bytes[TX_METADATA_SIZE..TX_METADATA_SIZE + 2]
                    .try_into()
                    .unwrap()
            ),
            0x1148
        );
        assert_eq!(&bytes[TX_METADATA_SIZE + 4..TX_METADATA_SIZE + 10], &BSSID);
        assert_eq!(
            &bytes[TX_METADATA_SIZE + 10..TX_METADATA_SIZE + 16],
            &[2, 3, 4, 5, 6, 7]
        );
    }

    #[test]
    fn ack_timeout_republishes_the_same_encoded_mpdu_with_retry_bit() {
        let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
        let mut hardware = Hardware {
            prepare: true,
            ..Hardware::default()
        };
        let mut tx = make_tx(slot.as_mut(), &mut hardware, 2);
        tx.start(&mut hardware, &ethernet()).unwrap();
        hardware.completion = Some(completion(5));

        assert_eq!(
            crate::test_support::block_on(tx.service(
                &mut hardware,
                WifiTxWake::Interrupt {
                    events: open_esp_radio_esp32s31_wifi_mac::irq::MAC_INT_TX_COMPLETE,
                },
            )),
            Ok(WifiTxProgress::Pending)
        );
        assert_eq!(hardware.publications, 2);
        let active = tx
            .ordinary
            .active_snapshot()
            .expect("normal retry rate remains available")
            .expect("ACK timeout retains an active publication");
        assert_eq!(active.counters.mpdu, 1);
        assert_eq!(active.counters.short, 1);
        assert_eq!(active.counters.long, 0);
        assert_eq!(active.publications, 2);
        assert_eq!(active.current_rate, TxPhyRate::Legacy(LegacyRate::Ofdm54M));
        assert!(active.retry_bit_set);
        assert_eq!(active.retries.ack_timeouts, 1);
        hardware.completion = Some(completion(0));
        assert_eq!(
            crate::test_support::block_on(tx.service(
                &mut hardware,
                WifiTxWake::Interrupt {
                    events: open_esp_radio_esp32s31_wifi_mac::irq::MAC_INT_TX_COMPLETE,
                },
            )),
            Ok(WifiTxProgress::Complete)
        );
        assert_ne!(
            tx.ordinary.slot.as_mut().buffer_mut().unwrap()[TX_METADATA_SIZE + 1] & 0x08,
            0
        );
        let report = tx
            .take_last_outcome()
            .expect("successful retried exchange")
            .report();
        assert_eq!(report.status.result, MacTxResult::Transmitted);
        assert_eq!(report.status.attempts, 2);
        assert_eq!(
            report.status.final_rate,
            TxPhyRate::Legacy(LegacyRate::Ofdm54M)
        );
        assert_eq!(report.status.acknowledged, Some(true));
    }

    #[test]
    fn timeout_waits_sixteen_micros_and_terminates_without_republication() {
        let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
        let mut hardware = Hardware {
            prepare: true,
            ..Hardware::default()
        };
        let mut tx = make_tx(slot.as_mut(), &mut hardware, 2);
        tx.start(&mut hardware, &ethernet()).unwrap();
        hardware.timeout = true;
        let timeout = WifiTxWake::Interrupt {
            events: open_esp_radio_esp32s31_wifi_mac::irq::MAC_INT_TX_TIMEOUT,
        };

        assert_eq!(
            crate::test_support::block_on(tx.service(&mut hardware, timeout)),
            Ok(WifiTxProgress::Complete)
        );
        assert_eq!(tx.ordinary.timer.settled, 16);
        assert_eq!(hardware.publications, 1);
        assert_eq!(
            tx.take_last_outcome()
                .expect("terminal timeout report")
                .report()
                .status
                .result,
            MacTxResult::HardwareTimeout
        );
    }

    #[test]
    fn collision_retries_without_marking_an_untransmitted_mpdu_as_retry() {
        let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
        let mut hardware = Hardware {
            prepare: true,
            ..Hardware::default()
        };
        let mut tx = make_tx(slot.as_mut(), &mut hardware, 2);
        tx.start(&mut hardware, &ethernet()).unwrap();
        hardware.collision = true;
        let collision = WifiTxWake::Interrupt {
            events: open_esp_radio_esp32s31_wifi_mac::irq::MAC_INT_COLLISION,
        };

        for collision_number in 1..=VENDOR_SHORT_RETRY_LIMIT {
            hardware.collision = true;
            let expected = if collision_number < VENDOR_SHORT_RETRY_LIMIT {
                WifiTxProgress::Pending
            } else {
                WifiTxProgress::Complete
            };
            assert_eq!(
                crate::test_support::block_on(tx.service(&mut hardware, collision)),
                Ok(expected)
            );
        }
        assert_eq!(
            tx.ordinary.slot.as_mut().buffer_mut().unwrap()[TX_METADATA_SIZE + 1] & 0x08,
            0
        );
        let report = tx
            .take_last_outcome()
            .expect("terminal collision report")
            .report();
        assert_eq!(report.status.result, MacTxResult::CollisionLimit);
        assert_eq!(report.status.attempts, VENDOR_SHORT_RETRY_LIMIT);
        assert_eq!(report.retries.collisions, VENDOR_SHORT_RETRY_LIMIT - 1);
    }

    #[test]
    fn executor_deadline_quarantines_without_drop_panic() {
        let mut slot = std::boxed::Box::pin(TxSlot::<512>::new_model());
        let mut hardware = Hardware {
            prepare: true,
            ..Hardware::default()
        };
        {
            let mut tx = make_tx(slot.as_mut(), &mut hardware, 2);
            tx.start(&mut hardware, &ethernet()).unwrap();

            assert_eq!(
                crate::test_support::block_on(tx.service(&mut hardware, WifiTxWake::Deadline)),
                Err(SingleMpduTxError::RadioResetRequired(
                    TxResetReason::ExecutorDeadline
                ))
            );
            assert_eq!(tx.ordinary.slot.state(), TxSlotState::ResetRequired);
            assert_eq!(tx.queue_state(), MacTxQueueState::ResetRequired);
            assert!(tx.ordinary.slot.as_mut().reserve(64, 32).is_err());
        }
        let drop_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(slot)));
        assert!(drop_result.is_ok());
    }

    #[test]
    fn queue_rejection_cancels_the_unpublished_descriptor() {
        let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
        let mut hardware = Hardware::default();
        let mut tx = make_tx(slot.as_mut(), &mut hardware, 2);

        assert_eq!(
            tx.start(&mut hardware, &ethernet()),
            Err(SingleMpduTxError::Tx(TxError::QueueActive))
        );
        assert_eq!(tx.ordinary.slot.state(), TxSlotState::Free);
        assert_eq!(tx.ordinary.slot.descriptor_word0(), 0);
    }
}
