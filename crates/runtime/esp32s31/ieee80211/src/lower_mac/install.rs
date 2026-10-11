//! The ESP32-S31 Wi-Fi port bound to the radio's lifetime.
//!
//! [`install_esp32s31_wifi_port`] takes the stopped Wi-Fi a radio start
//! produced: it publishes the MAC register owner in the radio's arena, wraps
//! it in [`CooperativeRadioHardware`] and installs the lower-MAC core over
//! it. The port then owns the MAC for as long as the radio runs: no role
//! materializes or returns the hardware in between.
//! [`uninstall_esp32s31_wifi_port`] reverses it at radio stop: it takes the
//! core back, reclaims the register owner and reassembles the stopped
//! Wi-Fi the radio stop releases. The core comes back as the parts a later
//! install rebuilds it from ([`LowerMacCore::dismantle`]).
//!
//! Clocks and the PHY stay with the radio system: the port never holds a
//! `PlatformClockGuard`, so it releases nothing here, and the radio stop's
//! own release ([`WifiStopped::release`]) ends them explicitly.

use oer_esp32s31_hal::{
    ieee80211::arena::{RadioAccess, RadioOwnerArena, RadioOwnerArenaError},
    owner::MacInterruptSetup,
};
use oer_esp32s31_ieee80211::{
    cooperative_hardware::CooperativeRadioHardware,
    lower_mac::{AmpduBacking, LowerMacCore, LowerMacCoreParts},
    ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile},
    runtime::{WifiRoleOwner, WifiStopped, materialize_esp32s31_wifi_role},
};
use oer_ieee80211_mac::channel::WifiChannel;

use embassy_sync::blocking_mutex::raw::RawMutex;

use super::{
    Esp32s31LowerMac, Esp32s31LowerMacControl, Esp32s31LowerMacParts, Esp32s31LowerMacPort,
    LowerMacRetune, LowerMacRxUnit,
};

/// The product retune of an installed port: one channel switch under the
/// shared radio's lease, through the register owner published in the arena.
pub struct Esp32s31PortRetune<'a, P, C, D, O> {
    radio: &'a oer_esp32s31_radio_runtime::RadioSystem<P, C, D>,
    registers: RadioAccess<'a>,
    observer: O,
}

impl<'a, P, C, D, O> Esp32s31PortRetune<'a, P, C, D, O> {
    /// A retune of the port whose registers `registers` reaches, leasing
    /// `radio` for each switch; `observer` sees the PHY's steps.
    pub const fn new(
        radio: &'a oer_esp32s31_radio_runtime::RadioSystem<P, C, D>,
        registers: RadioAccess<'a>,
        observer: O,
    ) -> Self {
        Self {
            radio,
            registers,
            observer,
        }
    }
}

impl<P, C, D, O> LowerMacRetune for Esp32s31PortRetune<'_, P, C, D, O>
where
    C: oer_esp32s31_hal::shared_radio::PlatformClockProvider,
    D: oer_time::Timer,
    O: oer_esp32s31_phy::PhyTargetObserver,
{
    async fn retune(&mut self, channel: WifiChannel) -> bool {
        let mut guard = self.radio.lock().await;
        let (lease, platform, _) = guard.parts();
        oer_esp32s31_ieee80211::switch_published_esp32s31_wifi_channel::<
            oer_esp32s31_phy::RomShortDelay,
            _,
            _,
        >(
            self.radio.timer(),
            lease,
            platform,
            &self.registers,
            channel,
            &mut self.observer,
        )
        .await
        .is_ok()
    }
}

/// What the installed port holds aside for the radio stop: the Wi-Fi owner
/// the stopped radio gave up. The interrupt setup is the composition's
/// while the port runs: it routes the MAC and power interrupts to the port
/// and returns the setup at uninstall.
#[must_use = "the radio stop needs the held Wi-Fi owner back"]
pub struct Esp32s31WifiPortHeld<W> {
    owner: WifiRoleOwner<W>,
}

/// An installed port: its handles, the owner held for the radio stop and
/// the interrupt setup for the composition's route.
pub struct Esp32s31WifiPortInstalled<'r, L, W> {
    pub port: Esp32s31LowerMacPort<'r, L>,
    pub control: Esp32s31LowerMacControl<'r, L>,
    pub held: Esp32s31WifiPortHeld<W>,
    pub interrupt_setup: MacInterruptSetup,
}

/// Why the port could not be installed, with every owner it was given.
pub struct Esp32s31WifiPortInstallFailure<W, C, R> {
    pub error: Esp32s31WifiPortInstallError,
    pub wifi: WifiStopped<W>,
    pub core: C,
    pub retune: R,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31WifiPortInstallError {
    /// The arena refused the register owner.
    Registers(RadioOwnerArenaError),
    /// A core is installed already.
    Installed,
}

/// The stopped Wi-Fi and the core's parts back from an uninstalled port.
pub struct Esp32s31WifiPortUninstalled<W, Parts> {
    pub wifi: WifiStopped<W>,
    pub core: Parts,
}

/// Why the port could not be taken apart: it is still installed's owners,
/// returned as they were so the caller retries after the cause ends.
pub enum Esp32s31WifiPortUninstallFailure<'arena, W, Core> {
    /// A register transaction is still borrowed; the hardware and the core
    /// are back, uninstalled, with the held owner and the setup.
    Registers {
        error: RadioOwnerArenaError,
        hardware: CooperativeRadioHardware<'arena>,
        core: Core,
        held: Esp32s31WifiPortHeld<W>,
        interrupt_setup: MacInterruptSetup,
    },
    /// The core is not disabled and idle; the stopped Wi-Fi is back.
    Core { wifi: WifiStopped<W>, core: Core },
}

/// Install the lower-MAC port over the stopped Wi-Fi a radio start produced:
/// publish its register owner in `arena`, wrap it in
/// [`CooperativeRadioHardware`] and install `core` with `retune` into
/// `runtime`. The port starts disabled with an empty stream.
///
/// # Errors
///
/// Every owner comes back unchanged when the arena refuses the register
/// owner or a core is installed already.
#[allow(
    clippy::result_large_err,
    clippy::type_complexity,
    reason = "the no-alloc runtime returns every unconsumed owner by value"
)]
pub fn install_esp32s31_wifi_port<
    'r,
    'arena,
    'slot,
    M: RawMutex,
    P,
    E,
    T,
    R,
    W,
    const BUFFER_SIZE: usize,
    const TX_BUFFERS: usize,
    const EVENTS: usize,
    U: LowerMacRxUnit,
    S: AmpduBacking,
    const AMPDU_SLOTS: usize,
    const AMPDU_BUFFERS: usize,
    O,
>(
    runtime: &'r Esp32s31LowerMac<
        'slot,
        M,
        P,
        E,
        T,
        CooperativeRadioHardware<'arena>,
        R,
        BUFFER_SIZE,
        TX_BUFFERS,
        EVENTS,
        U,
        S,
        AMPDU_SLOTS,
        AMPDU_BUFFERS,
        O,
    >,
    wifi: WifiStopped<W>,
    arena: &'arena RadioOwnerArena,
    core: LowerMacCore<'slot, P, E, T, BUFFER_SIZE, TX_BUFFERS, S, AMPDU_SLOTS, AMPDU_BUFFERS>,
    retune: R,
) -> Result<
    Esp32s31WifiPortInstalled<
        'r,
        Esp32s31LowerMac<
            'slot,
            M,
            P,
            E,
            T,
            CooperativeRadioHardware<'arena>,
            R,
            BUFFER_SIZE,
            TX_BUFFERS,
            EVENTS,
            U,
            S,
            AMPDU_SLOTS,
            AMPDU_BUFFERS,
            O,
        >,
        W,
    >,
    Esp32s31WifiPortInstallFailure<
        W,
        LowerMacCore<'slot, P, E, T, BUFFER_SIZE, TX_BUFFERS, S, AMPDU_SLOTS, AMPDU_BUFFERS>,
        R,
    >,
>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
    R: LowerMacRetune,
    O: oer_ieee80211_lower_mac::TxBody,
{
    let materialized = materialize_esp32s31_wifi_role(wifi, ());
    let (owner, registers, interrupt_setup) = (
        materialized.owner,
        materialized.registers,
        materialized.interrupt_setup,
    );
    let published = match arena.publish(registers) {
        Ok(published) => published,
        Err(failure) => {
            return Err(Esp32s31WifiPortInstallFailure {
                error: Esp32s31WifiPortInstallError::Registers(failure.error),
                wifi: owner.into_stopped(failure.owner, interrupt_setup, ()).wifi,
                core,
                retune,
            });
        }
    };
    let hardware = CooperativeRadioHardware::new(published);
    match runtime.install(Esp32s31LowerMacParts {
        core,
        hardware,
        retune,
    }) {
        Ok((port, control)) => Ok(Esp32s31WifiPortInstalled {
            port,
            control,
            held: Esp32s31WifiPortHeld { owner },
            interrupt_setup,
        }),
        Err(parts) => {
            let registers = match parts.hardware.try_into_registers() {
                Ok(registers) => registers,
                // Nothing borrowed the fresh publication.
                Err((_, error)) => unreachable!("a fresh publication reclaims: {error:?}"),
            };
            Err(Esp32s31WifiPortInstallFailure {
                error: Esp32s31WifiPortInstallError::Installed,
                wifi: owner.into_stopped(registers, interrupt_setup, ()).wifi,
                core: parts.core,
                retune: parts.retune,
            })
        }
    }
}

/// Take the port apart at radio stop: uninstall its core, reclaim the
/// register owner from the arena and reassemble the stopped Wi-Fi from it,
/// the held owner and the interrupt setup the composition's route returned.
/// The port must be disabled with nothing admitted, its DMA and interrupt
/// service stopped; the stopped Wi-Fi records the channel the core is
/// tuned to.
///
/// # Errors
///
/// A still borrowed register transaction or a core that is not disabled and
/// idle returns every owner, as [`Esp32s31WifiPortUninstallFailure`] says.
#[allow(
    clippy::result_large_err,
    clippy::type_complexity,
    reason = "the no-alloc runtime returns every unconsumed owner by value"
)]
pub fn uninstall_esp32s31_wifi_port<
    'r,
    'arena,
    'slot,
    M: RawMutex,
    P,
    E,
    T,
    R,
    W,
    const BUFFER_SIZE: usize,
    const TX_BUFFERS: usize,
    const EVENTS: usize,
    U: LowerMacRxUnit,
    S: AmpduBacking,
    const AMPDU_SLOTS: usize,
    const AMPDU_BUFFERS: usize,
    O,
>(
    installed: Esp32s31WifiPortInstalled<
        'r,
        Esp32s31LowerMac<
            'slot,
            M,
            P,
            E,
            T,
            CooperativeRadioHardware<'arena>,
            R,
            BUFFER_SIZE,
            TX_BUFFERS,
            EVENTS,
            U,
            S,
            AMPDU_SLOTS,
            AMPDU_BUFFERS,
            O,
        >,
        W,
    >,
) -> Result<
    Esp32s31WifiPortUninstalled<
        W,
        LowerMacCoreParts<'slot, P, E, T, BUFFER_SIZE, TX_BUFFERS, S, AMPDU_SLOTS, AMPDU_BUFFERS>,
    >,
    Esp32s31WifiPortUninstallFailure<
        'arena,
        W,
        LowerMacCore<'slot, P, E, T, BUFFER_SIZE, TX_BUFFERS, S, AMPDU_SLOTS, AMPDU_BUFFERS>,
    >,
>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
    R: LowerMacRetune,
    O: oer_ieee80211_lower_mac::TxBody,
{
    let Esp32s31WifiPortInstalled {
        port,
        control,
        held,
        interrupt_setup,
    } = installed;
    let (core, hardware) = control.uninstall(port);
    let registers = match hardware.try_into_registers() {
        Ok(registers) => registers,
        Err((hardware, error)) => {
            return Err(Esp32s31WifiPortUninstallFailure::Registers {
                error,
                hardware,
                core,
                held,
                interrupt_setup,
            });
        }
    };
    let mut owner = held.owner;
    owner.set_current_channel(core.tuned_channel());
    let wifi = owner.into_stopped(registers, interrupt_setup, ()).wifi;
    match core.dismantle() {
        Ok(core) => Ok(Esp32s31WifiPortUninstalled { wifi, core }),
        Err(core) => Err(Esp32s31WifiPortUninstallFailure::Core { wifi, core }),
    }
}
