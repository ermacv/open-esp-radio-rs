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
//! cannot: the publication watchdog of the attempt in flight, which it turns
//! into a deadline edge of the ordinary TX owner, and the PHY retune an
//! `Enable` needs after a channel change, through [`LowerMacRetune`]. The
//! ordinary TX owner's timer must therefore read the `embassy-time` clock
//! (`EmbassyWifiTxTimer` in production).
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
use oer_esp32s31_ieee80211::{
    lower_mac::{
        ESP32S31_LOWER_MAC_CAPABILITIES, LifecycleStart, LowerMacCore, LowerMacFault,
        LowerMacHardware, LowerMacSink,
    },
    ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile, WifiTxTimer},
    tx::WifiTxWake,
};
use oer_esp32s31_ieee80211_mac::rx::NormalizedRxFrame;
use oer_ieee80211_lower_mac::{
    EventsLost, Ieee80211LowerMacPort, KeyHandle, KeyInstall, LifecycleCommand, LifecycleError,
    LifecycleEvent, LowerMacCapabilities, LowerMacEvent, LowerMacSetting, RxMeta, SettingError,
    SubmitError, Tsf, TxAttempt, TxCompletion, VifId,
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
pub struct Esp32s31LowerMacParts<'slot, P, E, T, H, R, const BUFFER_SIZE: usize> {
    pub core: LowerMacCore<'slot, P, E, T, BUFFER_SIZE>,
    pub hardware: H,
    pub retune: R,
}

struct Installed<'slot, P, E, T, H, const BUFFER_SIZE: usize> {
    core: LowerMacCore<'slot, P, E, T, BUFFER_SIZE>,
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
}

/// The ESP32-S31 lower MAC behind the portable port.
///
/// `EVENTS` bounds the events waiting for the consumer and `FRAME` the
/// bytes of one received MPDU; a longer MPDU is lost and reported as
/// [`EventsLost`].
pub struct Esp32s31LowerMac<
    'slot,
    M: RawMutex,
    P,
    E,
    T,
    H,
    R,
    const BUFFER_SIZE: usize,
    const EVENTS: usize,
    const FRAME: usize,
> {
    installed: Mutex<M, RefCell<Option<Installed<'slot, P, E, T, H, BUFFER_SIZE>>>>,
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
    const BUFFER_SIZE: usize,
    const EVENTS: usize,
    const FRAME: usize,
> Default for Esp32s31LowerMac<'slot, M, P, E, T, H, R, BUFFER_SIZE, EVENTS, FRAME>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
    H: LowerMacHardware,
    R: LowerMacRetune,
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
    const BUFFER_SIZE: usize,
    const EVENTS: usize,
    const FRAME: usize,
> Esp32s31LowerMac<'slot, M, P, E, T, H, R, BUFFER_SIZE, EVENTS, FRAME>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
    H: LowerMacHardware,
    R: LowerMacRetune,
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
        parts: Esp32s31LowerMacParts<'slot, P, E, T, H, R, BUFFER_SIZE>,
    ) -> Result<(), Esp32s31LowerMacParts<'slot, P, E, T, H, R, BUFFER_SIZE>> {
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
    pub fn uninstall(&self) -> Option<(LowerMacCore<'slot, P, E, T, BUFFER_SIZE>, H)> {
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
            &mut LowerMacCore<'slot, P, E, T, BUFFER_SIZE>,
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

    /// One MPDU of the receive producer, copied into the queue while the
    /// port receives.
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
                let _ = self.with_core(|core, _, sink| core.finish_retune(retuned, sink));
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

impl<M: RawMutex, P, E, T, H, R, const BUFFER_SIZE: usize, const EVENTS: usize, const FRAME: usize>
    Ieee80211LowerMacPort for Esp32s31LowerMac<'_, M, P, E, T, H, R, BUFFER_SIZE, EVENTS, FRAME>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
    H: LowerMacHardware,
    R: LowerMacRetune,
{
    type Event = Esp32s31LowerMacEvent<FRAME>;
    type Error = Esp32s31LowerMacError;

    fn view(event: &Esp32s31LowerMacEvent<FRAME>) -> LowerMacEvent<'_> {
        event.portable()
    }

    fn capabilities(&self) -> LowerMacCapabilities {
        ESP32S31_LOWER_MAC_CAPABILITIES
    }

    fn submit(
        &self,
        attempt: TxAttempt<'_>,
    ) -> Result<Result<(), SubmitError>, Esp32s31LowerMacError> {
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
        // Releasing a power-save hold publishes the held attempt.
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

    fn tsf(&self, vif: VifId) -> Result<Result<Tsf, SettingError>, Esp32s31LowerMacError> {
        self.with_core(|core, hardware, _| Ok(core.tsf(hardware, vif)))
    }
}

#[cfg(all(test, not(target_pointer_width = "32")))]
mod tests;
