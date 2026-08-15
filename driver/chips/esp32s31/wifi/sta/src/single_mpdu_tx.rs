//! Production owner for one connected-station legacy/HT MPDU transaction.
//!
//! This module owns the executor-independent transaction between an encoded
//! station frame and the ESP32-S31 ordinary TX queue. It deliberately owns no logging,
//! benchmark statistics, NVS state or RTOS adapter. Every PAC borrow ends
//! before a timer is awaited.

use core::future::Future;

use open_esp_radio_esp32s31_wifi_mac::{
    crypto::StaPairwiseCcmpSlot,
    tx::{LegacyTxQueue, TxError, TxHardware, TxPhyRate},
    tx_runtime::{OrdinaryRetryError, WifiTxRuntimePolicy},
};
use open_esp_radio_ieee80211::management::ProbeRequest;
use open_esp_radio_ieee80211::station::{
    StaActionFrame, StaProtectedDataFrame, StaProtectedEthernetFrame, StaTxSequenceCounters,
    StationFrameError,
};
use open_esp_radio_ieee80211::station_power_save::{StaNullDataFrame, StaPowerManagement};
use open_esp_radio_wifi_softmac::{MacTxPlan, MacTxQueueState};

use open_esp_radio_esp32s31_wifi::ordinary_tx::{
    OrdinaryTxError, OrdinaryTxOwner, OrdinaryTxPlan, TX_CCMP_MIC_SIZE, TX_METADATA_SIZE,
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

/// Protocol resources installed at the WPA2-to-connected TX handoff.
///
/// Keeping the key token, all independent sequence spaces and the negotiated
/// publication policy in one value prevents a partial transition from
/// constructing a connected transmitter with mismatched session state.
pub struct ConnectedTxHandoff {
    pub key: StaPairwiseCcmpSlot,
    pub sequences: StaTxSequenceCounters,
    pub config: SingleMpduTxConfig,
}

/// Queue priorities for one unprotected connected Action frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ActionTxConfig {
    pub scheduler_priority: u8,
    pub packet_priority: u8,
}

impl ActionTxConfig {
    /// Profile used by ordinary management frames recovered from the vendor
    /// queue-zero path.
    pub const VENDOR_MANAGEMENT: Self = Self {
        scheduler_priority: 1,
        packet_priority: 1,
    };

    /// Profile retained by the recovered connected RX ADDBA response path.
    pub const RX_ADDBA_RESPONSE: Self = Self {
        scheduler_priority: 0,
        packet_priority: 0,
    };
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SingleMpduTxError {
    Busy,
    EthernetFrameTooShort,
    UnsupportedHeOrdinaryMpdu,
    BufferSizeOverflow,
    DeadlineOverflow,
    ProbeEncode,
    Encode(StationFrameError),
    Tx(TxError),
    Retry(OrdinaryRetryError),
    RadioResetRequired(TxResetReason),
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

impl From<OrdinaryTxError> for SingleMpduTxError {
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

/// Unique ordinary-MPDU descriptor, crypto PN and retry owner.
pub struct Esp32s31SingleMpduTx<'slot, P, E, T, const BUFFER_SIZE: usize> {
    ordinary: OrdinaryTxOwner<'slot, P, E, T, BUFFER_SIZE>,
    key: StaPairwiseCcmpSlot,
    sequences: StaTxSequenceCounters,
    config: SingleMpduTxConfig,
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
            key,
            sequences,
            config,
        } = handoff;
        Self {
            ordinary: OrdinaryTxOwner::new(resources),
            key,
            sequences,
            config,
        }
    }

    pub(crate) fn from_ordinary(
        ordinary: OrdinaryTxOwner<'slot, P, E, T, BUFFER_SIZE>,
        key: StaPairwiseCcmpSlot,
        sequences: StaTxSequenceCounters,
        config: SingleMpduTxConfig,
    ) -> Self {
        Self {
            ordinary,
            key,
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
            key,
            sequences,
            config,
        } = self;
        match ordinary.try_into_resources() {
            Ok(resources) => Ok((
                resources,
                ConnectedTxHandoff {
                    key,
                    sequences,
                    config,
                },
            )),
            Err(ordinary) => Err(Self {
                ordinary,
                key,
                sequences,
                config,
            }),
        }
    }

    pub fn peek_qos_sequence(&self, tid: u8) -> Option<u16> {
        self.sequences.peek_qos(tid)
    }

    pub fn take_protected_metadata(&mut self, tid: u8) -> Option<StaProtectedEthernetFrame> {
        Some(StaProtectedEthernetFrame {
            bssid: self.config.bssid,
            sequence_number: self.sequences.take_data(Some(tid))?,
            user_priority: tid,
            peer_qos: self.config.peer_qos,
            ccmp_header: self.key.next_tx_ccmp_header(),
        })
    }

    pub const fn hardware_key_selector(&self) -> u8 {
        self.key.hardware_index()
    }

    pub const fn config(&self) -> SingleMpduTxConfig {
        self.config
    }

    /// Publish one exact protected MPDU copied from a detached aggregate.
    /// Sequence Control and CCMP PN are already present and must not be
    /// allocated again; only the IEEE Retry bit is added.
    pub fn copy_encoded_retry(&mut self, encoded: &[u8]) -> Result<usize, SingleMpduTxError> {
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
        if matches!(rate, TxPhyRate::He(_)) {
            return Err(SingleMpduTxError::UnsupportedHeOrdinaryMpdu);
        }
        self.ordinary
            .start(
                hardware,
                OrdinaryTxPlan {
                    frame_length,
                    descriptor_capacity: None,
                    exchange: MacTxPlan {
                        access_category: LegacyTxQueue::BestEffort.access_category(),
                        initial_rate: rate,
                        publication_limit: self.config.exchange.publication_limit,
                        publication_timeout_micros: self.config.exchange.publication_timeout_micros,
                    },
                    hardware_mic_length,
                    hardware_key_selector: self.key.hardware_index(),
                    interface:
                        open_esp_radio_esp32s31_wifi::ordinary_tx::OrdinaryTxInterface::Station,
                    scheduler_priority: LegacyTxQueue::BestEffort.vendor_data_scheduler_priority(),
                    packet_priority: LegacyTxQueue::BestEffort.vendor_data_packet_priority(),
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
        if self.ordinary.active() {
            return Err(SingleMpduTxError::Busy);
        }
        if matches!(self.config.exchange.initial_rate, TxPhyRate::He(_)) {
            return Err(SingleMpduTxError::UnsupportedHeOrdinaryMpdu);
        }
        if ethernet.len() < 14 {
            return Err(SingleMpduTxError::EthernetFrameTooShort);
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
            .take_data(self.config.peer_qos.then_some(0))
            .expect("TID zero is a valid sequence space");
        let ccmp_header = self.key.next_tx_ccmp_header();
        let frame_length = {
            let buffer = self.ordinary.buffer_mut()?;
            StaProtectedDataFrame {
                source,
                bssid: self.config.bssid,
                destination,
                sequence_number,
                user_priority: 0,
                peer_qos: self.config.peer_qos,
                ccmp_header,
                ether_type,
                payload: &ethernet[14..],
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
                    exchange: self.config.exchange,
                    hardware_mic_length: TX_CCMP_MIC_SIZE,
                    hardware_key_selector: self.key.hardware_index(),
                    interface:
                        open_esp_radio_esp32s31_wifi::ordinary_tx::OrdinaryTxInterface::Station,
                    scheduler_priority: LegacyTxQueue::BestEffort.vendor_data_scheduler_priority(),
                    packet_priority: LegacyTxQueue::BestEffort.vendor_data_packet_priority(),
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
        let sequence_number = self
            .sequences
            .take_data(self.config.peer_qos.then_some(0))
            .expect("selected EAPOL sequence-number owner exists");
        let ccmp_header = self.key.next_tx_ccmp_header();
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
                    hardware_key_selector: self.key.hardware_index(),
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
    use open_esp_radio_wifi_softmac::MacTxResult;

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
        fn install_sta_ccmp_entry(&mut self, _index: u8, _words: [u32; 6]) -> MacKeyInstallOutcome {
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
                key,
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
    fn idle_connected_owner_returns_descriptor_key_and_sequences_for_teardown() {
        let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
        let mut hardware = Hardware::default();
        let tx = make_tx(slot.as_mut(), &mut hardware, 4);
        let key_index = tx.key.hardware_index();

        let (resources, handoff) = match tx.try_into_parts() {
            Ok(parts) => parts,
            Err(_) => panic!("idle connected TX must decompose"),
        };

        assert_eq!(resources.slot.state(), TxSlotState::Free);
        assert_eq!(handoff.key.hardware_index(), key_index);
        assert_eq!(handoff.sequences.peek_non_qos(), 7);
        handoff.key.clear(&mut hardware);
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
