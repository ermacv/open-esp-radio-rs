//! The ESP32-S31 Wi-Fi backend as an `Ieee80211LowerMacPort`.
//!
//! [`Esp32s31LowerMac`] holds the sans-IO
//! [`LowerMacCore`] of `oer-esp32s31-ieee80211` and its register owner in one
//! blocking mutex over a caller-chosen raw mutex, as the IEEE 802.15.4
//! runtime does. The MAC interrupt handler ([`Esp32s31LowerMac::on_interrupt`]),
//! the receive producer ([`Esp32s31LowerMac::on_received`]) and every port
//! call run inside that lock; events leave it as owned values through a
//! bounded queue that any executor may await. Overflow drops the newest
//! event and is reported once as [`EventsLost`].
//!
//! [`Ieee80211LowerMacPort::next_event`] also runs the two waits the core
//! cannot: the publication watchdog of the attempts in flight, which it turns
//! into a deadline edge of the ordinary TX owner, and the PHY retune an
//! `Enable` needs after a channel change, through [`LowerMacRetune`]. The
//! ordinary TX owner's timer must therefore read the `embassy-time` clock
//! (`EmbassyWifiTxTimer` in production).
//!
//! Besides the base port it implements the extensions the ESP32-S31 has:
//! [`LowerMacAmpdu`] (HT aggregates, when the core was built with aggregate
//! owners over an [`AmpduBackingSource`]), [`LowerMacBeaconTiming`] (the
//! station TSF and TBTT schedule, whose events the power interrupt delivers
//! through [`Esp32s31LowerMac::on_power_interrupt`], and the access-point TSF
//! restart) and [`LowerMacMonitor`].
//!
//! Each of the four ordinary EDCA queues holds one attempt; completions leave
//! the queue in whatever order the queues end, correlated by `TxId`. The MAC
//! interrupt entry offers each edge to every published queue, and the
//! watchdog waits for the earliest deadline of any of them.
//!
//! This is an additional entry point: the station and access-point roles do
//! not drive the backend through it yet.

use core::{
    cell::{Cell, RefCell},
    sync::atomic::{AtomicBool, Ordering},
};

use embassy_futures::select::{Either3, select3};
use embassy_sync::{
    blocking_mutex::{Mutex, raw::RawMutex},
    channel::Channel,
    signal::Signal,
};
use embassy_time::{Instant, Timer};
use oer_esp32s31_hal::types::MacPowerInterruptObservation;
use oer_esp32s31_ieee80211::{
    lower_mac::{
        AmpduBacking, AmpduBackingSource, ESP32S31_BEACON_TIMING_CAPABILITIES,
        ESP32S31_MONITOR_CAPABILITIES, Esp32s31AmpduAttempt, Esp32s31AmpduBuffer,
        Esp32s31MpduAttempt, Esp32s31TxBuffer, LifecycleStart, LowerMacCore, LowerMacFault,
        LowerMacHardware, LowerMacSink, NoAmpdu, esp32s31_ampdu_capabilities,
        esp32s31_lower_mac_capabilities,
    },
    ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile, WifiTxTimer},
    tx::WifiTxWake,
};
use oer_esp32s31_ieee80211_mac::rx::NormalizedRxFrame;
use oer_ieee80211_lower_mac::{
    AmpduCapabilities, BeaconTimingCapabilities, EventsLost, Ieee80211LowerMacPort, KeyHandle,
    KeyInstall, LifecycleCommand, LifecycleError, LifecycleEvent, LowerMacAmpdu,
    LowerMacBeaconTiming, LowerMacCapabilities, LowerMacEvent, LowerMacMonitor, LowerMacSetting,
    MonitorCapabilities, RxMeta, SettingError, SubmitResult, TbttEvent, TbttSchedule, Tsf,
    TxCompletion, VifId,
};
use oer_ieee80211_mac::channel::WifiChannel;
use oer_time::RadioInstant;

/// The PHY retune of an `Enable` after a channel change.
///
/// The ESP32-S31 retunes through `switch_esp32s31_wifi_channel` under the
/// shared-radio lease while the MAC's DMA and interrupt service are stopped,
/// which a disabled port guarantees.
pub trait LowerMacRetune {
    /// Tune to `channel`; `false` when the PHY refused.
    fn retune(&mut self, channel: WifiChannel) -> impl Future<Output = bool>;
}

/// One owned event of the port.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Esp32s31LowerMacEvent<const FRAME: usize> {
    /// A received MPDU, its first `length` bytes in `frame`.
    Received {
        frame: [u8; FRAME],
        length: usize,
        meta: RxMeta,
    },
    TxCompleted(TxCompletion),
    Lifecycle(LifecycleEvent),
    /// A station TBTT, viewed through [`LowerMacBeaconTiming::tbtt`].
    Tbtt(TbttEvent),
}

impl<const FRAME: usize> Esp32s31LowerMacEvent<FRAME> {
    /// Lend the event as a portable value.
    pub fn portable(&self) -> LowerMacEvent<'_> {
        match self {
            Self::Received {
                frame,
                length,
                meta,
            } => LowerMacEvent::Received {
                frame: &frame[..*length],
                meta: *meta,
            },
            Self::TxCompleted(completion) => LowerMacEvent::TxCompleted(*completion),
            Self::Lifecycle(event) => LowerMacEvent::Lifecycle(*event),
            Self::Tbtt(_) => LowerMacEvent::Extension,
        }
    }
}

/// Why the port cannot serve.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31LowerMacError {
    /// No backend is installed.
    NotInstalled,
    /// The backend's state is unknown; only a radio reset restores it.
    Poisoned(LowerMacFault),
}

/// The core, its register owner and the retune of `Enable`.
pub struct Esp32s31LowerMacParts<
    'slot,
    P,
    E,
    T,
    H,
    R,
    const BUFFER_SIZE: usize,
    const TX_BUFFERS: usize,
    S: AmpduBacking = NoAmpdu,
    const AMPDU_SLOTS: usize = 2,
    const AMPDU_BUFFERS: usize = 0,
> {
    pub core: LowerMacCore<'slot, P, E, T, BUFFER_SIZE, TX_BUFFERS, S, AMPDU_SLOTS, AMPDU_BUFFERS>,
    pub hardware: H,
    pub retune: R,
}

struct Installed<
    'slot,
    P,
    E,
    T,
    H,
    S: AmpduBacking,
    const BUFFER_SIZE: usize,
    const TX_BUFFERS: usize,
    const AMPDU_SLOTS: usize,
    const AMPDU_BUFFERS: usize,
> {
    core: LowerMacCore<'slot, P, E, T, BUFFER_SIZE, TX_BUFFERS, S, AMPDU_SLOTS, AMPDU_BUFFERS>,
    hardware: H,
}

/// The sink of one locked entry: events go to the queue.
struct QueueSink<'a, M: RawMutex, const EVENTS: usize, const FRAME: usize> {
    events: &'a Channel<M, Esp32s31LowerMacEvent<FRAME>, EVENTS>,
    lost: &'a AtomicBool,
}

impl<M: RawMutex, const EVENTS: usize, const FRAME: usize> QueueSink<'_, M, EVENTS, FRAME> {
    fn push(&mut self, event: Esp32s31LowerMacEvent<FRAME>) {
        if self.events.try_send(event).is_err() {
            self.lost.store(true, Ordering::Release);
        }
    }
}

impl<M: RawMutex, const EVENTS: usize, const FRAME: usize> LowerMacSink
    for QueueSink<'_, M, EVENTS, FRAME>
{
    fn tx_completed(&mut self, completion: TxCompletion) {
        self.push(Esp32s31LowerMacEvent::TxCompleted(completion));
    }

    fn lifecycle(&mut self, event: LifecycleEvent) {
        self.push(Esp32s31LowerMacEvent::Lifecycle(event));
    }

    fn tbtt(&mut self, event: TbttEvent) {
        self.push(Esp32s31LowerMacEvent::Tbtt(event));
    }
}

/// The ESP32-S31 lower MAC behind the portable port.
///
/// `TX_BUFFERS` counts the transmit buffers the core lends, `EVENTS` bounds
/// the events waiting for the consumer and `FRAME` the bytes of one
/// received MPDU; a longer MPDU is lost and reported as [`EventsLost`].
/// `S`, `AMPDU_SLOTS` and `AMPDU_BUFFERS` are the core's aggregate memory,
/// subframes per aggregate and aggregate owners; with the default
/// [`NoAmpdu`] the port has no aggregates and no [`LowerMacAmpdu`].
pub struct Esp32s31LowerMac<
    'slot,
    M: RawMutex,
    P,
    E,
    T,
    H,
    R,
    const BUFFER_SIZE: usize,
    const TX_BUFFERS: usize,
    const EVENTS: usize,
    const FRAME: usize,
    S: AmpduBacking = NoAmpdu,
    const AMPDU_SLOTS: usize = 2,
    const AMPDU_BUFFERS: usize = 0,
> {
    #[allow(
        clippy::type_complexity,
        reason = "the installed owners keep their types"
    )]
    installed: Mutex<
        M,
        RefCell<
            Option<
                Installed<
                    'slot,
                    P,
                    E,
                    T,
                    H,
                    S,
                    BUFFER_SIZE,
                    TX_BUFFERS,
                    AMPDU_SLOTS,
                    AMPDU_BUFFERS,
                >,
            >,
        >,
    >,
    retune: Mutex<M, RefCell<Option<R>>>,
    /// The channel an admitted `Enable` waits to be tuned to.
    pending_retune: Mutex<M, Cell<Option<WifiChannel>>>,
    fault: Mutex<M, Cell<Option<LowerMacFault>>>,
    events: Channel<M, Esp32s31LowerMacEvent<FRAME>, EVENTS>,
    lost: AtomicBool,
    /// Raised when a deadline or a retune may have started, so an awaiting
    /// consumer rearms.
    wake: Signal<M, ()>,
}

impl<
    'slot,
    M: RawMutex,
    P,
    E,
    T,
    H,
    R,
    S,
    const BUFFER_SIZE: usize,
    const TX_BUFFERS: usize,
    const EVENTS: usize,
    const FRAME: usize,
    const AMPDU_SLOTS: usize,
    const AMPDU_BUFFERS: usize,
> Default
    for Esp32s31LowerMac<
        'slot,
        M,
        P,
        E,
        T,
        H,
        R,
        BUFFER_SIZE,
        TX_BUFFERS,
        EVENTS,
        FRAME,
        S,
        AMPDU_SLOTS,
        AMPDU_BUFFERS,
    >
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
    H: LowerMacHardware,
    R: LowerMacRetune,
    S: AmpduBacking,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<
    'slot,
    M: RawMutex,
    P,
    E,
    T,
    H,
    R,
    S,
    const BUFFER_SIZE: usize,
    const TX_BUFFERS: usize,
    const EVENTS: usize,
    const FRAME: usize,
    const AMPDU_SLOTS: usize,
    const AMPDU_BUFFERS: usize,
>
    Esp32s31LowerMac<
        'slot,
        M,
        P,
        E,
        T,
        H,
        R,
        BUFFER_SIZE,
        TX_BUFFERS,
        EVENTS,
        FRAME,
        S,
        AMPDU_SLOTS,
        AMPDU_BUFFERS,
    >
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
    H: LowerMacHardware,
    R: LowerMacRetune,
    S: AmpduBacking,
{
    /// An empty port, suitable for a `static`.
    pub const fn new() -> Self {
        Self {
            installed: Mutex::new(RefCell::new(None)),
            retune: Mutex::new(RefCell::new(None)),
            pending_retune: Mutex::new(Cell::new(None)),
            fault: Mutex::new(Cell::new(None)),
            events: Channel::new(),
            lost: AtomicBool::new(false),
            wake: Signal::new(),
        }
    }

    /// Install a disabled core with its register owner and retune.
    ///
    /// # Errors
    ///
    /// Returns the parts unchanged if a backend is already installed.
    #[allow(
        clippy::result_large_err,
        reason = "the no-alloc runtime returns the unconsumed owners by value"
    )]
    pub fn install(
        &self,
        parts: Esp32s31LowerMacParts<
            'slot,
            P,
            E,
            T,
            H,
            R,
            BUFFER_SIZE,
            TX_BUFFERS,
            S,
            AMPDU_SLOTS,
            AMPDU_BUFFERS,
        >,
    ) -> Result<
        (),
        Esp32s31LowerMacParts<
            'slot,
            P,
            E,
            T,
            H,
            R,
            BUFFER_SIZE,
            TX_BUFFERS,
            S,
            AMPDU_SLOTS,
            AMPDU_BUFFERS,
        >,
    > {
        let Esp32s31LowerMacParts {
            core,
            hardware,
            retune,
        } = parts;
        let refused = self.installed.lock(|installed| {
            let mut installed = installed.borrow_mut();
            if installed.is_some() {
                return Some((core, hardware));
            }
            *installed = Some(Installed { core, hardware });
            None
        });
        if let Some((core, hardware)) = refused {
            return Err(Esp32s31LowerMacParts {
                core,
                hardware,
                retune,
            });
        }
        self.retune.lock(|slot| *slot.borrow_mut() = Some(retune));
        self.fault.lock(|fault| fault.set(None));
        Ok(())
    }

    /// Take the core and its register owner back and discard queued events.
    /// The retune stays while an `Enable` awaits it.
    #[allow(
        clippy::type_complexity,
        reason = "the uninstalled owners are returned as they were installed"
    )]
    pub fn uninstall(
        &self,
    ) -> Option<(
        LowerMacCore<'slot, P, E, T, BUFFER_SIZE, TX_BUFFERS, S, AMPDU_SLOTS, AMPDU_BUFFERS>,
        H,
    )> {
        let installed = self
            .installed
            .lock(|installed| installed.borrow_mut().take());
        while self.events.try_receive().is_ok() {}
        self.lost.store(false, Ordering::Release);
        self.pending_retune.lock(|pending| pending.set(None));
        installed.map(|installed| (installed.core, installed.hardware))
    }

    /// Run one entry under the lock; a fault poisons the port.
    fn with_core<U>(
        &self,
        entry: impl FnOnce(
            &mut LowerMacCore<
                'slot,
                P,
                E,
                T,
                BUFFER_SIZE,
                TX_BUFFERS,
                S,
                AMPDU_SLOTS,
                AMPDU_BUFFERS,
            >,
            &mut H,
            &mut QueueSink<'_, M, EVENTS, FRAME>,
        ) -> Result<U, LowerMacFault>,
    ) -> Result<U, Esp32s31LowerMacError> {
        if let Some(fault) = self.fault.lock(Cell::get) {
            return Err(Esp32s31LowerMacError::Poisoned(fault));
        }
        let result = self.installed.lock(|installed| {
            let mut installed = installed.borrow_mut();
            let installed = installed
                .as_mut()
                .ok_or(Esp32s31LowerMacError::NotInstalled)?;
            let mut sink = QueueSink {
                events: &self.events,
                lost: &self.lost,
            };
            entry(&mut installed.core, &mut installed.hardware, &mut sink)
                .map_err(Esp32s31LowerMacError::Poisoned)
        });
        if let Err(Esp32s31LowerMacError::Poisoned(fault)) = result {
            self.fault.lock(|poisoned| poisoned.set(Some(fault)));
        }
        result
    }

    /// The MAC interrupt handler's TX edges: completion, hardware timeout
    /// and collision events of `oer_esp32s31_ieee80211_mac::irq`.
    pub fn on_interrupt(&self, events: u32) {
        let _ = self.with_core(|core, hardware, sink| {
            core.service(hardware, WifiTxWake::Interrupt { events }, sink)
        });
        // A timeout starts the abort-settle deadline.
        self.wake.signal(());
    }

    /// The power interrupt's semantic causes: a station TBTT is reported as
    /// an event while its schedule runs.
    pub fn on_power_interrupt(&self, observation: MacPowerInterruptObservation) {
        if observation.sta_tbtt() {
            let _ = self.with_core(|core, hardware, sink| {
                core.station_tbtt(hardware, sink);
                Ok(())
            });
        }
    }

    /// One MPDU of the receive producer, copied into the queue while the
    /// port receives and a receive rule or monitor reception admits it.
    pub fn on_received(&self, frame: &NormalizedRxFrame<'_>) {
        let _ = self.with_core(|core, _, sink| {
            if let Some((bytes, meta)) = core.received(frame) {
                let mut owned = [0; FRAME];
                match owned.get_mut(..bytes.len()) {
                    Some(prefix) => {
                        prefix.copy_from_slice(bytes);
                        sink.push(Esp32s31LowerMacEvent::Received {
                            frame: owned,
                            length: bytes.len(),
                            meta,
                        });
                    }
                    None => sink.lost.store(true, Ordering::Release),
                }
            }
            Ok(())
        });
    }

    /// Wait for the next event, running the publication watchdog and the
    /// retune of an `Enable` meanwhile. Cancelling the wait keeps both for
    /// the next call.
    async fn wait_event(&self) -> Result<Esp32s31LowerMacEvent<FRAME>, EventsLost> {
        loop {
            if self.lost.swap(false, Ordering::AcqRel) {
                return Err(EventsLost);
            }
            if let Some(channel) = self.pending_retune.lock(Cell::take) {
                let mut guard = RetuneGuard {
                    slot: &self.retune,
                    pending: &self.pending_retune,
                    retune: self.retune.lock(|slot| slot.borrow_mut().take()),
                    channel,
                    done: false,
                };
                let retuned = match guard.retune.as_mut() {
                    Some(retune) => retune.retune(channel).await,
                    None => false,
                };
                guard.done = true;
                drop(guard);
                let _ = self.with_core(|core, _, sink| {
                    core.finish_retune(retuned, sink);
                    Ok(())
                });
                continue;
            }
            let deadline = self
                .installed
                .lock(|installed| {
                    installed
                        .borrow()
                        .as_ref()
                        .map(|installed| installed.core.next_deadline_micros())
                })
                .flatten();
            let watchdog = async {
                match deadline {
                    Some(deadline) => Timer::at(Instant::from_micros(deadline)).await,
                    None => core::future::pending().await,
                }
            };
            match select3(self.events.receive(), watchdog, self.wake.wait()).await {
                Either3::First(event) => return Ok(event),
                Either3::Second(()) => {
                    let _ = self.with_core(|core, hardware, sink| {
                        core.service(hardware, WifiTxWake::Deadline, sink)
                    });
                }
                // A deadline or a retune started; rearm.
                Either3::Third(()) => {}
            }
        }
    }
}

/// Returns the retune to its slot, and the channel to the pending retune
/// when the wait was dropped before the retune finished.
struct RetuneGuard<'a, M: RawMutex, R> {
    slot: &'a Mutex<M, RefCell<Option<R>>>,
    pending: &'a Mutex<M, Cell<Option<WifiChannel>>>,
    retune: Option<R>,
    channel: WifiChannel,
    done: bool,
}

impl<M: RawMutex, R> Drop for RetuneGuard<'_, M, R> {
    fn drop(&mut self) {
        let retune = self.retune.take();
        self.slot.lock(|slot| *slot.borrow_mut() = retune);
        if !self.done {
            self.pending.lock(|pending| pending.set(Some(self.channel)));
        }
    }
}

impl<
    'slot,
    M: RawMutex,
    P,
    E,
    T,
    H,
    R,
    S,
    const BUFFER_SIZE: usize,
    const TX_BUFFERS: usize,
    const EVENTS: usize,
    const FRAME: usize,
    const AMPDU_SLOTS: usize,
    const AMPDU_BUFFERS: usize,
> Ieee80211LowerMacPort
    for Esp32s31LowerMac<
        'slot,
        M,
        P,
        E,
        T,
        H,
        R,
        BUFFER_SIZE,
        TX_BUFFERS,
        EVENTS,
        FRAME,
        S,
        AMPDU_SLOTS,
        AMPDU_BUFFERS,
    >
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
    H: LowerMacHardware,
    R: LowerMacRetune,
    S: AmpduBacking,
{
    type Event = Esp32s31LowerMacEvent<FRAME>;
    type Error = Esp32s31LowerMacError;
    type TxBuffer = Esp32s31TxBuffer<'slot, BUFFER_SIZE>;

    fn view(event: &Esp32s31LowerMacEvent<FRAME>) -> LowerMacEvent<'_> {
        event.portable()
    }

    fn capabilities(&self) -> LowerMacCapabilities {
        esp32s31_lower_mac_capabilities(BUFFER_SIZE)
    }

    /// `None` also while no backend is installed or the port is poisoned.
    fn tx_buffer(&self, len: usize) -> Option<Esp32s31TxBuffer<'slot, BUFFER_SIZE>> {
        self.with_core(|core, _, _| Ok(core.tx_buffer(len)))
            .ok()
            .flatten()
    }

    /// A buffer released while no backend is installed or the port is
    /// poisoned is lost until the radio is reset.
    fn release_tx_buffer(&self, buffer: Esp32s31TxBuffer<'slot, BUFFER_SIZE>) {
        let _ = self.with_core(|core, _, _| {
            core.release_tx_buffer(buffer);
            Ok(())
        });
    }

    fn submit(
        &self,
        attempt: Esp32s31MpduAttempt<'slot, BUFFER_SIZE>,
    ) -> SubmitResult<Esp32s31MpduAttempt<'slot, BUFFER_SIZE>, Esp32s31LowerMacError> {
        let admitted = self.with_core(|core, hardware, _| core.submit(hardware, attempt))?;
        // A publication starts its watchdog.
        self.wake.signal(());
        Ok(admitted)
    }

    async fn next_event(&self) -> Result<Esp32s31LowerMacEvent<FRAME>, EventsLost> {
        self.wait_event().await
    }

    fn apply(
        &self,
        setting: LowerMacSetting,
    ) -> Result<Result<(), SettingError>, Esp32s31LowerMacError> {
        let applied = self.with_core(|core, hardware, _| core.apply(hardware, setting))?;
        // Opening the transmit gate publishes a held attempt.
        self.wake.signal(());
        Ok(applied)
    }

    fn install_key(
        &self,
        key: KeyInstall<'_>,
    ) -> Result<Result<KeyHandle, SettingError>, Esp32s31LowerMacError> {
        self.with_core(|core, hardware, _| Ok(core.install_key(hardware, key)))
    }

    fn lifecycle(
        &self,
        command: LifecycleCommand,
    ) -> Result<Result<(), LifecycleError>, Esp32s31LowerMacError> {
        let started = self.with_core(|core, _, sink| Ok(core.lifecycle(command, sink)))?;
        match started {
            Ok(LifecycleStart::Admitted) => Ok(Ok(())),
            Ok(LifecycleStart::Retune(channel)) => {
                self.pending_retune
                    .lock(|pending| pending.set(Some(channel)));
                self.wake.signal(());
                Ok(Ok(()))
            }
            Err(error) => Ok(Err(error)),
        }
    }

    fn now(&self) -> Result<RadioInstant, Esp32s31LowerMacError> {
        self.with_core(|core, _, _| Ok(RadioInstant::from_micros(core.now_micros())))
    }
}

impl<
    M: RawMutex,
    P,
    E,
    T,
    H,
    R,
    S,
    const BUFFER_SIZE: usize,
    const TX_BUFFERS: usize,
    const EVENTS: usize,
    const FRAME: usize,
    const AMPDU_SLOTS: usize,
    const AMPDU_BUFFERS: usize,
> LowerMacBeaconTiming
    for Esp32s31LowerMac<
        '_,
        M,
        P,
        E,
        T,
        H,
        R,
        BUFFER_SIZE,
        TX_BUFFERS,
        EVENTS,
        FRAME,
        S,
        AMPDU_SLOTS,
        AMPDU_BUFFERS,
    >
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
    H: LowerMacHardware,
    R: LowerMacRetune,
    S: AmpduBacking,
{
    fn beacon_timing_capabilities(&self) -> BeaconTimingCapabilities {
        ESP32S31_BEACON_TIMING_CAPABILITIES
    }

    fn tsf(&self, vif: VifId) -> Result<Result<Tsf, SettingError>, Esp32s31LowerMacError> {
        self.with_core(|core, hardware, _| Ok(core.tsf(hardware, vif)))
    }

    fn set_tsf(
        &self,
        vif: VifId,
        tsf: Tsf,
    ) -> Result<Result<(), SettingError>, Esp32s31LowerMacError> {
        self.with_core(|core, hardware, _| Ok(core.set_tsf(hardware, vif, tsf)))
    }

    fn set_tbtt(
        &self,
        vif: VifId,
        schedule: Option<TbttSchedule>,
    ) -> Result<Result<(), SettingError>, Esp32s31LowerMacError> {
        self.with_core(|core, hardware, _| Ok(core.set_tbtt(hardware, vif, schedule)))
    }

    fn tbtt(event: &Esp32s31LowerMacEvent<FRAME>) -> Option<TbttEvent> {
        match event {
            Esp32s31LowerMacEvent::Tbtt(event) => Some(*event),
            _ => None,
        }
    }
}

impl<
    M: RawMutex,
    P,
    E,
    T,
    H,
    R,
    S,
    const BUFFER_SIZE: usize,
    const TX_BUFFERS: usize,
    const EVENTS: usize,
    const FRAME: usize,
    const AMPDU_SLOTS: usize,
    const AMPDU_BUFFERS: usize,
> LowerMacMonitor
    for Esp32s31LowerMac<
        '_,
        M,
        P,
        E,
        T,
        H,
        R,
        BUFFER_SIZE,
        TX_BUFFERS,
        EVENTS,
        FRAME,
        S,
        AMPDU_SLOTS,
        AMPDU_BUFFERS,
    >
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
    H: LowerMacHardware,
    R: LowerMacRetune,
    S: AmpduBacking,
{
    fn monitor_capabilities(&self) -> MonitorCapabilities {
        ESP32S31_MONITOR_CAPABILITIES
    }

    fn set_monitor(
        &self,
        enabled: bool,
    ) -> Result<Result<(), SettingError>, Esp32s31LowerMacError> {
        self.with_core(|core, hardware, _| Ok(core.set_monitor(hardware, enabled)))
    }
}

impl<
    'slot,
    M: RawMutex,
    P,
    E,
    T,
    H,
    R,
    S,
    const BUFFER_SIZE: usize,
    const TX_BUFFERS: usize,
    const EVENTS: usize,
    const FRAME: usize,
    const AMPDU_SLOTS: usize,
    const AMPDU_BUFFERS: usize,
> LowerMacAmpdu
    for Esp32s31LowerMac<
        'slot,
        M,
        P,
        E,
        T,
        H,
        R,
        BUFFER_SIZE,
        TX_BUFFERS,
        EVENTS,
        FRAME,
        S,
        AMPDU_SLOTS,
        AMPDU_BUFFERS,
    >
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
    H: LowerMacHardware,
    R: LowerMacRetune,
    S: AmpduBackingSource,
{
    type AmpduBuffer = Esp32s31AmpduBuffer<'slot, S, AMPDU_SLOTS>;

    fn ampdu_capabilities(&self) -> AmpduCapabilities {
        esp32s31_ampdu_capabilities(AMPDU_SLOTS)
    }

    /// `None` also while no backend is installed or the port is poisoned.
    fn ampdu_buffer(&self) -> Option<Esp32s31AmpduBuffer<'slot, S, AMPDU_SLOTS>> {
        self.with_core(|core, _, _| Ok(core.ampdu_buffer()))
            .ok()
            .flatten()
    }

    /// An aggregate released while no backend is installed or the port is
    /// poisoned returns its subframes, and its aggregate owner is lost until
    /// the radio is reset.
    fn release_ampdu_buffer(&self, buffer: Esp32s31AmpduBuffer<'slot, S, AMPDU_SLOTS>) {
        let _ = self.with_core(|core, _, _| {
            core.release_ampdu_buffer(buffer);
            Ok(())
        });
    }

    fn submit_ampdu(
        &self,
        attempt: Esp32s31AmpduAttempt<'slot, S, AMPDU_SLOTS>,
    ) -> SubmitResult<Esp32s31AmpduAttempt<'slot, S, AMPDU_SLOTS>, Esp32s31LowerMacError> {
        let admitted = self.with_core(|core, hardware, _| core.submit_ampdu(hardware, attempt))?;
        // A publication starts its watchdog.
        self.wake.signal(());
        Ok(admitted)
    }
}

#[cfg(all(test, not(target_pointer_width = "32")))]
mod tests;
