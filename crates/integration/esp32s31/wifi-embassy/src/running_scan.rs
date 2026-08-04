//! Concrete running-scan composition for a quiesced connected STA epoch.
//!
//! The chip-independent lifecycle service owns plan progress and retry policy;
//! [`Esp32s31StaScanBackend`](crate::sta_scan::Esp32s31StaScanBackend) owns the
//! mandatory ESP32-S31 transaction order. This module binds that transaction
//! to the returned PHY, cooperative register owner, RX ring and control-TX
//! descriptor without importing board fixtures, credentials or diagnostics.

use core::future::Future;

use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_time::Timer;
use open_esp_radio_esp32s31_wifi_mac::{rx::RxDma, tx::TxHardware};
use open_esp_radio_ieee80211::{
    scan::{ScanRecord, ScanTable, best_matching_ssid},
    station::StaSequenceCounter,
};
use open_esp_radio_wifi_lifecycle::scan::StaScanChannelContext;

use crate::{
    control_tx::ControlTxError,
    embassy_rx::RxReloadDelay,
    ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile, WifiTxTimer},
    sta_scan::{
        Esp32s31ActiveProbeOutcome, Esp32s31RunningScanRx, Esp32s31RunningScanTx,
        Esp32s31ScanFrameObserver, Esp32s31ScanObservationContext, Esp32s31ScanProbeReport,
        Esp32s31ScanProbeRequest, Esp32s31ScanRxError, Esp32s31ScanRxProgress, Esp32s31StaScanPort,
    },
};

/// PHY channel-switch capability required by a running scan.
pub trait Esp32s31RunningScanPhy<H> {
    type Error;

    fn switch_channel<'a>(
        &'a mut self,
        hardware: &'a mut H,
        channel: u8,
    ) -> impl Future<Output = Result<(), Self::Error>> + 'a;
}

/// RX-ring capability retained across every finite running-scan channel.
pub trait Esp32s31RunningScanReceive<H> {
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
pub trait Esp32s31RunningScanTransmit<H> {
    type Error;

    fn begin_scan(&mut self);

    fn transmit_probe_request<'a>(
        &'a mut self,
        hardware: &'a mut H,
        request: Esp32s31ScanProbeRequest<'a>,
    ) -> impl Future<Output = Result<Esp32s31ScanProbeReport, Self::Error>> + 'a;
}

/// Executor clock edge for one scan dwell tick.
pub trait Esp32s31RunningScanTimer {
    fn wait_dwell_tick(&mut self) -> impl Future<Output = ()> + '_;
}

/// Production one-millisecond Embassy dwell tick.
#[derive(Clone, Copy, Debug, Default)]
pub struct EmbassyEsp32s31RunningScanTimer;

impl Esp32s31RunningScanTimer for EmbassyEsp32s31RunningScanTimer {
    fn wait_dwell_tick(&mut self) -> impl Future<Output = ()> + '_ {
        Timer::after_millis(1)
    }
}

/// Driver resources returned by one completely quiesced connected epoch.
pub struct Esp32s31RunningScanRadio<P, H, R, T> {
    phy: P,
    hardware: H,
    rx: R,
    tx: T,
}

impl<P, H, R, T> Esp32s31RunningScanRadio<P, H, R, T> {
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
pub struct Esp32s31RunningScanStorage<'resources, 'sequence, O, const RECORDS: usize> {
    table: &'resources mut ScanTable<RECORDS>,
    frame: &'resources mut [u8],
    observer: O,
    sequence: &'sequence mut StaSequenceCounter,
}

impl<'resources, 'sequence, O, const RECORDS: usize>
    Esp32s31RunningScanStorage<'resources, 'sequence, O, RECORDS>
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
pub struct Esp32s31RunningScanStation<'ssid, 'rates> {
    station_address: [u8; 6],
    target_ssid: &'ssid [u8],
    supported_rates: &'rates [u8],
    descriptor_capacity: Option<u32>,
}

impl<'ssid, 'rates> Esp32s31RunningScanStation<'ssid, 'rates> {
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
        }
    }

    pub const fn with_descriptor_capacity(mut self, capacity: u32) -> Self {
        self.descriptor_capacity = Some(capacity);
        self
    }
}

/// Bounded telemetry produced without retaining any DMA-backed frame.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Esp32s31RunningScanTelemetry {
    pub raw_frames: u32,
    pub ring_epochs: u32,
}

/// Exact primitive edge which failed inside the concrete running port.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31RunningScanPortError<P, R, T> {
    ChannelSwitch(P),
    Receive(R),
    Transmit(T),
}

/// Owners returned after the scan service has stopped RX.
pub struct Esp32s31RunningScanParts<'resources, 'sequence, P, H, R, T, W, O, const RECORDS: usize> {
    pub phy: P,
    pub hardware: H,
    pub rx: R,
    pub tx: T,
    pub timer: W,
    pub observer: O,
    pub table: &'resources mut ScanTable<RECORDS>,
    pub frame: &'resources mut [u8],
    pub sequence: &'sequence mut StaSequenceCounter,
    pub telemetry: Esp32s31RunningScanTelemetry,
}

/// Complete production running-scan port.
///
/// Board code supplies only coherent driver owners, fixed storage and station
/// policy. No PAC singleton, static address or executor task is reconstructed
/// inside this value.
pub struct Esp32s31RunningScanPort<
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
    radio: Esp32s31RunningScanRadio<P, H, R, T>,
    storage: Esp32s31RunningScanStorage<'resources, 'sequence, O, RECORDS>,
    station: Esp32s31RunningScanStation<'ssid, 'rates>,
    timer: W,
    telemetry: Esp32s31RunningScanTelemetry,
}

impl<'resources, 'sequence, 'ssid, 'rates, P, H, R, T, W, O, const RECORDS: usize>
    Esp32s31RunningScanPort<'resources, 'sequence, 'ssid, 'rates, P, H, R, T, W, O, RECORDS>
{
    pub const fn new(
        radio: Esp32s31RunningScanRadio<P, H, R, T>,
        storage: Esp32s31RunningScanStorage<'resources, 'sequence, O, RECORDS>,
        station: Esp32s31RunningScanStation<'ssid, 'rates>,
        timer: W,
    ) -> Self {
        Self {
            radio,
            storage,
            station,
            timer,
            telemetry: Esp32s31RunningScanTelemetry {
                raw_frames: 0,
                ring_epochs: 0,
            },
        }
    }

    pub fn into_parts(
        self,
    ) -> Esp32s31RunningScanParts<'resources, 'sequence, P, H, R, T, W, O, RECORDS> {
        let Self {
            radio,
            storage,
            station: _,
            timer,
            telemetry,
        } = self;
        let Esp32s31RunningScanRadio {
            phy,
            hardware,
            rx,
            tx,
        } = radio;
        let Esp32s31RunningScanStorage {
            table,
            frame,
            observer,
            sequence,
        } = storage;
        Esp32s31RunningScanParts {
            phy,
            hardware,
            rx,
            tx,
            timer,
            observer,
            table,
            frame,
            sequence,
            telemetry,
        }
    }

    fn observe_scan_rx(
        &mut self,
        channel: u8,
    ) -> Result<(), Esp32s31RunningScanPortError<P::Error, R::Error, T::Error>>
    where
        P: Esp32s31RunningScanPhy<H>,
        R: Esp32s31RunningScanReceive<H>,
        T: Esp32s31RunningScanTransmit<H>,
        O: Esp32s31ScanFrameObserver,
    {
        let mut context = Esp32s31ScanObservationContext::new(
            channel,
            self.storage.frame,
            self.storage.table,
            &mut self.storage.observer,
        );
        let progress = self
            .radio
            .rx
            .observe_management(&mut self.radio.hardware, &mut context)
            .map_err(Esp32s31RunningScanPortError::Receive)?;
        self.telemetry.raw_frames = self
            .telemetry
            .raw_frames
            .saturating_add(progress.completed_descriptors);
        if progress.recycled_descriptors != 0 {
            self.telemetry.ring_epochs = self.telemetry.ring_epochs.saturating_add(1);
        }
        Ok(())
    }
}

impl<'resources, 'sequence, 'ssid, 'rates, P, H, R, T, W, O, const RECORDS: usize>
    Esp32s31StaScanPort
    for Esp32s31RunningScanPort<'resources, 'sequence, 'ssid, 'rates, P, H, R, T, W, O, RECORDS>
where
    P: Esp32s31RunningScanPhy<H>,
    R: Esp32s31RunningScanReceive<H>,
    T: Esp32s31RunningScanTransmit<H>,
    W: Esp32s31RunningScanTimer,
    O: Esp32s31ScanFrameObserver,
{
    type Channel = u8;
    type Candidate = ScanRecord;
    type Error = Esp32s31RunningScanPortError<P::Error, R::Error, T::Error>;

    fn begin_scan(&mut self) -> impl Future<Output = Result<(), Self::Error>> + '_ {
        async {
            self.storage.table.clear();
            self.telemetry = Esp32s31RunningScanTelemetry::default();
            self.radio.tx.begin_scan();
            self.radio
                .rx
                .prepare_initial(&mut self.radio.hardware)
                .map_err(Esp32s31RunningScanPortError::Receive)
        }
    }

    fn switch_channel(
        &mut self,
        context: StaScanChannelContext<Self::Channel>,
    ) -> impl Future<Output = Result<(), Self::Error>> + '_ {
        async move {
            self.radio
                .phy
                .switch_channel(&mut self.radio.hardware, context.channel)
                .await
                .map_err(Esp32s31RunningScanPortError::ChannelSwitch)
        }
    }

    fn start_receive(
        &mut self,
        _context: StaScanChannelContext<Self::Channel>,
    ) -> impl Future<Output = Result<(), Self::Error>> + '_ {
        async move {
            self.radio
                .rx
                .start(&mut self.radio.hardware)
                .await
                .map_err(Esp32s31RunningScanPortError::Receive)
        }
    }

    fn transmit_active_probe(
        &mut self,
        context: StaScanChannelContext<Self::Channel>,
    ) -> impl Future<Output = Result<Esp32s31ActiveProbeOutcome, Self::Error>> + '_ {
        let request = Esp32s31ScanProbeRequest {
            source: self.station.station_address,
            sequence_number: self.storage.sequence.take(),
            ssid: b"",
            supported_rates: self.station.supported_rates,
            current_channel: Some(context.channel),
            descriptor_capacity: self.station.descriptor_capacity,
        };
        async move {
            self.radio
                .tx
                .transmit_probe_request(&mut self.radio.hardware, request)
                .await
                .map(Esp32s31ScanProbeReport::outcome)
                .map_err(Esp32s31RunningScanPortError::Transmit)
        }
    }

    fn observe_receive(
        &mut self,
        context: StaScanChannelContext<Self::Channel>,
    ) -> Result<(), Self::Error> {
        self.observe_scan_rx(context.channel)
    }

    fn wait_dwell_tick(&mut self) -> impl Future<Output = Result<(), Self::Error>> + '_ {
        async {
            self.timer.wait_dwell_tick().await;
            Ok(())
        }
    }

    fn stop_receive(
        &mut self,
        context: StaScanChannelContext<Self::Channel>,
    ) -> Result<(), Self::Error> {
        let observe_error = self.observe_scan_rx(context.channel).err();
        self.radio
            .rx
            .stop(&mut self.radio.hardware)
            .map_err(Esp32s31RunningScanPortError::Receive)?;
        if let Some(error) = observe_error {
            return Err(error);
        }
        Ok(())
    }

    fn prepare_next_ring(
        &mut self,
        _context: StaScanChannelContext<Self::Channel>,
    ) -> Result<(), Self::Error> {
        self.radio
            .rx
            .prepare_next(&mut self.radio.hardware)
            .map_err(Esp32s31RunningScanPortError::Receive)
    }

    fn select_candidate(&mut self) -> Result<Option<Self::Candidate>, Self::Error> {
        Ok(best_matching_ssid(self.storage.table.records(), self.station.target_ssid).copied())
    }
}

impl<
    'storage,
    'pool,
    'queue,
    D,
    M: RawMutex,
    H,
    const QUEUE_DEPTH: usize,
    const COUNT: usize,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> Esp32s31RunningScanReceive<H>
    for Esp32s31RunningScanRx<
        'storage,
        'pool,
        'queue,
        D,
        M,
        QUEUE_DEPTH,
        COUNT,
        STAGE_CAPACITY,
        STAGE_SLOTS,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
    >
where
    D: RxReloadDelay,
    H: RxDma,
{
    type Error = Esp32s31ScanRxError;

    fn prepare_initial(&mut self, hardware: &mut H) -> Result<(), Self::Error> {
        Self::prepare_initial(self, hardware)
    }

    fn start<'a>(
        &'a mut self,
        hardware: &'a mut H,
    ) -> impl Future<Output = Result<(), Self::Error>> + 'a {
        Self::start(self, hardware)
    }

    fn observe_management<O, const RECORDS: usize>(
        &mut self,
        hardware: &mut H,
        context: &mut Esp32s31ScanObservationContext<'_, O, RECORDS>,
    ) -> Result<Esp32s31ScanRxProgress, Self::Error>
    where
        O: Esp32s31ScanFrameObserver,
    {
        Self::observe_management(self, hardware, context)
    }

    fn stop(&mut self, hardware: &mut H) -> Result<(), Self::Error> {
        Self::stop(self, hardware)
    }

    fn prepare_next(&mut self, hardware: &mut H) -> Result<(), Self::Error> {
        Self::prepare_next(self, hardware)
    }
}

impl<'slot, 'interrupt, P, E, W, H, const BUFFER_SIZE: usize> Esp32s31RunningScanTransmit<H>
    for Esp32s31RunningScanTx<'slot, 'interrupt, P, E, W, BUFFER_SIZE>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    W: WifiTxTimer,
    H: TxHardware,
{
    type Error = ControlTxError;

    fn begin_scan(&mut self) {
        Self::begin_scan(self);
    }

    fn transmit_probe_request<'a>(
        &'a mut self,
        hardware: &'a mut H,
        request: Esp32s31ScanProbeRequest<'a>,
    ) -> impl Future<Output = Result<Esp32s31ScanProbeReport, Self::Error>> + 'a {
        Self::transmit_probe_request(self, hardware, request)
    }
}

#[cfg(test)]
mod tests {
    use core::{
        future::{Future, ready},
        pin::pin,
        task::{Context, Poll},
    };
    use std::vec::Vec;

    use open_esp_radio_ieee80211::scan::ScanObservation;
    use open_esp_radio_wifi_lifecycle::scan::{StaCandidateScanExit, StaCandidateScanService};

    use super::*;
    use crate::sta_scan::{Esp32s31StaScanBackend, Esp32s31StaScanConfig};

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Action {
        Prepare,
        Switch(u8),
        Start,
        Probe(u8),
        Observe,
        Stop,
    }

    #[derive(Default)]
    struct Hardware {
        actions: Vec<Action>,
    }

    struct Phy(u32);

    impl Esp32s31RunningScanPhy<Hardware> for Phy {
        type Error = ();

        fn switch_channel<'a>(
            &'a mut self,
            hardware: &'a mut Hardware,
            channel: u8,
        ) -> impl Future<Output = Result<(), Self::Error>> + 'a {
            hardware.actions.push(Action::Switch(channel));
            ready(Ok(()))
        }
    }

    struct Receive(u32);

    impl Esp32s31RunningScanReceive<Hardware> for Receive {
        type Error = ();

        fn prepare_initial(&mut self, hardware: &mut Hardware) -> Result<(), Self::Error> {
            hardware.actions.push(Action::Prepare);
            Ok(())
        }

        fn start<'a>(
            &'a mut self,
            hardware: &'a mut Hardware,
        ) -> impl Future<Output = Result<(), Self::Error>> + 'a {
            hardware.actions.push(Action::Start);
            ready(Ok(()))
        }

        fn observe_management<O, const RECORDS: usize>(
            &mut self,
            hardware: &mut Hardware,
            context: &mut Esp32s31ScanObservationContext<'_, O, RECORDS>,
        ) -> Result<Esp32s31ScanRxProgress, Self::Error>
        where
            O: Esp32s31ScanFrameObserver,
        {
            hardware.actions.push(Action::Observe);
            let mut beacon = [0_u8; 47];
            beacon[0] = 0x80;
            beacon[16..22].copy_from_slice(&[1, 2, 3, 4, 5, 6]);
            beacon[36..42].copy_from_slice(&[0, 4, b't', b'e', b's', b't']);
            beacon[42..45].copy_from_slice(&[3, 1, 11]);
            beacon[45..47].copy_from_slice(&[48, 0]);
            assert_ne!(
                context.observe_management_frame(&beacon, -30),
                ScanObservation::Ignored
            );
            Ok(Esp32s31ScanRxProgress {
                completed_descriptors: 1,
                parsed_management_frames: 1,
                recycled_descriptors: 1,
                ..Esp32s31ScanRxProgress::default()
            })
        }

        fn stop(&mut self, hardware: &mut Hardware) -> Result<(), Self::Error> {
            hardware.actions.push(Action::Stop);
            Ok(())
        }

        fn prepare_next(&mut self, hardware: &mut Hardware) -> Result<(), Self::Error> {
            hardware.actions.push(Action::Prepare);
            Ok(())
        }
    }

    struct Transmit(u32);

    impl Esp32s31RunningScanTransmit<Hardware> for Transmit {
        type Error = ();

        fn begin_scan(&mut self) {}

        fn transmit_probe_request<'a>(
            &'a mut self,
            hardware: &'a mut Hardware,
            request: Esp32s31ScanProbeRequest<'a>,
        ) -> impl Future<Output = Result<Esp32s31ScanProbeReport, Self::Error>> + 'a {
            hardware
                .actions
                .push(Action::Probe(request.current_channel.unwrap()));
            ready(Ok(Esp32s31ScanProbeReport::PassiveWithoutAttempt))
        }
    }

    #[derive(Default)]
    struct DwellTimer(u32);

    impl Esp32s31RunningScanTimer for DwellTimer {
        fn wait_dwell_tick(&mut self) -> impl Future<Output = ()> + '_ {
            self.0 += 1;
            ready(())
        }
    }

    #[derive(Default)]
    struct Observer(u32);

    impl Esp32s31ScanFrameObserver for Observer {
        fn observe(&mut self, _frame: &[u8], _rssi: i8, _outcome: ScanObservation) {
            self.0 += 1;
        }
    }

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

    #[test]
    fn concrete_port_returns_every_owner_after_selected_candidate() {
        let mut table = ScanTable::<4>::new();
        let mut frame = [0_u8; 128];
        let mut sequence = StaSequenceCounter::new(7);
        let port = Esp32s31RunningScanPort::new(
            Esp32s31RunningScanRadio::new(Phy(11), Hardware::default(), Receive(22), Transmit(33)),
            Esp32s31RunningScanStorage::new(
                &mut table,
                &mut frame,
                Observer::default(),
                &mut sequence,
            ),
            Esp32s31RunningScanStation::new([7; 6], b"test", &[0x82, 0x84])
                .with_descriptor_capacity(88),
            DwellTimer::default(),
        );
        let backend = Esp32s31StaScanBackend::new(Esp32s31StaScanConfig::new(2).unwrap());
        let mut service = StaCandidateScanService::new(backend);
        let exit = block_on(service.run(port, &[11]));
        let (owner, candidate) = match exit {
            StaCandidateScanExit::Selected {
                owner, candidate, ..
            } => (owner, candidate),
            StaCandidateScanExit::NoCandidate { .. } => {
                panic!("matching beacon must select one candidate")
            }
            StaCandidateScanExit::Failed { error, .. } => {
                panic!("running scan failed: {error:?}")
            }
            StaCandidateScanExit::Stopped { .. } => panic!("running scan stopped"),
            StaCandidateScanExit::InvalidPlan { error, .. } => {
                panic!("running scan plan failed: {error:?}")
            }
        };
        assert_eq!(candidate.channel, 11);

        let parts = owner.into_parts();
        assert_eq!(parts.phy.0, 11);
        assert_eq!(parts.rx.0, 22);
        assert_eq!(parts.tx.0, 33);
        assert_eq!(parts.timer.0, 2);
        assert_eq!(parts.observer.0, 3);
        assert_eq!(
            parts.telemetry,
            Esp32s31RunningScanTelemetry {
                raw_frames: 3,
                ring_epochs: 3,
            }
        );
        assert_eq!(
            parts.hardware.actions,
            [
                Action::Prepare,
                Action::Switch(11),
                Action::Start,
                Action::Probe(11),
                Action::Observe,
                Action::Observe,
                Action::Observe,
                Action::Stop,
            ]
        );
        assert_eq!(parts.sequence.peek(), 8);
        assert_eq!(parts.table.summary().records, 1);
        assert_eq!(parts.frame.len(), 128);
    }
}
