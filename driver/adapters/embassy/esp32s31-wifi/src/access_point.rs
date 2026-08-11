//! Embassy-owned AP control-plane RX/TX service.
//!
//! The initial service handles beacons, management frames and WPA2 EAPOL.
//! Authorized Ethernet traffic is intentionally a later data-plane owner; it
//! must not be silently dropped through this finite control path.

use core::{future::Future, pin::pin};

use embassy_futures::{
    select::{Either, Either3, select, select3},
    yield_now,
};
use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_time::{Instant, Timer};

use open_esp_radio_esp32s31_wifi::{
    ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile, WifiTxResources, WifiTxTimer},
    tx::{WifiTxProgress, WifiTxWake},
};
use open_esp_radio_esp32s31_wifi_ap::{
    engine::{Esp32s31ApRuntimeHardware, Esp32s31ApWpa2Outcome},
    mac::{Esp32s31ApMac, Esp32s31ApMacError, Esp32s31ApMacReport, Esp32s31ApTxCompletionAction},
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

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Esp32s31AccessPointControlReport {
    pub completed_rx_descriptors: u32,
    pub ignored_rx_frames: u32,
    pub control_frames_staged: u32,
    pub control_frames_dropped_while_busy: u32,
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
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Esp32s31AccessPointRunReport {
    pub control: Esp32s31AccessPointControlReport,
    pub mac: Esp32s31ApMacReport,
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
        Self {
            receive,
            storage,
            mac,
            rx_frame,
            tx_frame,
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
    ) -> Result<(), Esp32s31AccessPointControlError>
    where
        H: RxDma + TxHardware + Esp32s31ApRuntimeHardware,
    {
        if self.mac.tx_pending() {
            return Ok(());
        }
        let mut staged = None;
        let rx_frame = &mut *self.rx_frame;
        let progress = self
            .receive
            .service_completed(hardware, self.storage, |segment| {
                if staged.is_some() {
                    self.report.control_frames_dropped_while_busy = self
                        .report
                        .control_frames_dropped_while_busy
                        .saturating_add(1);
                    return Esp32s31PreconnectedRxDirective::Continue;
                }
                let Ok(frame) = view_normalized_rx_frame(
                    &segment,
                    RxIngressConfig {
                        ring_entry_limit: 1,
                        csi_config: 0,
                        flags: 0,
                    },
                ) else {
                    self.report.ignored_rx_frames = self.report.ignored_rx_frames.saturating_add(1);
                    return Esp32s31PreconnectedRxDirective::Continue;
                };
                if frame.mpdu.len() > rx_frame.len() {
                    self.report.ignored_rx_frames = self.report.ignored_rx_frames.saturating_add(1);
                    return Esp32s31PreconnectedRxDirective::Continue;
                }
                rx_frame[..frame.mpdu.len()].copy_from_slice(frame.mpdu);
                let frame_control = u16::from_le_bytes([frame.mpdu[0], frame.mpdu[1]]);
                staged = if frame_control & 0x000c == 0 {
                    Some(StagedControlFrame::Management {
                        length: frame.mpdu.len(),
                    })
                } else if frame_control & 0x000c == 0x0008 {
                    Some(StagedControlFrame::Eapol {
                        length: frame.mpdu.len(),
                    })
                } else {
                    self.report.ignored_rx_frames = self.report.ignored_rx_frames.saturating_add(1);
                    None
                };
                Esp32s31PreconnectedRxDirective::Continue
            })?;
        self.report.completed_rx_descriptors = self
            .report
            .completed_rx_descriptors
            .saturating_add(progress.completed);

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
        }
        Ok(())
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

    pub fn wait_tx_deadline(&mut self) -> impl core::future::Future<Output = ()> + '_ {
        self.mac.wait_tx_deadline()
    }

    pub fn publish_beacon<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        now_micros: u64,
    ) -> Result<(), Esp32s31AccessPointControlError> {
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
    pub async fn run_until_stopped<R, M, H, F, N>(
        &mut self,
        hardware: &mut H,
        interrupts: &mut Esp32s31MacInterruptEpoch<'_, R, M>,
        platform: &R::Platform,
        stop: F,
        mut security_material: N,
    ) -> Result<Esp32s31AccessPointRunReport, Esp32s31AccessPointRunError<R::Error>>
    where
        R: MacInterruptRoute,
        M: RawMutex,
        H: RxDma + TxHardware + Esp32s31ApRuntimeHardware,
        F: Future<Output = ()>,
        N: FnMut() -> ([u8; 32], u64),
    {
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
        loop {
            if self.tx_pending() {
                let tx_edge = select(interrupts.mac_runtime().wait_tx(), self.wait_tx_deadline());
                let wake = if stopping {
                    match tx_edge.await {
                        Either::First(events) => WifiTxWake::Interrupt { events },
                        Either::Second(()) => WifiTxWake::Deadline,
                    }
                } else {
                    match select(stop.as_mut(), tx_edge).await {
                        Either::First(()) => {
                            stopping = true;
                            continue;
                        }
                        Either::Second(Either::First(events)) => WifiTxWake::Interrupt { events },
                        Either::Second(Either::Second(())) => WifiTxWake::Deadline,
                    }
                };
                self.service_tx(hardware, wake)
                    .await
                    .map_err(Esp32s31AccessPointRunError::Control)?;
                continue;
            }
            if stopping {
                break;
            }

            let now_micros = Instant::now().as_micros();
            let (_, beacon_delay_ms) = self
                .next_beacon_delay(now_micros as u32)
                .ok_or(Esp32s31AccessPointRunError::InvalidBeaconSchedule)?;
            match select3(
                stop.as_mut(),
                interrupts.mac_runtime().wait_rx(),
                Timer::after_millis(u64::from(beacon_delay_ms)),
            )
            .await
            {
                Either3::First(()) => stopping = true,
                Either3::Second(()) => {
                    let (nonce, replay_counter) = security_material();
                    self.service_rx(hardware, nonce, replay_counter)
                        .map_err(Esp32s31AccessPointRunError::Control)?;
                }
                Either3::Third(()) => {
                    self.publish_beacon(hardware, Instant::now().as_micros())
                        .map_err(Esp32s31AccessPointRunError::Control)?;
                }
            }
        }

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
            engine,
            control_report: report,
            mac_report,
        })
    }
}
