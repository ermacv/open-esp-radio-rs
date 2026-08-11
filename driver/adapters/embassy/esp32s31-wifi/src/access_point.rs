//! Embassy-owned AP control-plane RX/TX service.
//!
//! The initial service handles beacons, management frames and WPA2 EAPOL.
//! Authorized Ethernet traffic is intentionally a later data-plane owner; it
//! must not be silently dropped through this finite control path.

use core::{future::Future, pin::pin};

use embassy_futures::{
    select::{Either, Either4, select, select4},
    yield_now,
};
use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_time::{Instant, Timer};

use open_esp_radio_embassy_net::{
    FrameLengthError, LinkState, RxEnqueueError, SplitPinnedRadioRunner,
};

use open_esp_radio_esp32s31_wifi::{
    ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile, WifiTxResources, WifiTxTimer},
    tx::{WifiTxProgress, WifiTxWake},
};
use open_esp_radio_esp32s31_wifi_ap::{
    engine::{Esp32s31ApRuntimeHardware, Esp32s31ApWpa2Outcome},
    mac::{Esp32s31ApMac, Esp32s31ApMacError, Esp32s31ApMacReport, Esp32s31ApTxCompletionAction},
    rx::{
        Esp32s31ApRxConfig, Esp32s31ApRxDispatch, Esp32s31ApRxDispatcher, Esp32s31ApRxError,
        Esp32s31ApRxEvent, Esp32s31ApRxSink,
    },
};
use open_esp_radio_esp32s31_wifi_mac::{
    init::MAC_COLD_RX_INTERRUPT_MASK,
    irq::MacInterruptRoute,
    rx::{RxDma, RxIngressConfig, RxRingHalted, view_normalized_rx_frame},
    tx::TxHardware,
};
use open_esp_radio_ieee80211::data::{
    DataInterfaceRole, IEEE80211_LEGACY_DATA_HEADER_LEN, IEEE80211_QOS_DATA_HEADER_LEN,
    plan_data_decapsulation,
};
use open_esp_radio_wpa2::{OwnedEapolFrame, Wpa2Interface};

use crate::{
    embassy_irq::{
        Esp32s31MacInterruptEpoch, Esp32s31MacInterruptEpochActivateError,
        Esp32s31MacInterruptEpochDrain, Esp32s31MacInterruptEpochQuiesceError,
    },
    preconnected_rx::{
        Esp32s31PreconnectedRx, Esp32s31PreconnectedRxDelay, Esp32s31PreconnectedRxDirective,
        Esp32s31PreconnectedRxError,
    },
    rx_dma_service::Esp32s31RxDmaStorage,
};

const EAPOL_ETHERTYPE: u16 = 0x888e;
const EAPOL_CAPACITY: usize = 512;

/// Wait for AP work in protocol priority order.
///
/// `embassy_futures::select4` is intentionally biased toward its first ready
/// future. Stop remains the ownership boundary, but an elapsed TBTT must win
/// over RX and network traffic: otherwise a continuously ready RX signal can
/// postpone beacons by complete intervals in a busy channel.
async fn wait_for_ap_work<S, B, R, N>(
    stop: S,
    beacon: B,
    rx: R,
    network: N,
) -> Either4<S::Output, B::Output, R::Output, N::Output>
where
    S: Future,
    B: Future,
    R: Future,
    N: Future,
{
    select4(stop, beacon, rx, network).await
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Esp32s31AccessPointControlReport {
    pub missed_beacon_intervals: u32,
    pub maximum_beacon_lateness_micros: u32,
    pub tx_interrupt_wakes: u32,
    pub tx_deadline_wakes: u32,
    pub maximum_tx_pending_micros: u32,
    pub maximum_rx_service_micros: u32,
    pub maximum_network_backpressure_micros: u32,
    pub completed_rx_descriptors: u32,
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
    Receive(Esp32s31PreconnectedRxError),
    Mac(Esp32s31ApMacError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31AccessPointRunError<E> {
    Control(Esp32s31AccessPointControlError),
    InterruptActivate(Esp32s31MacInterruptEpochActivateError<E>),
    InterruptQuiesce(Esp32s31MacInterruptEpochQuiesceError<E>),
    InvalidBeaconSchedule,
    Network(FrameLengthError),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Esp32s31AccessPointRunReport {
    pub control: Esp32s31AccessPointControlReport,
    pub mac: Esp32s31ApMacReport,
    pub engine: open_esp_radio_esp32s31_wifi_ap::engine::Esp32s31ApEngineReport,
    pub interrupt_drain: Esp32s31MacInterruptEpochDrain,
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
    pub engine: open_esp_radio_esp32s31_wifi_ap::engine::Esp32s31ApEngineStop<'beacon>,
    pub control_report: Esp32s31AccessPointControlReport,
    pub mac_report: Esp32s31ApMacReport,
}

impl From<Esp32s31PreconnectedRxError> for Esp32s31AccessPointControlError {
    fn from(error: Esp32s31PreconnectedRxError) -> Self {
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
    Ethernet,
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
    receive: Esp32s31PreconnectedRx<'storage, D, COUNT, DMA_BUFFER_SIZE>,
    storage: &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    mac: Esp32s31ApMac<'beacon, 'slot, P, E, T, TX_BUFFER_SIZE>,
    rx_frame: &'storage mut [u8],
    tx_frame: &'storage mut [u8],
    data_rx: Esp32s31ApRxDispatcher,
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
    D: Esp32s31PreconnectedRxDelay,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
{
    pub fn new(
        receive: Esp32s31PreconnectedRx<'storage, D, COUNT, DMA_BUFFER_SIZE>,
        storage: &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        mac: Esp32s31ApMac<'beacon, 'slot, P, E, T, TX_BUFFER_SIZE>,
        rx_frame: &'storage mut [u8],
        tx_frame: &'storage mut [u8],
    ) -> Self {
        let access_point = mac.engine().service_address();
        Self {
            receive,
            storage,
            mac,
            rx_frame,
            tx_frame,
            data_rx: Esp32s31ApRxDispatcher::new(Esp32s31ApRxConfig {
                access_point,
                ingress: RxIngressConfig {
                    ring_entry_limit: 1,
                    csi_config: 0,
                    flags: 0,
                },
            }),
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

    /// Drain completed descriptors and stage at most one control MPDU.
    ///
    /// The RX walker is always recycled before TX receives the PAC borrow.
    /// This avoids nested mutable access to one register owner and makes the
    /// ISR→RX→TX ordering explicit.
    pub fn service_rx<H>(
        &mut self,
        hardware: &mut H,
        authenticator_nonce: [u8; 32],
        initial_replay_counter: u64,
    ) -> Result<Option<usize>, Esp32s31AccessPointControlError>
    where
        H: RxDma + TxHardware + Esp32s31ApRuntimeHardware,
    {
        if self.mac.tx_pending() {
            return Ok(None);
        }
        let mut staged = None;
        let mut ethernet_length = None;
        let rx_frame = &mut *self.rx_frame;
        let authorized_peer = self.mac.engine().authorized_peer();
        let data_rx = &mut self.data_rx;
        let report = &mut self.report;
        let progress = self
            .receive
            .service_completed(hardware, self.storage, |segment| {
                if staged.is_some() {
                    report.control_frames_dropped_while_busy =
                        report.control_frames_dropped_while_busy.saturating_add(1);
                    return Esp32s31PreconnectedRxDirective::Pause;
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
                        return Esp32s31PreconnectedRxDirective::Continue;
                    }
                };
                let frame_control = u16::from_le_bytes([frame.mpdu[0], frame.mpdu[1]]);
                if frame_control & 0x000c == 0x0008 && frame_control & 0x4000 != 0 {
                    struct StageEthernet<'a> {
                        storage: &'a mut [u8],
                        length: &'a mut Option<usize>,
                    }

                    impl Esp32s31ApRxSink for StageEthernet<'_> {
                        fn publish(&mut self, event: Esp32s31ApRxEvent<'_>) {
                            if self.length.is_some() || event.frame.length() > self.storage.len() {
                                return;
                            }
                            if event.frame.copy_to(self.storage).is_ok() {
                                *self.length = Some(event.frame.length());
                            }
                        }
                    }

                    let mut sink = StageEthernet {
                        storage: rx_frame,
                        length: &mut ethernet_length,
                    };
                    report.protected_data_frames = report.protected_data_frames.saturating_add(1);
                    match data_rx.dispatch_protected(segment, authorized_peer, &mut sink) {
                        Esp32s31ApRxDispatch::Data { .. } => {}
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
                    if ethernet_length.is_some() {
                        report.ethernet_frames_staged =
                            report.ethernet_frames_staged.saturating_add(1);
                        let ethernet = &rx_frame[..ethernet_length.unwrap_or(0)];
                        match ethernet_protocol(ethernet) {
                            Some(EthernetProtocol::ArpRequest) => {
                                report.ethernet_arp_requests_staged =
                                    report.ethernet_arp_requests_staged.saturating_add(1);
                            }
                            Some(EthernetProtocol::Ipv4Tcp) => {
                                report.ethernet_tcp_frames_staged =
                                    report.ethernet_tcp_frames_staged.saturating_add(1);
                            }
                            _ => {}
                        }
                        staged = Some(StagedControlFrame::Ethernet);
                        return Esp32s31PreconnectedRxDirective::Pause;
                    }
                    report.ignored_rx_frames = report.ignored_rx_frames.saturating_add(1);
                    return Esp32s31PreconnectedRxDirective::Continue;
                }
                if frame.mpdu.len() > rx_frame.len() {
                    report.ignored_rx_frames = report.ignored_rx_frames.saturating_add(1);
                    return Esp32s31PreconnectedRxDirective::Continue;
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
                    Esp32s31PreconnectedRxDirective::Pause
                } else {
                    Esp32s31PreconnectedRxDirective::Continue
                }
            })?;
        self.report.completed_rx_descriptors = self
            .report
            .completed_rx_descriptors
            .saturating_add(progress.completed);

        let Some(staged) = staged else {
            return Ok(None);
        };
        match staged {
            StagedControlFrame::Management { length } => {
                let outcome = self.mac.publish_management(
                    hardware,
                    &self.rx_frame[..length],
                    authenticator_nonce,
                    initial_replay_counter,
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
                self.service_eapol(hardware, length)?;
            }
            StagedControlFrame::Ethernet => {}
        }
        Ok(ethernet_length)
    }

    fn service_eapol<H>(
        &mut self,
        hardware: &mut H,
        length: usize,
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
            || self.mac.engine().peer() != Some(plan.source)
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
            .handle_eapol(hardware, plan.source, frame)?
        {
            Esp32s31ApWpa2Outcome::Transmit(frame) => {
                self.mac
                    .publish_eapol(hardware, plan.source, &frame, self.tx_frame)?;
                self.report.control_frames_staged =
                    self.report.control_frames_staged.saturating_add(1);
            }
            Esp32s31ApWpa2Outcome::None
            | Esp32s31ApWpa2Outcome::PeerAuthorized { .. }
            | Esp32s31ApWpa2Outcome::DeauthenticatePeer { .. } => {}
        }
        Ok(())
    }

    pub async fn service_tx<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        wake: WifiTxWake,
    ) -> Result<WifiTxProgress, Esp32s31AccessPointControlError> {
        let (progress, action) = self.mac.service_tx(hardware, wake).await?;
        if let Esp32s31ApTxCompletionAction::BeginWpa2 { peer } = action {
            let message1 = self.mac.engine().begin_wpa2::<EAPOL_CAPACITY>(peer)?;
            self.mac
                .publish_eapol(hardware, peer, &message1, self.tx_frame)?;
            self.report.control_frames_staged = self.report.control_frames_staged.saturating_add(1);
            return Ok(WifiTxProgress::Pending);
        }
        Ok(progress)
    }

    pub const fn report(&self) -> Esp32s31AccessPointControlReport {
        self.report
    }

    pub const fn mac_report(&self) -> Esp32s31ApMacReport {
        self.mac.report()
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

    /// Run the AP control plane until the caller publishes stop.
    ///
    /// A pending TX descriptor is always driven to a terminal edge before IRQ
    /// routing is masked. RX is then stopped cooperatively; `Busy` means the
    /// walker has not yet acknowledged the request and is retried without
    /// weakening ownership.
    pub async fn run_until_stopped<
        R,
        M,
        NM,
        H,
        F,
        N,
        const FRAME_CAPACITY: usize,
        const HEADROOM: usize,
        const TRAILER: usize,
        const RX_QUEUE_DEPTH: usize,
        const TX_QUEUE_DEPTH: usize,
    >(
        &mut self,
        hardware: &mut H,
        interrupts: &mut Esp32s31MacInterruptEpoch<'_, R, M>,
        platform: &R::Platform,
        network: &SplitPinnedRadioRunner<
            '_,
            NM,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            RX_QUEUE_DEPTH,
            TX_QUEUE_DEPTH,
        >,
        stop: F,
        mut security_material: N,
    ) -> Result<Esp32s31AccessPointRunReport, Esp32s31AccessPointRunError<R::Error>>
    where
        R: MacInterruptRoute,
        M: RawMutex,
        NM: RawMutex,
        H: RxDma + TxHardware + Esp32s31ApRuntimeHardware,
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

        let mut stop = pin!(stop);
        let mut stopping = false;
        let mut network_link_up = false;
        let mut tx_pending_since_micros = None;
        let mut pending_network_rx = None;
        let mut network_backpressure_since_micros = None;
        let mut network_rx = network.rx_publisher();
        loop {
            if self.tx_pending() {
                let pending_since =
                    *tx_pending_since_micros.get_or_insert_with(|| Instant::now().as_micros());
                let tx_edge = select(interrupts.mac_runtime().wait_tx(), self.wait_tx_deadline());
                let (wake, deadline) = if stopping {
                    match tx_edge.await {
                        Either::First(events) => (WifiTxWake::Interrupt { events }, false),
                        Either::Second(()) => (WifiTxWake::Deadline, true),
                    }
                } else {
                    match select(stop.as_mut(), tx_edge).await {
                        Either::First(()) => {
                            stopping = true;
                            continue;
                        }
                        Either::Second(Either::First(events)) => {
                            (WifiTxWake::Interrupt { events }, false)
                        }
                        Either::Second(Either::Second(())) => (WifiTxWake::Deadline, true),
                    }
                };
                if deadline {
                    self.report.tx_deadline_wakes = self.report.tx_deadline_wakes.saturating_add(1);
                } else {
                    self.report.tx_interrupt_wakes =
                        self.report.tx_interrupt_wakes.saturating_add(1);
                }
                self.service_tx(hardware, wake)
                    .await
                    .map_err(Esp32s31AccessPointRunError::Control)?;
                if !self.tx_pending() {
                    let elapsed = Instant::now().as_micros().saturating_sub(pending_since);
                    self.report.maximum_tx_pending_micros = self
                        .report
                        .maximum_tx_pending_micros
                        .max(u32::try_from(elapsed).unwrap_or(u32::MAX));
                    tx_pending_since_micros = None;
                }
                continue;
            }
            if stopping {
                if pending_network_rx.is_none() {
                    break;
                }
            }

            let now_micros = Instant::now().as_micros();
            // A data publication may finish just after TBTT. The recovered
            // next-deadline calculation intentionally advances past an
            // already missed edge, so publish the overdue beacon before
            // admitting another network frame. The active descriptor above
            // still always reaches a terminal outcome first.
            if self.beacon_publication_due(now_micros as u32) {
                self.publish_beacon(hardware, now_micros)
                    .map_err(Esp32s31AccessPointRunError::Control)?;
                continue;
            }
            let (_, beacon_delay_ms) = self
                .next_beacon_delay(now_micros as u32)
                .ok_or(Esp32s31AccessPointRunError::InvalidBeaconSchedule)?;
            if let Some(length) = pending_network_rx {
                let ready = select(
                    Timer::after_millis(u64::from(beacon_delay_ms)),
                    network_rx.wait_ready(),
                );
                let ready = if stopping {
                    ready.await
                } else {
                    match select(stop.as_mut(), ready).await {
                        Either::First(()) => {
                            stopping = true;
                            continue;
                        }
                        Either::Second(ready) => ready,
                    }
                };
                match ready {
                    Either::First(()) => {
                        self.publish_beacon(hardware, Instant::now().as_micros())
                            .map_err(Esp32s31AccessPointRunError::Control)?;
                    }
                    Either::Second(()) => {
                        network_rx
                            .try_send(&self.rx_frame[..length])
                            .expect("wait_ready reserved one pinned AP RX slot");
                        if let Some(started) = network_backpressure_since_micros.take() {
                            let elapsed = Instant::now().as_micros().saturating_sub(started);
                            self.report.maximum_network_backpressure_micros = self
                                .report
                                .maximum_network_backpressure_micros
                                .max(u32::try_from(elapsed).unwrap_or(u32::MAX));
                        }
                        pending_network_rx = None;
                    }
                }
                continue;
            }
            match wait_for_ap_work(
                stop.as_mut(),
                Timer::after_millis(u64::from(beacon_delay_ms)),
                interrupts.mac_runtime().wait_rx(),
                network.receive_tx(),
            )
            .await
            {
                Either4::First(()) => stopping = true,
                Either4::Second(()) => {
                    self.publish_beacon(hardware, Instant::now().as_micros())
                        .map_err(Esp32s31AccessPointRunError::Control)?;
                }
                Either4::Third(()) => {
                    // One IRQ publication may represent several completed
                    // descriptors. Drain until the walker reports no further
                    // ownership instead of requiring a second interrupt edge
                    // for work which was already complete when the first edge
                    // was acknowledged. TBTT is checked between descriptors;
                    // this is a deadline budget rather than a packet-count
                    // batch and therefore cannot impose an artificial RX
                    // throughput ceiling.
                    loop {
                        if self.beacon_publication_due(Instant::now().as_micros() as u32) {
                            break;
                        }
                        let (nonce, replay_counter) = security_material();
                        let completed_before = self.report.completed_rx_descriptors;
                        let service_started = Instant::now().as_micros();
                        let ethernet_length = self
                            .service_rx(hardware, nonce, replay_counter)
                            .map_err(Esp32s31AccessPointRunError::Control)?;
                        let service_elapsed =
                            Instant::now().as_micros().saturating_sub(service_started);
                        self.report.maximum_rx_service_micros = self
                            .report
                            .maximum_rx_service_micros
                            .max(u32::try_from(service_elapsed).unwrap_or(u32::MAX));
                        if let Some(length) = ethernet_length {
                            match network_rx.try_send(&self.rx_frame[..length]) {
                                Ok(()) => {}
                                Err(RxEnqueueError::QueueFull) => {
                                    pending_network_rx = Some(length);
                                    network_backpressure_since_micros =
                                        Some(Instant::now().as_micros());
                                    break;
                                }
                                Err(RxEnqueueError::InvalidLength(error)) => {
                                    return Err(Esp32s31AccessPointRunError::Network(error));
                                }
                            }
                        }
                        if self.report.completed_rx_descriptors == completed_before {
                            break;
                        }
                    }
                    let authorized = self.mac.engine().authorized_peer().is_some();
                    if authorized != network_link_up {
                        network.set_link_state(if authorized {
                            LinkState::Up
                        } else {
                            LinkState::Down
                        });
                        network_link_up = authorized;
                    }
                }
                Either4::Fourth(frame) => {
                    self.report.network_tx_frames_observed =
                        self.report.network_tx_frames_observed.saturating_add(1);
                    match ethernet_protocol(frame.as_slice()) {
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
                    let Some(peer) = self.mac.engine().authorized_peer() else {
                        self.report.network_tx_rejected_no_peer =
                            self.report.network_tx_rejected_no_peer.saturating_add(1);
                        self.report.network_tx_frames_rejected =
                            self.report.network_tx_frames_rejected.saturating_add(1);
                        continue;
                    };
                    if frame.as_slice().get(..6) != Some(peer.as_slice()) {
                        // AP v1 deliberately has no GTK TX packet-number
                        // owner. Group and foreign-unicast Ethernet therefore
                        // remain outside the advertised data service.
                        self.report.network_tx_rejected_destination = self
                            .report
                            .network_tx_rejected_destination
                            .saturating_add(1);
                        self.report.network_tx_frames_rejected =
                            self.report.network_tx_frames_rejected.saturating_add(1);
                        continue;
                    }
                    self.publish_ethernet(hardware, peer, frame.as_slice())
                        .map_err(Esp32s31AccessPointRunError::Control)?;
                }
            }
        }

        network.set_link_state(LinkState::Down);
        let interrupt_drain = interrupts
            .quiesce(platform)
            .map_err(Esp32s31AccessPointRunError::InterruptQuiesce)?;
        loop {
            match self.stop(hardware) {
                Ok(()) => break,
                Err(Esp32s31AccessPointControlError::Receive(
                    Esp32s31PreconnectedRxError::Ring(
                        open_esp_radio_esp32s31_wifi_mac::rx::RxRingError::Busy,
                    ),
                )) => yield_now().await,
                Err(error) => return Err(Esp32s31AccessPointRunError::Control(error)),
            }
        }
        Ok(Esp32s31AccessPointRunReport {
            control: self.report(),
            mac: self.mac_report(),
            engine: self.mac.engine().report(),
            interrupt_drain,
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
        let Self {
            receive,
            storage,
            mac,
            rx_frame,
            tx_frame,
            data_rx,
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
                    report,
                });
            }
        };
        let (engine, transmit, mac_report) = match mac.try_into_parts() {
            Ok(parts) => parts,
            Err(mac) => {
                return Err(Self {
                    receive: Esp32s31PreconnectedRx::from_halted(ring),
                    storage,
                    mac,
                    rx_frame,
                    tx_frame,
                    data_rx,
                    report,
                });
            }
        };
        let engine = engine.stop(hardware);
        let _ = data_rx;
        Ok(Esp32s31AccessPointStopped {
            ring,
            storage,
            transmit,
            rx_frame,
            tx_frame,
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

#[cfg(test)]
mod tests {
    use core::future::{pending, ready};

    use embassy_futures::{block_on, select::Either4};

    use super::wait_for_ap_work;

    #[test]
    fn elapsed_tbtt_wins_over_simultaneously_ready_rx() {
        let selected = block_on(wait_for_ap_work(
            pending::<()>(),
            ready(()),
            ready(()),
            pending::<()>(),
        ));

        assert!(matches!(selected, Either4::Second(())));
    }
}
