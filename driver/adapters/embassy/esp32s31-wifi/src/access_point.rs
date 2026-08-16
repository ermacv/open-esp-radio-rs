//! Embassy-owned AP MAC and network handoff service.
//!
//! The service handles beacons, management frames, WPA2 EAPOL and authorized
//! Ethernet traffic through one bounded RX/TX owner.

use core::{convert::Infallible, future::Future};

use embassy_futures::yield_now;
use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_time::{Instant, Timer};

use open_esp_radio_dma::StableDmaBacking;
use open_esp_radio_embassy_net::{
    FrameLengthError, LinkState, PinnedTxConsumer, PinnedTxFrame, RxEnqueueError,
    SplitPinnedRadioRunner,
};

use open_esp_radio_esp32s31_wifi::{
    ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile, WifiTxResources, WifiTxTimer},
    tx::{WifiTxProgress, WifiTxWake},
};
use open_esp_radio_esp32s31_wifi_ap::protocol::{
    AccessPointServiceStatus, ApPeerClose, ApWpa2RetryProgress,
};
use open_esp_radio_esp32s31_wifi_ap::{
    ampdu::{Esp32s31ApAmpduError, Esp32s31ApAmpduProgress},
    engine::{Esp32s31ApRuntimeHardware, Esp32s31ApWpa2Outcome},
    mac::{
        Esp32s31ApMac, Esp32s31ApMacError, Esp32s31ApMacReport, Esp32s31ApPeerDisconnectStage,
        Esp32s31ApTxCompletionAction,
    },
    rx::{
        Esp32s31ApRxConfig, Esp32s31ApRxDispatch, Esp32s31ApRxDispatcher, Esp32s31ApRxError,
        Esp32s31ApRxEvent, Esp32s31ApRxSink,
    },
};
use open_esp_radio_esp32s31_wifi_mac::{
    init::MAC_COLD_RX_INTERRUPT_MASK,
    irq::{MAC_INT_COLLISION, MAC_INT_TX_COMPLETE, MAC_INT_TX_TIMEOUT, MacInterruptRoute},
    rx::{RxDescriptorSnapshot, RxDma, RxIngressConfig, RxRingHalted, view_normalized_rx_frame},
    tx::TxHardware,
};
use open_esp_radio_ieee80211::data::EthernetFrameParts;
use open_esp_radio_ieee80211::data::{
    DataInterfaceRole, IEEE80211_LEGACY_DATA_HEADER_LEN, IEEE80211_QOS_DATA_HEADER_LEN,
    plan_data_decapsulation,
};
use open_esp_radio_wifi_embassy::await_stack_boundary;
use open_esp_radio_wpa2::{OwnedEapolFrame, Wpa2Interface};

#[cfg(feature = "rx-delivery-observation")]
use crate::network_rx::{RxNetworkDeliveryEvent, RxNetworkDeliveryObserver};
use crate::{
    embassy_irq::{
        Esp32s31MacInterruptEpoch, Esp32s31MacInterruptEpochActivateError,
        Esp32s31MacInterruptEpochDrain, Esp32s31MacInterruptEpochQuiesceError,
    },
    rx_dma_service::Esp32s31RxDmaStorage,
    rx_frontier::{
        Esp32s31RecycledRxDirective, Esp32s31RxFrontier, Esp32s31RxFrontierContinuation,
        Esp32s31RxFrontierDelay, Esp32s31RxFrontierError, Esp32s31RxFrontierSchedulerSnapshot,
    },
    wdev::{
        WdevControlContext, WdevControlProgress, WdevNetworkRx, WdevRunner, WdevRxProgress,
        WdevServices, WdevStopProgress,
    },
};

const EAPOL_ETHERTYPE: u16 = 0x888e;
const EAPOL_CAPACITY: usize = 512;

mod ampdu;
mod network_tx;
mod wdev;

pub use ampdu::Esp32s31AccessPointAmpdu;
use network_tx::Esp32s31AccessPointNetworkTx;
use wdev::Esp32s31AccessPointWdevServices;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Esp32s31AccessPointControlReport {
    pub missed_beacon_intervals: u32,
    pub maximum_beacon_lateness_micros: u32,
    pub tx_interrupt_wakes: u32,
    pub tx_deadline_wakes: u32,
    pub maximum_tx_pending_micros: u32,
    /// Longest network-originated data transaction, excluding chained AP
    /// management, WPA2 and shutdown publications.
    pub maximum_network_tx_pending_micros: u32,
    /// Hardware publications made by the network frame which established
    /// `maximum_network_tx_pending_micros`.
    pub network_tx_attempts_at_maximum_pending: u8,
    pub maximum_rx_service_micros: u32,
    pub maximum_network_backpressure_micros: u32,
    pub completed_rx_units: u32,
    pub completed_rx_descriptors: u32,
    pub recycled_rx_descriptors: u32,
    pub retained_rx_descriptors: u32,
    pub discarded_rx_units: u32,
    pub ignored_rx_frames: u32,
    pub rx_mic_failures: u32,
    pub rx_quarantined_frames: u32,
    pub rx_view_rejected: u32,
    pub control_frames_staged: u32,
    pub control_frames_dropped_while_busy: u32,
    pub ethernet_frames_staged: u32,
    pub ethernet_arp_requests_staged: u32,
    pub ethernet_tcp_frames_staged: u32,
    pub network_tx_frames_observed: u32,
    pub network_tx_arp_requests: u32,
    pub network_tx_arp_replies: u32,
    pub network_tx_rejected_no_peer: u32,
    pub network_tx_rejected_destination: u32,
    pub network_tx_frames_rejected: u32,
    pub protected_data_frames: u32,
    pub protected_data_unauthorized: u32,
    pub protected_data_foreign: u32,
    pub protected_data_duplicates: u32,
    pub protected_data_radio_rejected: u32,
    pub protected_data_protocol_rejected: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31AccessPointControlError {
    Receive(Esp32s31RxFrontierError),
    Mac(Esp32s31ApMacError),
    /// The caller-provided RX scratch cannot retain one fully decoded batch.
    ReceiveBatchCapacity,
    /// A live BlockAck agreement lost its corresponding peer HT facts.
    InvalidPeerHtRate,
    InvalidBeaconSchedule,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Esp32s31AccessPointWdevError {
    Control(Esp32s31AccessPointControlError),
    Network(FrameLengthError),
    Aggregate(Esp32s31ApAmpduError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31AccessPointRunError<E> {
    Control(Esp32s31AccessPointControlError),
    InterruptActivate(Esp32s31MacInterruptEpochActivateError<E>),
    InterruptQuiesce(Esp32s31MacInterruptEpochQuiesceError<E>),
    Network(FrameLengthError),
    Aggregate(Esp32s31ApAmpduError),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Esp32s31AccessPointRunReport {
    pub control: Esp32s31AccessPointControlReport,
    pub mac: Esp32s31ApMacReport,
    pub engine: open_esp_radio_esp32s31_wifi_ap::engine::Esp32s31ApEngineReport,
    pub interrupt_drain: Esp32s31MacInterruptEpochDrain,
    pub rx_scheduler: Option<Esp32s31RxFrontierSchedulerSnapshot>,
}

/// Complete reusable frontier after IRQ, RX, TX, keys and AP TSF stop.
pub struct Esp32s31AccessPointStopped<
    'storage,
    'beacon,
    'slot,
    P,
    E,
    T,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
    const TX_BUFFER_SIZE: usize,
> {
    pub ring: RxRingHalted<'storage, COUNT>,
    pub storage: &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    pub transmit: WifiTxResources<'slot, P, E, T, TX_BUFFER_SIZE>,
    pub rx_frame: &'storage mut [u8],
    pub tx_frame: &'storage mut [u8],
    pub data_rx: &'storage mut Esp32s31ApRxDispatcher,
    pub engine: open_esp_radio_esp32s31_wifi_ap::engine::Esp32s31ApEngineStop<'beacon>,
    pub control_report: Esp32s31AccessPointControlReport,
    pub mac_report: Esp32s31ApMacReport,
}

impl From<Esp32s31RxFrontierError> for Esp32s31AccessPointControlError {
    fn from(error: Esp32s31RxFrontierError) -> Self {
        Self::Receive(error)
    }
}

impl From<Esp32s31ApMacError> for Esp32s31AccessPointControlError {
    fn from(error: Esp32s31ApMacError) -> Self {
        Self::Mac(error)
    }
}

impl From<open_esp_radio_esp32s31_wifi_ap::engine::Esp32s31ApEngineError>
    for Esp32s31AccessPointControlError
{
    fn from(error: open_esp_radio_esp32s31_wifi_ap::engine::Esp32s31ApEngineError) -> Self {
        Self::Mac(Esp32s31ApMacError::Engine(error))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StagedControlFrame {
    Management { length: usize },
    Eapol { length: usize },
    EthernetBatch { used: usize },
}

struct DeferredAccessPointRxSink<'storage> {
    frames: crate::ethernet_rx::PackedEthernetWriter<'storage>,
    exhausted: bool,
}

impl<'storage> DeferredAccessPointRxSink<'storage> {
    fn new(storage: &'storage mut [u8]) -> Self {
        Self {
            frames: crate::ethernet_rx::PackedEthernetWriter::new(storage),
            exhausted: false,
        }
    }

    const fn used(&self) -> usize {
        self.frames.used()
    }
}

impl Esp32s31ApRxSink for DeferredAccessPointRxSink<'_> {
    fn publish(&mut self, event: Esp32s31ApRxEvent<'_>) {
        if self.frames.push(event.frame).is_err() {
            self.exhausted = true;
        }
    }
}

/// Control-plane owner for one active AP role.
pub struct Esp32s31AccessPointControl<
    'storage,
    'beacon,
    'slot,
    D,
    P,
    E,
    T,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
    const TX_BUFFER_SIZE: usize,
> {
    receive: Esp32s31RxFrontier<'storage, D, COUNT, DMA_BUFFER_SIZE>,
    storage: &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    mac: Esp32s31ApMac<'beacon, 'slot, P, E, T, TX_BUFFER_SIZE>,
    rx_frame: &'storage mut [u8],
    tx_frame: &'storage mut [u8],
    data_rx: &'storage mut Esp32s31ApRxDispatcher,
    rx_batch_used: usize,
    rx_batch_offset: usize,
    report: Esp32s31AccessPointControlReport,
}

impl<
    'storage,
    'beacon,
    'slot,
    D,
    P,
    E,
    T,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
    const TX_BUFFER_SIZE: usize,
>
    Esp32s31AccessPointControl<
        'storage,
        'beacon,
        'slot,
        D,
        P,
        E,
        T,
        COUNT,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
        TX_BUFFER_SIZE,
    >
where
    D: Esp32s31RxFrontierDelay,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
{
    pub fn new(
        receive: Esp32s31RxFrontier<'storage, D, COUNT, DMA_BUFFER_SIZE>,
        storage: &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        mac: Esp32s31ApMac<'beacon, 'slot, P, E, T, TX_BUFFER_SIZE>,
        rx_frame: &'storage mut [u8],
        tx_frame: &'storage mut [u8],
        data_rx: &'storage mut Esp32s31ApRxDispatcher,
    ) -> Self {
        let access_point = mac.engine().service_address();
        data_rx.reset(Esp32s31ApRxConfig {
            access_point,
            ingress: RxIngressConfig {
                ring_entry_limit: 1,
                csi_config: 0,
                flags: 0,
            },
        });
        Self {
            receive,
            storage,
            mac,
            rx_frame,
            tx_frame,
            data_rx,
            rx_batch_used: 0,
            rx_batch_offset: 0,
            report: Esp32s31AccessPointControlReport::default(),
        }
    }

    pub async fn start<H: RxDma>(
        &mut self,
        hardware: &mut H,
    ) -> Result<(), Esp32s31AccessPointControlError> {
        self.receive
            .start_with_storage(hardware, self.storage)
            .await?;
        Ok(())
    }

    pub fn stop<H: RxDma>(
        &mut self,
        hardware: &mut H,
    ) -> Result<(), Esp32s31AccessPointControlError> {
        self.receive.stop(hardware)?;
        Ok(())
    }

    /// Observe one RX descriptor without exposing its DMA ownership.
    pub fn rx_descriptor_snapshot(&self, index: usize) -> Option<RxDescriptorSnapshot> {
        self.receive.descriptor_snapshot(index)
    }

    /// Observe the live RX scheduler frontier without exposing ownership.
    pub fn rx_scheduler_snapshot(&self) -> Option<Esp32s31RxFrontierSchedulerSnapshot> {
        self.receive.scheduler_snapshot()
    }

    /// Recycle one complete RX unit, stage at most one control MPDU, and
    /// publish every protected Ethernet MSDU through the supplied bounded
    /// sink before the borrowed RX view expires.
    ///
    /// The RX walker is always recycled before TX receives the PAC borrow.
    /// This avoids nested mutable access to one register owner and makes the
    /// ISR→RX→TX ordering explicit.
    pub async fn service_rx<H>(
        &mut self,
        hardware: &mut H,
        authenticator_nonce: [u8; 32],
        initial_replay_counter: u64,
        now_micros: u64,
    ) -> Result<(), Esp32s31AccessPointControlError>
    where
        H: RxDma + TxHardware + Esp32s31ApRuntimeHardware,
    {
        if self.mac.tx_pending() || self.rx_batch_pending() {
            return Ok(());
        }
        let mut staged = None;
        let mut activity_peer = None;
        let mut batch_exhausted = false;
        let rx_frame = &mut *self.rx_frame;
        let unit_staging = &mut *self.tx_frame;
        let mac = &self.mac;
        let data_rx = &mut self.data_rx;
        let report = &mut self.report;
        let progress = self
            .receive
            .service_completed_unit(hardware, self.storage, unit_staging, |segment| {
                if staged.is_some() {
                    report.control_frames_dropped_while_busy =
                        report.control_frames_dropped_while_busy.saturating_add(1);
                    return Esp32s31RecycledRxDirective::Pause;
                }
                let frame = match view_normalized_rx_frame(
                    &segment,
                    RxIngressConfig {
                        ring_entry_limit: 1,
                        csi_config: 0,
                        flags: 0,
                    },
                ) {
                    Ok(frame) => frame,
                    Err(error) => {
                        match error {
                            open_esp_radio_esp32s31_wifi_mac::rx::RxError::MicFailure => {
                                report.rx_mic_failures = report.rx_mic_failures.saturating_add(1);
                            }
                            open_esp_radio_esp32s31_wifi_mac::rx::RxError::Quarantined => {
                                report.rx_quarantined_frames =
                                    report.rx_quarantined_frames.saturating_add(1);
                            }
                            _ => {
                                report.rx_view_rejected = report.rx_view_rejected.saturating_add(1);
                            }
                        }
                        report.ignored_rx_frames = report.ignored_rx_frames.saturating_add(1);
                        return Esp32s31RecycledRxDirective::Continue;
                    }
                };
                let frame_control = u16::from_le_bytes([frame.mpdu[0], frame.mpdu[1]]);
                if frame_control & 0x000c == 0x0008 && frame_control & 0x4000 != 0 {
                    let source = frame
                        .mpdu
                        .get(10..16)
                        .and_then(|bytes| <[u8; 6]>::try_from(bytes).ok());
                    report.protected_data_frames = report.protected_data_frames.saturating_add(1);
                    let mut sink = DeferredAccessPointRxSink::new(rx_frame);
                    let dispatch = data_rx.dispatch_protected(
                        segment,
                        |peer| mac.engine().is_authorized_peer(peer),
                        &mut sink,
                    );
                    let batch_used = sink.used();
                    batch_exhausted = sink.exhausted;
                    match dispatch {
                        Esp32s31ApRxDispatch::Data {
                            ethernet_frames, ..
                        } => {
                            activity_peer = source;
                            if ethernet_frames != 0 {
                                staged =
                                    Some(StagedControlFrame::EthernetBatch { used: batch_used });
                            }
                        }
                        Esp32s31ApRxDispatch::Duplicate => {
                            report.protected_data_duplicates =
                                report.protected_data_duplicates.saturating_add(1);
                        }
                        Esp32s31ApRxDispatch::ForeignPeer => {
                            report.protected_data_foreign =
                                report.protected_data_foreign.saturating_add(1);
                        }
                        Esp32s31ApRxDispatch::Unauthorized => {
                            report.protected_data_unauthorized =
                                report.protected_data_unauthorized.saturating_add(1);
                        }
                        Esp32s31ApRxDispatch::Rejected(Esp32s31ApRxError::Radio(_)) => {
                            report.protected_data_radio_rejected =
                                report.protected_data_radio_rejected.saturating_add(1);
                        }
                        Esp32s31ApRxDispatch::Rejected(Esp32s31ApRxError::Data(_)) => {
                            report.protected_data_protocol_rejected =
                                report.protected_data_protocol_rejected.saturating_add(1);
                        }
                    }
                    if matches!(staged, Some(StagedControlFrame::EthernetBatch { .. })) {
                        return Esp32s31RecycledRxDirective::Pause;
                    }
                    report.ignored_rx_frames = report.ignored_rx_frames.saturating_add(1);
                    return Esp32s31RecycledRxDirective::Continue;
                }
                if frame.mpdu.len() > rx_frame.len() {
                    report.ignored_rx_frames = report.ignored_rx_frames.saturating_add(1);
                    return Esp32s31RecycledRxDirective::Continue;
                }
                rx_frame[..frame.mpdu.len()].copy_from_slice(frame.mpdu);
                staged = if frame_control & 0x000c == 0 {
                    Some(StagedControlFrame::Management {
                        length: frame.mpdu.len(),
                    })
                } else if frame_control & 0x000c == 0x0008 {
                    Some(StagedControlFrame::Eapol {
                        length: frame.mpdu.len(),
                    })
                } else {
                    report.ignored_rx_frames = report.ignored_rx_frames.saturating_add(1);
                    None
                };
                if staged.is_some() {
                    Esp32s31RecycledRxDirective::Pause
                } else {
                    Esp32s31RecycledRxDirective::Continue
                }
            })
            .await?;
        if batch_exhausted {
            self.report.protected_data_protocol_rejected = self
                .report
                .protected_data_protocol_rejected
                .saturating_add(1);
            return Err(Esp32s31AccessPointControlError::ReceiveBatchCapacity);
        }
        if let Some(peer) = activity_peer {
            self.mac
                .engine_mut()
                .observe_peer_activity(peer, now_micros)?;
        }
        self.report.completed_rx_units = self
            .report
            .completed_rx_units
            .saturating_add(progress.completed_units);
        self.report.completed_rx_descriptors = self
            .report
            .completed_rx_descriptors
            .saturating_add(progress.completed_descriptors);
        self.report.recycled_rx_descriptors = self
            .report
            .recycled_rx_descriptors
            .saturating_add(progress.recycled_descriptors);
        self.report.discarded_rx_units = self
            .report
            .discarded_rx_units
            .saturating_add(progress.discarded_units);

        let Some(staged) = staged else {
            return Ok(());
        };
        match staged {
            StagedControlFrame::Management { length } => {
                let outcome = self.mac.publish_management(
                    hardware,
                    &self.rx_frame[..length],
                    authenticator_nonce,
                    initial_replay_counter,
                    now_micros,
                    self.tx_frame,
                )?;
                if matches!(
                    outcome,
                    open_esp_radio_esp32s31_wifi_ap::engine::Esp32s31ApManagementOutcome::Response {
                        ..
                    }
                ) {
                    self.report.control_frames_staged =
                        self.report.control_frames_staged.saturating_add(1);
                }
            }
            StagedControlFrame::Eapol { length } => {
                self.service_eapol(hardware, length, now_micros)?;
            }
            StagedControlFrame::EthernetBatch { used } => {
                self.rx_batch_used = used;
                self.rx_batch_offset = 0;
            }
        }
        Ok(())
    }

    pub const fn rx_batch_pending(&self) -> bool {
        self.rx_batch_offset < self.rx_batch_used
    }

    fn rx_batch_record(
        &self,
    ) -> Result<Option<crate::ethernet_rx::PackedEthernetRecord<'_>>, Esp32s31AccessPointControlError>
    {
        crate::ethernet_rx::record_at(self.rx_frame, self.rx_batch_used, self.rx_batch_offset)
            .map_err(|_| Esp32s31AccessPointControlError::ReceiveBatchCapacity)
    }

    fn commit_rx_batch_record(&mut self, next_offset: usize) {
        debug_assert!(next_offset > self.rx_batch_offset);
        debug_assert!(next_offset <= self.rx_batch_used);
        self.rx_batch_offset = next_offset;
        if self.rx_batch_offset == self.rx_batch_used {
            self.rx_batch_offset = 0;
            self.rx_batch_used = 0;
        }
    }

    fn service_eapol<H>(
        &mut self,
        hardware: &mut H,
        length: usize,
        now_micros: u64,
    ) -> Result<(), Esp32s31AccessPointControlError>
    where
        H: TxHardware + Esp32s31ApRuntimeHardware,
    {
        let mpdu = &self.rx_frame[..length];
        let header_length = if mpdu[0] & 0x80 != 0 {
            IEEE80211_QOS_DATA_HEADER_LEN
        } else {
            IEEE80211_LEGACY_DATA_HEADER_LEN
        };
        let Some(payload_length) = mpdu.len().checked_sub(header_length) else {
            self.report.ignored_rx_frames = self.report.ignored_rx_frames.saturating_add(1);
            return Ok(());
        };
        let Ok(plan) = plan_data_decapsulation(
            DataInterfaceRole::AccessPoint,
            mpdu,
            header_length,
            payload_length,
        ) else {
            self.report.ignored_rx_frames = self.report.ignored_rx_frames.saturating_add(1);
            return Ok(());
        };
        if plan.ether_type != EAPOL_ETHERTYPE
            || plan.destination != self.mac.engine().service_address()
            || self.mac.engine().peer_status(plan.source).is_none()
        {
            self.report.ignored_rx_frames = self.report.ignored_rx_frames.saturating_add(1);
            return Ok(());
        }
        let payload = &mpdu[plan.payload_offset..plan.payload_offset + plan.payload_length];
        let Ok(frame) = OwnedEapolFrame::<EAPOL_CAPACITY>::try_copy(
            Wpa2Interface::AccessPoint,
            plan.source,
            payload,
        ) else {
            self.report.ignored_rx_frames = self.report.ignored_rx_frames.saturating_add(1);
            return Ok(());
        };
        match self
            .mac
            .engine_mut()
            .handle_eapol(hardware, plan.source, frame, now_micros)?
        {
            Esp32s31ApWpa2Outcome::Transmit(frame) => {
                self.mac
                    .publish_eapol(hardware, plan.source, &frame, self.tx_frame)?;
                self.report.control_frames_staged =
                    self.report.control_frames_staged.saturating_add(1);
            }
            Esp32s31ApWpa2Outcome::DeauthenticatePeer { peer } => {
                let close = self
                    .mac
                    .engine_mut()
                    .begin_wpa2_failure_close(peer)
                    .map_err(Esp32s31ApMacError::Engine)?;
                self.publish_peer_close(hardware, close)?;
            }
            Esp32s31ApWpa2Outcome::PeerAuthorized { peer } => {
                if self.mac.publish_tx_block_ack_request(
                    hardware,
                    peer,
                    now_micros,
                    self.tx_frame,
                )? {
                    self.report.control_frames_staged =
                        self.report.control_frames_staged.saturating_add(1);
                }
            }
            Esp32s31ApWpa2Outcome::None => {}
        }
        Ok(())
    }

    pub async fn service_tx<H>(
        &mut self,
        hardware: &mut H,
        wake: WifiTxWake,
    ) -> Result<WifiTxProgress, Esp32s31AccessPointControlError>
    where
        H: Esp32s31ApRuntimeHardware + TxHardware,
    {
        let (progress, action) = self
            .mac
            .service_tx(hardware, wake, Instant::now().as_micros())
            .await?;
        match action {
            Esp32s31ApTxCompletionAction::BeginWpa2 { peer } => {
                let message1 = self.mac.engine().begin_wpa2::<EAPOL_CAPACITY>(peer)?;
                self.mac
                    .publish_eapol(hardware, peer, &message1, self.tx_frame)?;
                self.report.control_frames_staged =
                    self.report.control_frames_staged.saturating_add(1);
                return Ok(WifiTxProgress::Pending);
            }
            Esp32s31ApTxCompletionAction::PeerDisconnectTerminal {
                close,
                stage: Esp32s31ApPeerDisconnectStage::Disassociation,
                ..
            } => {
                self.mac.publish_peer_disconnect(
                    hardware,
                    close,
                    Esp32s31ApPeerDisconnectStage::Deauthentication,
                    self.tx_frame,
                )?;
                self.report.control_frames_staged =
                    self.report.control_frames_staged.saturating_add(1);
                return Ok(WifiTxProgress::Pending);
            }
            Esp32s31ApTxCompletionAction::PeerDisconnectTerminal {
                close,
                stage: Esp32s31ApPeerDisconnectStage::Deauthentication,
                ..
            } => self.mac.engine_mut().complete_peer_close(hardware, close)?,
            Esp32s31ApTxCompletionAction::None
            | Esp32s31ApTxCompletionAction::PublicationFailed => {}
        }
        Ok(progress)
    }

    fn publish_peer_close<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        close: ApPeerClose,
    ) -> Result<(), Esp32s31AccessPointControlError> {
        let stage = if close.was_associated {
            Esp32s31ApPeerDisconnectStage::Disassociation
        } else {
            Esp32s31ApPeerDisconnectStage::Deauthentication
        };
        self.mac
            .publish_peer_disconnect(hardware, close, stage, self.tx_frame)?;
        self.report.control_frames_staged = self.report.control_frames_staged.saturating_add(1);
        Ok(())
    }

    pub const fn report(&self) -> Esp32s31AccessPointControlReport {
        self.report
    }

    pub const fn mac_report(&self) -> Esp32s31ApMacReport {
        self.mac.report()
    }

    /// Whether the AP owns at least one operational downlink Block Ack
    /// agreement and can therefore profitably collect a network TX batch.
    pub fn has_operational_tx_block_ack(&self) -> bool {
        self.mac.engine().has_operational_tx_block_ack()
    }

    pub const fn tx_pending(&self) -> bool {
        self.mac.tx_pending()
    }

    pub const fn next_beacon_delay(&self, now_micros: u32) -> Option<(u32, u32)> {
        self.mac.next_beacon_delay(now_micros)
    }

    pub const fn beacon_publication_due(&self, now_micros: u32) -> bool {
        self.mac.beacon_publication_due(now_micros)
    }

    pub fn wait_tx_deadline(&mut self) -> impl core::future::Future<Output = ()> + '_ {
        self.mac.wait_tx_deadline()
    }

    pub fn publish_beacon<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        now_micros: u64,
    ) -> Result<(), Esp32s31AccessPointControlError> {
        let (missed, lateness) = self.mac.beacon_publication_lateness(now_micros as u32);
        self.report.missed_beacon_intervals =
            self.report.missed_beacon_intervals.saturating_add(missed);
        self.report.maximum_beacon_lateness_micros =
            self.report.maximum_beacon_lateness_micros.max(lateness);
        self.mac.publish_beacon(hardware, now_micros)?;
        Ok(())
    }

    /// Copy one network-owned Ethernet frame into the AP's ordinary DMA slot
    /// and begin a pairwise protected publication.
    ///
    /// The caller may release its network lease after this method returns:
    /// the complete plaintext MPDU is then owned by `self` until terminal TX.
    pub fn publish_ethernet<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        peer: [u8; 6],
        ethernet: &[u8],
    ) -> Result<(), Esp32s31AccessPointControlError> {
        self.mac
            .publish_ethernet(hardware, peer, ethernet, self.tx_frame)?;
        Ok(())
    }

    fn start_network_tx<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        ethernet: &[u8],
    ) -> Result<WifiTxProgress, Esp32s31AccessPointControlError> {
        self.report.network_tx_frames_observed =
            self.report.network_tx_frames_observed.saturating_add(1);
        match ethernet_protocol(ethernet) {
            Some(EthernetProtocol::ArpRequest) => {
                self.report.network_tx_arp_requests =
                    self.report.network_tx_arp_requests.saturating_add(1);
            }
            Some(EthernetProtocol::ArpReply) => {
                self.report.network_tx_arp_replies =
                    self.report.network_tx_arp_replies.saturating_add(1);
            }
            _ => {}
        }
        if self.mac.engine().authorized_peer_count() == 0 {
            self.report.network_tx_rejected_no_peer =
                self.report.network_tx_rejected_no_peer.saturating_add(1);
            self.report.network_tx_frames_rejected =
                self.report.network_tx_frames_rejected.saturating_add(1);
            return Ok(WifiTxProgress::Complete);
        }
        let Some(destination) = ethernet
            .get(..6)
            .and_then(|bytes| <[u8; 6]>::try_from(bytes).ok())
        else {
            self.report.network_tx_rejected_destination = self
                .report
                .network_tx_rejected_destination
                .saturating_add(1);
            self.report.network_tx_frames_rejected =
                self.report.network_tx_frames_rejected.saturating_add(1);
            return Ok(WifiTxProgress::Complete);
        };
        if destination[0] & 1 == 0 && !self.mac.engine().is_authorized_peer(destination) {
            self.report.network_tx_rejected_destination = self
                .report
                .network_tx_rejected_destination
                .saturating_add(1);
            self.report.network_tx_frames_rejected =
                self.report.network_tx_frames_rejected.saturating_add(1);
            return Ok(WifiTxProgress::Complete);
        }
        self.publish_ethernet(hardware, destination, ethernet)?;
        Ok(WifiTxProgress::Pending)
    }

    fn role_observation(&self) -> (u32, AccessPointServiceStatus, LinkState) {
        (
            self.mac.engine().service_status_revision(),
            self.mac.engine().service_status(),
            if self.mac.engine().authorized_peer_count() == 0 {
                LinkState::Down
            } else {
                LinkState::Up
            },
        )
    }

    /// Advance AP timer and peer policy by one finite WDEV control step.
    ///
    /// This method never waits. A published frame returns `TxPending`; the
    /// caller must drive the shared TX owner to a terminal edge before
    /// invoking another control transition.
    pub fn service_control<H>(
        &mut self,
        hardware: &mut H,
        now_micros: u64,
    ) -> Result<WdevControlProgress<Infallible>, Esp32s31AccessPointControlError>
    where
        H: TxHardware + Esp32s31ApRuntimeHardware,
    {
        if self.tx_pending() {
            return Ok(WdevControlProgress::TxPending);
        }
        if self.beacon_publication_due(now_micros as u32) {
            self.publish_beacon(hardware, now_micros)?;
            return Ok(WdevControlProgress::TxPending);
        }
        self.mac.expire_tx_block_ack(now_micros)?;
        match self
            .mac
            .engine_mut()
            .take_due_wpa2_retry::<EAPOL_CAPACITY>(now_micros)?
        {
            ApWpa2RetryProgress::Transmit { peer, frame } => {
                self.mac
                    .publish_eapol(hardware, peer, &frame, self.tx_frame)?;
                self.report.control_frames_staged =
                    self.report.control_frames_staged.saturating_add(1);
                return Ok(WdevControlProgress::TxPending);
            }
            ApWpa2RetryProgress::Close(close) => {
                self.publish_peer_close(hardware, close)?;
                return Ok(WdevControlProgress::TxPending);
            }
            ApWpa2RetryProgress::None => {}
        }
        if let Some(close) = self.mac.engine_mut().begin_due_peer_close(now_micros) {
            self.publish_peer_close(hardware, close)?;
            return Ok(WdevControlProgress::TxPending);
        }
        self.next_control_delay_millis(now_micros)?;
        Ok(WdevControlProgress::Idle)
    }

    fn next_control_delay_millis(
        &self,
        now_micros: u64,
    ) -> Result<u32, Esp32s31AccessPointControlError> {
        let (_, beacon_delay_ms) = self
            .next_beacon_delay(now_micros as u32)
            .ok_or(Esp32s31AccessPointControlError::InvalidBeaconSchedule)?;
        let deadline_delay = |deadline: u64| {
            let remaining = deadline.saturating_sub(now_micros);
            u32::try_from(remaining.saturating_add(999) / 1_000)
                .unwrap_or(u32::MAX)
                .max(1)
        };
        Ok(self
            .mac
            .engine()
            .next_peer_deadline()
            .into_iter()
            .chain(self.mac.engine().next_wpa2_retry_deadline())
            .chain(self.mac.next_tx_block_ack_deadline())
            .map(deadline_delay)
            .fold(beacon_delay_ms, u32::min))
    }

    /// Advance AP shutdown by one finite WDEV transition.
    pub fn service_stop<H>(
        &mut self,
        hardware: &mut H,
    ) -> Result<WdevStopProgress, Esp32s31AccessPointControlError>
    where
        H: TxHardware,
    {
        if let Some(close) = self.mac.engine_mut().begin_stop_peer() {
            self.publish_peer_close(hardware, close)?;
            Ok(WdevStopProgress::TxPending)
        } else {
            Ok(WdevStopProgress::Stopped)
        }
    }

    /// Run the AP control plane until the caller publishes stop.
    ///
    /// A pending TX descriptor is always driven to a terminal edge before IRQ
    /// routing is masked. RX is then stopped cooperatively; `Busy` means the
    /// walker has not yet acknowledged the request and is retried without
    /// weakening ownership.
    pub async fn run_until_stopped<
        'resources,
        R,
        NM,
        H,
        F,
        N,
        const FRAME_CAPACITY: usize,
        const HEADROOM: usize,
        const TRAILER: usize,
        const RX_QUEUE_DEPTH: usize,
        const TX_QUEUE_DEPTH: usize,
        const AMPDU_SLOTS: usize,
        const AMPDU_BUFFER_SIZE: usize,
    >(
        &mut self,
        hardware: &mut H,
        interrupts: &mut Esp32s31MacInterruptEpoch<'_, R, NM>,
        platform: &R::Platform,
        network: &SplitPinnedRadioRunner<
            'resources,
            NM,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            RX_QUEUE_DEPTH,
            TX_QUEUE_DEPTH,
        >,
        aggregate: &mut Esp32s31AccessPointAmpdu<
            'resources,
            PinnedTxFrame<'resources, NM, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
            AMPDU_SLOTS,
            AMPDU_BUFFER_SIZE,
        >,
        #[cfg(feature = "rx-delivery-observation")] delivery_observer: Option<
            &dyn RxNetworkDeliveryObserver,
        >,
        stop: F,
        mut status_observer: impl FnMut(AccessPointServiceStatus),
        security_material: N,
    ) -> Result<Esp32s31AccessPointRunReport, Esp32s31AccessPointRunError<R::Error>>
    where
        R: MacInterruptRoute,
        NM: RawMutex,
        H: RxDma
            + TxHardware
            + Esp32s31ApRuntimeHardware
            + open_esp_radio_esp32s31_wifi_mac::tx_ampdu::HtAmpduHardware,
        F: Future<Output = ()>,
        N: FnMut() -> ([u8; 32], u64),
    {
        network.set_link_state(LinkState::Down);
        network.set_hardware_address(self.mac.engine().service_address());
        self.start(hardware)
            .await
            .map_err(Esp32s31AccessPointRunError::Control)?;
        interrupts
            .activate(platform, MAC_COLD_RX_INTERRUPT_MASK)
            .map_err(Esp32s31AccessPointRunError::InterruptActivate)?;
        interrupts.mac_runtime().notify_rx_handoff();
        self.publish_beacon(hardware, Instant::now().as_micros())
            .map_err(Esp32s31AccessPointRunError::Control)?;
        let (last_status_revision, status, _) = self.role_observation();
        status_observer(status);
        let services = Esp32s31AccessPointWdevServices {
            control: self,
            hardware,
            network_tx: Esp32s31AccessPointNetworkTx::new(aggregate),
            status_observer,
            security_material,
            set_link_state: |state| network.set_link_state(state),
            #[cfg(feature = "rx-delivery-observation")]
            delivery_observer,
            last_status_revision,
            network_link_up: false,
            network_backpressure_since_micros: None,
            tx_pending_since_micros: Some(Instant::now().as_micros()),
            network_tx_pending: None,
            next_control_delay_millis: 1,
        };
        let mut runner = WdevRunner::new(interrupts.mac_runtime(), network, services);
        let exit = await_stack_boundary!(runner.run_until(stop)).map_err(|error| match error {
            Esp32s31AccessPointWdevError::Control(error) => {
                Esp32s31AccessPointRunError::Control(error)
            }
            Esp32s31AccessPointWdevError::Network(error) => {
                Esp32s31AccessPointRunError::Network(error)
            }
            Esp32s31AccessPointWdevError::Aggregate(error) => {
                Esp32s31AccessPointRunError::Aggregate(error)
            }
        })?;
        let (_, services) = runner.into_parts();
        match exit {
            crate::wdev::WdevRunnerExit::Stopped => {}
            crate::wdev::WdevRunnerExit::Role(exit) => match exit {},
        }
        drop(services);
        let rx_scheduler = self.receive.scheduler_snapshot();
        self.report.retained_rx_descriptors = rx_scheduler
            .map(|snapshot| snapshot.observed_mask.count_ones())
            .unwrap_or(0);
        let interrupt_drain = interrupts
            .quiesce(platform)
            .map_err(Esp32s31AccessPointRunError::InterruptQuiesce)?;
        loop {
            match self.stop(hardware) {
                Ok(()) => break,
                Err(Esp32s31AccessPointControlError::Receive(Esp32s31RxFrontierError::Ring(
                    open_esp_radio_esp32s31_wifi_mac::rx::RxRingError::Busy,
                ))) => yield_now().await,
                Err(error) => return Err(Esp32s31AccessPointRunError::Control(error)),
            }
        }
        Ok(Esp32s31AccessPointRunReport {
            control: self.report(),
            mac: self.mac_report(),
            engine: self.mac.engine().report(),
            interrupt_drain,
            rx_scheduler,
        })
    }

    /// Consume a quiescent AP service and return every reusable capability.
    /// Failure returns the exact service; no caller can manufacture stopped
    /// Wi-Fi while RX or TX remains active.
    #[allow(clippy::result_large_err, clippy::type_complexity)]
    pub fn try_finish<H: Esp32s31ApRuntimeHardware>(
        self,
        hardware: &mut H,
    ) -> Result<
        Esp32s31AccessPointStopped<
            'storage,
            'beacon,
            'slot,
            P,
            E,
            T,
            COUNT,
            DMA_BUFFER_SIZE,
            DMA_STORAGE_SIZE,
            TX_BUFFER_SIZE,
        >,
        Self,
    > {
        if self.rx_batch_pending() {
            return Err(self);
        }
        let Self {
            receive,
            storage,
            mac,
            rx_frame,
            tx_frame,
            data_rx,
            rx_batch_used: _,
            rx_batch_offset: _,
            report,
        } = self;
        let ring = match receive.try_into_halted() {
            Ok(ring) => ring,
            Err(receive) => {
                return Err(Self {
                    receive,
                    storage,
                    mac,
                    rx_frame,
                    tx_frame,
                    data_rx,
                    rx_batch_used: 0,
                    rx_batch_offset: 0,
                    report,
                });
            }
        };
        let (engine, transmit, mac_report) = match mac.try_into_parts() {
            Ok(parts) => parts,
            Err(mac) => {
                return Err(Self {
                    receive: Esp32s31RxFrontier::from_halted(ring),
                    storage,
                    mac,
                    rx_frame,
                    tx_frame,
                    data_rx,
                    rx_batch_used: 0,
                    rx_batch_offset: 0,
                    report,
                });
            }
        };
        let engine = engine.stop(hardware);
        Ok(Esp32s31AccessPointStopped {
            ring,
            storage,
            transmit,
            rx_frame,
            tx_frame,
            data_rx,
            engine,
            control_report: report,
            mac_report,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EthernetProtocol {
    ArpRequest,
    ArpReply,
    Ipv4Tcp,
    Ipv4Other,
    Other,
}

fn ethernet_protocol(frame: &[u8]) -> Option<EthernetProtocol> {
    let ether_type = u16::from_be_bytes([*frame.get(12)?, *frame.get(13)?]);
    match ether_type {
        0x0800 => Some(if *frame.get(23)? == 6 {
            EthernetProtocol::Ipv4Tcp
        } else {
            EthernetProtocol::Ipv4Other
        }),
        0x0806 => match u16::from_be_bytes([*frame.get(20)?, *frame.get(21)?]) {
            1 => Some(EthernetProtocol::ArpRequest),
            2 => Some(EthernetProtocol::ArpReply),
            _ => Some(EthernetProtocol::Other),
        },
        _ => Some(EthernetProtocol::Other),
    }
}

fn ethernet_parts_protocol(frame: EthernetFrameParts<'_>) -> Option<EthernetProtocol> {
    match frame.ether_type {
        0x0800 => Some(if *frame.payload.get(9)? == 6 {
            EthernetProtocol::Ipv4Tcp
        } else {
            EthernetProtocol::Ipv4Other
        }),
        0x0806 => match u16::from_be_bytes(frame.payload.get(6..8)?.try_into().ok()?) {
            1 => Some(EthernetProtocol::ArpRequest),
            2 => Some(EthernetProtocol::ArpReply),
            _ => Some(EthernetProtocol::Other),
        },
        _ => Some(EthernetProtocol::Other),
    }
}
