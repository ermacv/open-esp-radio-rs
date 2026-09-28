//! Protocol-neutral radio root and its concurrent split.
//!
//! The restricted PAC supplies opaque register partitions and register sets
//! without any route policy. This module owns the complete neutral root.
//! [`RadioHardware::into_concurrent`] hands every protocol partition out at
//! once and places the shared partitions under the [`SharedRadio`] arbiter;
//! [`RadioHardware::from_concurrent`] reunites them only after the shared
//! restore obligations are complete.

use oer_esp32s31_pac::{
    BluetoothControllerPartition, BluetoothInterruptSetup, BluetoothModemLpTimerRegisters,
    Ieee802154Partition, MacInterruptSetup, RadioPartitions, SharedRadioParts,
    SharedRadioRegisters, WifiMacPartition,
};

use crate::{
    phy::{registration::PhyRegistration, restore::PhyRouteState},
    shared_radio::{SharedRadio, SharedRadioReleaseError},
};

/// Unique protocol-neutral owner of every reviewed ESP32-S31 radio region.
///
/// This is the sole production acquisition root. It can be consumed by
/// exactly one concurrent split, which cannot manufacture a second owner. The
/// split returns the complete root after every partition and the arbiter
/// have been reunited. Protocol-specific cached state is
/// deliberately not retained in this neutral owner.
///
/// ```compile_fail
/// use oer_esp32s31_hal::root::RadioHardware;
///
/// let hardware = RadioHardware::take().unwrap();
/// let _first = hardware.into_concurrent(());
/// let _second = hardware.into_concurrent(());
/// ```
#[must_use = "dropping the radio root permanently loses the unique hardware capability"]
// CAPABILITY: whole-radio-exclusive-ownership-and-client-handoff-exclusive-radio-root-ownership, radio-cold-start
pub struct RadioHardware {
    partitions: RadioPartitions,
    phy_registration: PhyRegistration,
}

impl RadioHardware {
    /// Reassemble the neutral root from a returning route.
    ///
    /// Leaving the route retires the current PHY registration, so a
    /// registration result held apart from its hardware cannot describe a
    /// later route.
    fn returned(partitions: RadioPartitions, phy: PhyRouteState) -> Self {
        Self {
            partitions,
            phy_registration: phy.into_registration(),
        }
    }

    /// Acquire the radio register singleton once.
    pub fn take() -> Option<Self> {
        RadioPartitions::take().map(|partitions| Self {
            partitions,
            phy_registration: PhyRegistration::new(),
        })
    }

    /// Construct the complete root inside one isolated validation image.
    #[cfg(any(test, feature = "validation-probes"))]
    #[doc(hidden)]
    pub fn for_validation() -> Self {
        Self {
            partitions: RadioPartitions::for_validation(),
            phy_registration: PhyRegistration::new(),
        }
    }
}

/// Wi-Fi MAC partition and its interrupt setup, taken from a concurrent split.
#[must_use = "dropping a radio partition permanently loses its register authority"]
pub struct WifiPartition {
    mac: WifiMacPartition,
    interrupts: MacInterruptSetup,
}

impl WifiPartition {
    pub(crate) fn into_parts(self) -> (WifiMacPartition, MacInterruptSetup) {
        (self.mac, self.interrupts)
    }

    pub(crate) const fn from_parts(mac: WifiMacPartition, interrupts: MacInterruptSetup) -> Self {
        Self { mac, interrupts }
    }
}

/// Bluetooth controller, modem low-power timer and interrupt partitions,
/// taken from a concurrent split.
#[must_use = "dropping a radio partition permanently loses its register authority"]
pub struct BluetoothPartition {
    controller: BluetoothControllerPartition,
    modem_lp_timer: BluetoothModemLpTimerRegisters,
    interrupts: BluetoothInterruptSetup,
}

impl BluetoothPartition {
    pub(crate) fn into_parts(
        self,
    ) -> (
        BluetoothControllerPartition,
        BluetoothModemLpTimerRegisters,
        BluetoothInterruptSetup,
    ) {
        (self.controller, self.modem_lp_timer, self.interrupts)
    }

    pub(crate) const fn from_parts(
        controller: BluetoothControllerPartition,
        modem_lp_timer: BluetoothModemLpTimerRegisters,
        interrupts: BluetoothInterruptSetup,
    ) -> Self {
        Self {
            controller,
            modem_lp_timer,
            interrupts,
        }
    }
}

/// IEEE 802.15.4 MAC partition, taken from a concurrent split.
#[must_use = "dropping a radio partition permanently loses its register authority"]
pub struct Ieee802154RadioPartition {
    mac: Ieee802154Partition,
}

impl Ieee802154RadioPartition {
    pub(crate) fn into_mac(self) -> Ieee802154Partition {
        self.mac
    }

    pub(crate) const fn from_mac(mac: Ieee802154Partition) -> Self {
        Self { mac }
    }
}

/// Protocol partitions of a concurrently split radio root.
///
/// Each field can move to its own protocol route; the shared radio stays with
/// the [`SharedRadio`] arbiter returned beside it.
pub struct ConcurrentPartitions {
    pub wifi: WifiPartition,
    pub bluetooth: BluetoothPartition,
    pub ieee802154: Ieee802154RadioPartition,
}

/// Why a concurrent split cannot become the neutral root again.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConcurrentReunionError {
    Shared(SharedRadioReleaseError),
}

/// Rejected reunion returning both unchanged owners.
#[must_use = "a rejected reunion still owns the arbiter and every partition"]
pub struct ConcurrentReunionFailure<T = ()> {
    shared: SharedRadio<T>,
    partitions: ConcurrentPartitions,
    error: ConcurrentReunionError,
}

impl<T> ConcurrentReunionFailure<T> {
    pub const fn error(&self) -> ConcurrentReunionError {
        self.error
    }

    pub fn into_parts(self) -> (SharedRadio<T>, ConcurrentPartitions) {
        (self.shared, self.partitions)
    }
}

impl RadioHardware {
    /// Split the root for concurrently running protocols. This performs no
    /// MMIO.
    ///
    /// The shared radio partitions and the shared PHY state go to the
    /// returned [`SharedRadio`] arbiter, and every protocol partition to
    /// [`ConcurrentPartitions`]. The two are separate owners so a protocol
    /// can take its partition while other routes hold arbiter leases.
    ///
    /// `attachment` is the upper layer's state kept under the same
    /// arbitration, such as the PHY layer's registered domain slot.
    pub fn into_concurrent<T>(self, attachment: T) -> (SharedRadio<T>, ConcurrentPartitions) {
        let RadioPartitions {
            wifi_mac,
            wifi_interrupts,
            radio_phy,
            coexistence,
            bluetooth,
            bluetooth_modem_lp_timer,
            bluetooth_interrupts,
            shared_radio,
            ieee802154,
        } = self.partitions;
        (
            SharedRadio::new(
                SharedRadioRegisters::new(SharedRadioParts {
                    radio_phy,
                    coexistence,
                    shared_radio,
                }),
                PhyRouteState::new(self.phy_registration),
                attachment,
            ),
            ConcurrentPartitions {
                wifi: WifiPartition {
                    mac: wifi_mac,
                    interrupts: wifi_interrupts,
                },
                bluetooth: BluetoothPartition {
                    controller: bluetooth,
                    modem_lp_timer: bluetooth_modem_lp_timer,
                    interrupts: bluetooth_interrupts,
                },
                ieee802154: Ieee802154RadioPartition { mac: ieee802154 },
            },
        )
    }

    /// Reunite a concurrent split into the neutral root. This performs no
    /// MMIO.
    ///
    /// Consuming the arbiter proves no lease is alive. Leaving retires the
    /// PHY registration, as every route return does.
    ///
    /// # Errors
    ///
    /// Returns both owners while a client holds common power or a PHY
    /// calibration still owns a restore obligation.
    #[allow(
        clippy::result_large_err,
        reason = "rejection returns the arbiter and every partition without allocation"
    )]
    // CAPABILITY: whole-radio-exclusive-ownership-and-client-handoff-inactive-route-release-reselection
    pub fn from_concurrent<T>(
        shared: SharedRadio<T>,
        partitions: ConcurrentPartitions,
    ) -> Result<(Self, T), ConcurrentReunionFailure<T>> {
        let (registers, phy, attachment) = match shared.into_parts() {
            Ok(parts) => parts,
            Err((shared, error)) => {
                return Err(ConcurrentReunionFailure {
                    shared,
                    partitions,
                    error: ConcurrentReunionError::Shared(error),
                });
            }
        };
        let SharedRadioParts {
            radio_phy,
            coexistence,
            shared_radio,
        } = registers.into_parts();
        let ConcurrentPartitions {
            wifi,
            bluetooth,
            ieee802154,
        } = partitions;
        Ok((
            Self::returned(
                RadioPartitions {
                    wifi_mac: wifi.mac,
                    wifi_interrupts: wifi.interrupts,
                    radio_phy,
                    coexistence,
                    bluetooth: bluetooth.controller,
                    bluetooth_modem_lp_timer: bluetooth.modem_lp_timer,
                    bluetooth_interrupts: bluetooth.interrupts,
                    shared_radio,
                    ieee802154: ieee802154.mac,
                },
                phy,
            ),
            attachment,
        ))
    }
}

/// Why a cold protocol route cannot release the neutral radio root.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RadioPhyReleaseError {
    /// TX-DC PWDET still owns fields that must be restored in this route.
    TxDcPwdetRestorePending,
    /// TX-IQ still owns a tone-control image that must be restored.
    TxIqToneControlRestorePending,
    /// RX-DCO still owns one or both nested control-field snapshots.
    RxDcoControlRestorePending,
    /// Bluetooth TX-power calibration still owns analog-control snapshots.
    BluetoothTxPowerControlRestorePending,
    /// A route-owned cold-power field did not return to its captured baseline.
    WifiPowerRestore(WifiPowerRestoreCheckpoint),
}

/// Exact stage whose route-owned cold-power baseline could not be restored.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiPowerRestoreCheckpoint {
    /// Modem syscon clocks and resets did not read back.
    ModemSyscon,
    /// The upstream PLL-source fields did not read back.
    ModemSourceClocks,
    /// The modem register bus clock did not read back.
    ModemRegisterBusClock,
}

/// Reject release while a PHY calibration still owns a restore obligation.
pub(crate) fn check_phy_restore_complete(
    restore: &PhyRouteState,
) -> Result<(), RadioPhyReleaseError> {
    if restore.txdc_pending() {
        return Err(RadioPhyReleaseError::TxDcPwdetRestorePending);
    }
    if restore.txiq_pending() {
        return Err(RadioPhyReleaseError::TxIqToneControlRestorePending);
    }
    if restore.rx_dco_pending() {
        return Err(RadioPhyReleaseError::RxDcoControlRestorePending);
    }
    if restore.bluetooth_tx_power_control_pending() {
        return Err(RadioPhyReleaseError::BluetoothTxPowerControlRestorePending);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
