//! Concrete scan-port composition for cold and quiesced connected STA epochs.
//!
//! The chip-independent lifecycle service owns plan progress and retry policy;
//! [`Esp32s31StaScanBackend`](open_esp_radio_esp32s31_wifi_sta::scan::Esp32s31StaScanBackend)
//! owns the
//! mandatory ESP32-S31 transaction order. This module binds that transaction
//! to the returned PHY, cooperative register owner, RX ring and control-TX
//! descriptor without importing board fixtures, credentials or diagnostics.

use core::future::Future;

use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_time::Timer;
use open_esp_radio_esp32s31_wifi::ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile, WifiTxTimer};
use open_esp_radio_esp32s31_wifi_mac::{rx::RxDma, tx::TxHardware};
use open_esp_radio_esp32s31_wifi_sta::{
    control_tx::ControlTxError,
    scan::{Esp32s31ActiveProbeOutcome, Esp32s31StaScanPort},
    scan_tx::{Esp32s31RunningScanTx, Esp32s31ScanProbeReport, Esp32s31ScanProbeRequest},
};
use open_esp_radio_ieee80211::{
    scan::{ScanRecord, ScanTable, best_matching_ssid},
    station::StaSequenceCounter,
};
use open_esp_radio_wifi_sta::scan::StaScanChannelContext;

use crate::{
    embassy_rx::RxReloadDelay,
    rx_ring_owner::Esp32s31RxRingOwnerError,
    scan_rx::{
        Esp32s31RunningScanRx, Esp32s31ScanFrameObserver, Esp32s31ScanObservationContext,
        Esp32s31ScanRxProgress,
    },
};

/// PHY channel-switch capability required by a running scan.
pub trait Esp32s31ScanPhyPort<H> {
    type Error;

    fn switch_channel<'a>(
        &'a mut self,
        hardware: &'a mut H,
        channel: u8,
    ) -> impl Future<Output = Result<(), Self::Error>> + 'a;
}

/// RX-ring capability retained across every finite running-scan channel.
pub trait Esp32s31ScanReceivePort<H> {
    type Error;

    fn prepare_initial(&mut self, hardware: &mut H) -> Result<(), Self::Error>;

    fn start<'a>(
        &'a mut self,
        hardware: &'a mut H,
    ) -> impl Future<Output = Result<(), Self::Error>> + 'a;

    fn observe_management<O, const RECORDS: usize>(
        &mut self,
        hardware: &mut H,
        context: &mut Esp32s31ScanObservationContext<'_, O, RECORDS>,
    ) -> Result<Esp32s31ScanRxProgress, Self::Error>
    where
        O: Esp32s31ScanFrameObserver;

    fn stop(&mut self, hardware: &mut H) -> Result<(), Self::Error>;

    fn prepare_next(&mut self, hardware: &mut H) -> Result<(), Self::Error>;
}

/// Polling control-TX capability available only after connected IRQ teardown.
pub trait Esp32s31ScanTransmitPort<H> {
    type Error;

    fn begin_scan(&mut self);

    fn transmit_probe_request<'a>(
        &'a mut self,
        hardware: &'a mut H,
        request: Esp32s31ScanProbeRequest<'a>,
    ) -> impl Future<Output = Result<Esp32s31ScanProbeReport, Self::Error>> + 'a;
}

/// Executor clock edge for one scan dwell tick.
pub trait Esp32s31ScanTimer {
    fn wait_dwell_tick(&mut self) -> impl Future<Output = ()> + '_;
}

/// Production one-millisecond Embassy dwell tick.
#[derive(Clone, Copy, Debug, Default)]
pub struct EmbassyEsp32s31ScanTimer;

impl Esp32s31ScanTimer for EmbassyEsp32s31ScanTimer {
    fn wait_dwell_tick(&mut self) -> impl Future<Output = ()> + '_ {
        Timer::after_millis(1)
    }
}

/// Driver resources returned by one completely quiesced connected epoch.
pub struct Esp32s31ScanRadio<P, H, R, T> {
    phy: P,
    hardware: H,
    rx: R,
    tx: T,
}

impl<P, H, R, T> Esp32s31ScanRadio<P, H, R, T> {
    pub const fn new(phy: P, hardware: H, rx: R, tx: T) -> Self {
        Self {
            phy,
            hardware,
            rx,
            tx,
        }
    }
}

/// Borrowed allocation-free storage for one running scan.
pub struct Esp32s31ScanStorage<'resources, 'sequence, O, const RECORDS: usize> {
    table: &'resources mut ScanTable<RECORDS>,
    frame: &'resources mut [u8],
    observer: O,
    sequence: &'sequence mut StaSequenceCounter,
}

impl<'resources, 'sequence, O, const RECORDS: usize>
    Esp32s31ScanStorage<'resources, 'sequence, O, RECORDS>
{
    pub fn new(
        table: &'resources mut ScanTable<RECORDS>,
        frame: &'resources mut [u8],
        observer: O,
        sequence: &'sequence mut StaSequenceCounter,
    ) -> Self {
        Self {
            table,
            frame,
            observer,
            sequence,
        }
    }
}

/// Peer-independent station policy for active scan and candidate selection.
pub struct Esp32s31ScanStation<'ssid, 'rates> {
    station_address: [u8; 6],
    target_ssid: &'ssid [u8],
    supported_rates: &'rates [u8],
    descriptor_capacity: Option<u32>,
    select_candidate: bool,
}

impl<'ssid, 'rates> Esp32s31ScanStation<'ssid, 'rates> {
    pub const fn new(
        station_address: [u8; 6],
        target_ssid: &'ssid [u8],
        supported_rates: &'rates [u8],
    ) -> Self {
        Self {
            station_address,
            target_ssid,
            supported_rates,
            descriptor_capacity: None,
            select_candidate: true,
        }
    }

    pub const fn with_descriptor_capacity(mut self, capacity: u32) -> Self {
        self.descriptor_capacity = Some(capacity);
        self
    }

    pub const fn with_candidate_selection(mut self, enabled: bool) -> Self {
        self.select_candidate = enabled;
        self
    }
}

/// Bounded telemetry produced without retaining any DMA-backed frame.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Esp32s31ScanTelemetry {
    pub raw_frames: u32,
    pub ring_epochs: u32,
}

/// Exact primitive edge which failed inside the concrete running port.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31ScanPortError<P, R, T> {
    ChannelSwitch(P),
    Receive(R),
    Transmit(T),
}

/// Owners returned after the scan service has stopped RX.
pub struct Esp32s31ScanPortParts<'resources, 'sequence, P, H, R, T, W, O, const RECORDS: usize> {
    pub phy: P,
    pub hardware: H,
    pub rx: R,
    pub tx: T,
    pub timer: W,
    pub observer: O,
    pub table: &'resources mut ScanTable<RECORDS>,
    pub frame: &'resources mut [u8],
    pub sequence: &'sequence mut StaSequenceCounter,
    pub telemetry: Esp32s31ScanTelemetry,
}

/// Complete production running-scan port.
///
/// Board code supplies only coherent driver owners, fixed storage and station
/// policy. No PAC singleton, static address or executor task is reconstructed
/// inside this value.
pub struct Esp32s31ScanPort<
    'resources,
    'sequence,
    'ssid,
    'rates,
    P,
    H,
    R,
    T,
    W,
    O,
    const RECORDS: usize,
> {
    radio: Esp32s31ScanRadio<P, H, R, T>,
    storage: Esp32s31ScanStorage<'resources, 'sequence, O, RECORDS>,
    station: Esp32s31ScanStation<'ssid, 'rates>,
    timer: W,
    telemetry: Esp32s31ScanTelemetry,
}

mod bindings;
mod owner;
mod service;

#[cfg(test)]
mod tests;
