//! The two handles of an installed radio: the port of the protocol's one
//! event consumer and the composition's control.

use core::convert::Infallible;
use core::future::{Future, ready};

use embassy_sync::blocking_mutex::raw::RawMutex;
use oer_espressif_ieee802154_engine::{
    coex::Ieee802154Coexistence,
    engine::PENDING_TABLE_SIZE,
    ll::{Ieee802154LowLevel, Ieee802154RecentRssi},
    types::Ieee802154MultipanIndex,
};
use oer_ieee802154::{
    AcceptedCommand, AutoPendingMode, CancelError, ClockError, ClockInfo, CommandError, EventsLost,
    Ieee802154Capabilities, Ieee802154Instant, Ieee802154RadioPort, Interface, LifecycleCommand,
    LifecycleError, MacKeys, PendingTable, Poisoned, PortResult, RadioCommand, RadioEvent,
    RadioPort, RadioSetting, RadioState, RequestId, SettingError,
};
use oer_time::Timer;

use crate::{
    Hardware, Ieee802154PauseError, Ieee802154RadioEvent, Ieee802154Random, Ieee802154Runtime,
    Ieee802154RuntimeParts, Ieee802154RuntimePaused, Ieee802154TxRxStatistics, apply_setting,
};

/// The radio port of an installed [`Ieee802154Runtime`], for the protocol's
/// one event consumer.
///
/// [`Ieee802154Runtime::install`] returns it with the radio's
/// [`Ieee802154Control`], and [`Ieee802154Control::uninstall`] consumes
/// both, so neither meets a runtime without a radio: no call is refused as
/// not installed. It is neither `Copy` nor `Clone`. The interrupt entry and
/// the runner ([`Ieee802154Runtime::run`]) keep reaching the runtime
/// itself.
#[must_use = "an installed radio leaves only through its control's uninstall"]
pub struct Ieee802154Port<'r, R> {
    runtime: &'r R,
}

impl<'r, R> Ieee802154Port<'r, R> {
    pub(crate) const fn new(runtime: &'r R) -> Self {
        Self { runtime }
    }
}

/// The composition's handle to an installed [`Ieee802154Runtime`]:
/// shared-PHY maintenance, coexistence, statistics, the frame-pending table
/// and the uninstall. [`Ieee802154Runtime::install`] returns it with the
/// port; it is neither `Copy` nor `Clone`.
#[must_use = "an installed radio leaves only through its control's uninstall"]
pub struct Ieee802154Control<'r, R> {
    runtime: &'r R,
}

impl<'r, R> Ieee802154Control<'r, R> {
    pub(crate) const fn new(runtime: &'r R) -> Self {
        Self { runtime }
    }

    /// The runtime this control belongs to.
    pub const fn runtime(&self) -> &'r R {
        self.runtime
    }
}

impl<
    'storage,
    M: RawMutex,
    H: Ieee802154LowLevel,
    T: Timer,
    R: Ieee802154Random,
    const EVENTS: usize,
> Ieee802154Control<'_, Ieee802154Runtime<'storage, M, H, T, R, EVENTS>>
{
    /// Collect the vendor's TX/RX statistics
    /// (`CONFIG_IEEE802154_TXRX_STATISTIC`), from zero, or stop collecting
    /// them.
    pub fn set_txrx_statistics(&self, collect: bool) {
        self.runtime
            .with_installed(|radio, _, _| radio.engine().set_txrx_statistics(collect));
    }

    /// The TX/RX statistics while collected
    /// (`esp_ieee802154_txrx_statistic_print` reads them).
    pub fn txrx_statistics(&self) -> Option<Ieee802154TxRxStatistics> {
        self.runtime
            .with_installed(|radio, _, _| radio.engine().txrx_statistics())
    }

    /// `esp_ieee802154_txrx_statistic_clear`.
    pub fn clear_txrx_statistics(&self) {
        self.runtime
            .with_installed(|radio, _, _| radio.engine().clear_txrx_statistics());
    }

    /// Read or change the frame-pending table.
    pub fn with_pending_table<O>(
        &self,
        change: impl FnOnce(&mut PendingTable<PENDING_TABLE_SIZE>) -> O,
    ) -> O {
        self.runtime
            .with_installed(|radio, _, _| change(radio.engine().pending_table()))
    }

    /// `esp_ieee802154_set_pending_mode`: how the automatic acknowledgement
    /// decides frame pending. The PIB publishes it before the next operation.
    pub fn set_pending_mode(&self, mode: AutoPendingMode) {
        self.runtime.with_installed(|radio, _, _| {
            radio
                .engine()
                .pib()
                .set_pending_mode(Ieee802154MultipanIndex::CONTEXT0, mode);
        });
    }

    /// Replace the engine's software-coexistence priorities
    /// ([`Ieee802154Engine::set_coexistence`](oer_espressif_ieee802154_engine::engine::Ieee802154Engine::set_coexistence)).
    pub fn set_coexistence(&self, coexistence: Ieee802154Coexistence) {
        self.runtime
            .with_installed(|radio, _, _| radio.engine().set_coexistence(coexistence));
    }

    /// Run `entry` on the radio's hardware under the runtime's lock, for a
    /// diagnostic read such as the MAC power sequence; `None` while a pause
    /// holds the hardware.
    pub fn with_hardware<O>(&self, entry: impl FnOnce(&mut H) -> O) -> Option<O> {
        self.runtime
            .with_installed(|_, hardware, _| match hardware {
                Hardware::Held(hardware) => Some(entry(hardware)),
                Hardware::Lent { .. } => None,
            })
    }

    /// End the operation in flight, disable the radio and take the engine
    /// and hardware out of the runtime, consuming both handles of the
    /// installed radio (`esp_ieee802154_disable`). Call it after the
    /// platform CPU route is disabled. Queued events stay readable.
    ///
    /// # Panics
    ///
    /// `port` belongs to another runtime, or the radio is paused.
    pub fn uninstall(
        self,
        port: Ieee802154Port<'_, Ieee802154Runtime<'storage, M, H, T, R, EVENTS>>,
    ) -> Ieee802154RuntimeParts<'storage, H> {
        assert!(
            core::ptr::eq(self.runtime, port.runtime),
            "the port belongs to this control's runtime"
        );
        self.runtime.uninstall()
    }
}

impl<
    'storage,
    M: RawMutex,
    H: Ieee802154LowLevel + Ieee802154RecentRssi,
    T: Timer,
    R: Ieee802154Random,
    const EVENTS: usize,
> Ieee802154Control<'_, Ieee802154Runtime<'storage, M, H, T, R, EVENTS>>
{
    /// Stop the MAC for shared PHY maintenance, as a layer over the port's
    /// lifecycle: an enabled port the consumer has not quiesced is quiesced
    /// (the consumer takes `Quiesced`), a receiving radio leaves receive
    /// mode, and the hardware is lent out. The caller then closes the
    /// platform CPU route.
    ///
    /// Until [`Self::resume`], the port holds the radio still as a quiesced
    /// port does: it refuses commands that write the hardware as
    /// [`CommandError::Quiesced`] (a disabled radio's as
    /// [`CommandError::Disabled`]) and a setting that
    /// writes the hardware as [`SettingError::Quiesced`]; lifecycle commands
    /// are refused as [`LifecycleError::Busy`]. Its state, frame counters,
    /// recent RSSI and software settings stay available.
    ///
    /// # Errors
    ///
    /// An operation with a pending terminal event is running, or the event
    /// queue has no room for the `Quiesced` and `Enabled` the pause owes;
    /// nothing changes.
    ///
    /// # Panics
    ///
    /// The radio is paused already.
    pub fn pause(&mut self) -> Result<Ieee802154RuntimePaused<H>, Ieee802154PauseError> {
        self.runtime.pause()
    }

    /// Take a paused radio's hardware back, enter receive mode again if it
    /// was receiving and enable the port the pause quiesced (the consumer
    /// takes `Enabled`). Open the platform CPU route afterwards.
    ///
    /// # Panics
    ///
    /// The radio is not paused.
    pub fn resume(&mut self, paused: Ieee802154RuntimePaused<H>) {
        self.runtime.resume(paused);
    }
}

impl<M: RawMutex, H, T: Timer, R: Ieee802154Random, const EVENTS: usize> RadioPort
    for Ieee802154Port<'_, Ieee802154Runtime<'_, M, H, T, R, EVENTS>>
where
    H: Ieee802154LowLevel + Ieee802154RecentRssi,
{
    type Event = Ieee802154RadioEvent;
    type Id = RequestId;
    type Domain = oer_ieee802154::Ieee802154Radio;
    /// The runtime never poisons.
    type Fault = Infallible;

    fn next_event(
        &self,
    ) -> impl Future<Output = PortResult<Ieee802154RadioEvent, EventsLost, Infallible>> + '_ {
        self.runtime.wait_event()
    }

    /// The radio clock (`otPlatRadioGetNow`): the runtime's own clock, the
    /// one its engine reads at every event.
    fn now(
        &self,
    ) -> impl Future<Output = PortResult<Ieee802154Instant, ClockError, Infallible>> + '_ {
        ready(Ok(Ok(Ieee802154Instant::from_micros(
            self.runtime.clock().now().as_micros(),
        ))))
    }

    fn cancel(
        &self,
        id: RequestId,
    ) -> impl Future<Output = PortResult<(), CancelError, Infallible>> + '_ {
        ready(Ok(self.runtime.cancel_locked(id)))
    }

    fn lifecycle(
        &self,
        command: LifecycleCommand,
    ) -> impl Future<Output = PortResult<(), LifecycleError, Infallible>> + '_ {
        ready(Ok(self.runtime.lifecycle_locked(command)))
    }
}

impl<M: RawMutex, H, T: Timer, R: Ieee802154Random, const EVENTS: usize> Ieee802154RadioPort
    for Ieee802154Port<'_, Ieee802154Runtime<'_, M, H, T, R, EVENTS>>
where
    H: Ieee802154LowLevel + Ieee802154RecentRssi,
{
    fn view(event: &Ieee802154RadioEvent) -> RadioEvent<'_> {
        event.portable()
    }

    /// The installed radio's capabilities and interfaces.
    fn capabilities(&self) -> Ieee802154Capabilities {
        self.runtime
            .with_installed(|radio, _, _| Ieee802154Capabilities {
                operations: radio.capabilities(),
                interfaces: radio.interfaces(),
            })
    }

    fn submit(
        &self,
        command: RadioCommand<'_>,
    ) -> PortResult<AcceptedCommand, CommandError, Infallible> {
        Ok(self.runtime.submit_locked(command))
    }

    /// The radio clock is the runtime's clock (`esp_timer_get_time` in
    /// ESP-IDF), which the ESP32-S31 composition binds to the image's
    /// monotonic clock.
    fn clock_info(&self) -> ClockInfo {
        ClockInfo::MONOTONIC_MICROS
    }

    fn state(&self) -> Result<RadioState, Poisoned<Infallible>> {
        Ok(self.runtime.with_installed(|radio, _, _| radio.state()))
    }

    fn apply(&self, setting: RadioSetting<'_>) -> PortResult<(), SettingError, Infallible> {
        Ok(self.runtime.with_installed(|radio, hardware, _| {
            // A quiesced port holds the hardware still.
            let hardware = match hardware {
                Hardware::Held(hardware) if !self.runtime.quiesced() => Some(hardware),
                Hardware::Held(_) | Hardware::Lent { .. } => None,
            };
            apply_setting(radio, hardware, setting)
        }))
    }

    fn frame_counter(&self, interface: Interface) -> Result<Option<u32>, Poisoned<Infallible>> {
        Ok(self.runtime.with_installed(|radio, _, _| {
            radio
                .interface_mac_keys(interface)
                .and_then(|keys| keys.as_ref().map(MacKeys::frame_counter))
        }))
    }

    /// The live RSSI (`esp_ieee802154_get_recent_rssi`), read from the
    /// hardware whatever the radio's state, as the vendor reads it; while a
    /// pause holds the hardware, the value it read last.
    // CAPABILITY: ieee802154-phy-and-rf-rssi
    fn recent_rssi(&self) -> Result<i8, Poisoned<Infallible>> {
        Ok(self
            .runtime
            .with_installed(|_, hardware, _| match hardware {
                Hardware::Held(hardware) => hardware.recent_rssi(),
                Hardware::Lent { recent_rssi } => *recent_rssi,
            }))
    }
}
