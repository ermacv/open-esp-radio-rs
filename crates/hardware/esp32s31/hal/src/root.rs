//! Protocol-neutral radio root and exclusive protocol routes.
//!
//! The restricted PAC supplies opaque register partitions and register sets
//! without any route policy. This module owns the complete neutral root,
//! decides which partitions each exclusive protocol route consumes, retains
//! the remaining partitions privately and reconstructs the root only after
//! the route's restore obligations are complete.

use oer_esp32s31_pac::{
    BluetoothControllerPartition, BluetoothInterruptSetup, BluetoothModemLpTimerRegisters,
    BluetoothTaskRegisters, Ieee802154InterruptSetup, Ieee802154Partition, Ieee802154TaskParts,
    Ieee802154TaskRegisters, MacInterruptSetup, RadioPartitions, SharedRadioParts,
    SharedRadioRegisters, WifiMacPartition, WifiRadioRegisters,
};

pub use crate::clock::CommonPhyPowerError;
use crate::{
    clock::CommonPhyPower,
    phy::{registration::PhyRegistration, restore::PhyRouteState},
    route_registers::{BluetoothRegisters, WifiRegisters},
};

/// Unique protocol-neutral owner of every reviewed ESP32-S31 radio region.
///
/// This is the sole production acquisition root. It can be consumed by
/// exactly one exclusive protocol route; no route can manufacture a second
/// owner. Every route returns the complete root after its task and interrupt
/// capabilities have been reunited. Protocol-specific cached state is
/// deliberately not retained in this neutral owner.
///
/// ```compile_fail
/// use oer_esp32s31_hal::{bluetooth::ColdOwner, root::RadioHardware};
///
/// let hardware = RadioHardware::take().unwrap();
/// let _first = ColdOwner::from_radio_hardware(hardware);
/// let _second = ColdOwner::from_radio_hardware(hardware);
/// ```
#[must_use = "dropping the radio root permanently loses the unique hardware capability"]
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

    /// Consume the root into the exclusive Wi-Fi route. This performs no MMIO.
    pub(crate) fn into_wifi(self) -> WifiRoute {
        wifi_route(self.partitions, PhyRouteState::new(self.phy_registration))
    }

    /// Reconstruct the root from a Wi-Fi route whose leases were released.
    pub(crate) fn from_wifi(
        registers: WifiRegisters,
        interrupts: MacInterruptSetup,
        phy: PhyRouteState,
        retained: RetainedBluetooth,
    ) -> Self {
        Self::returned(wifi_partitions(registers, interrupts, retained), phy)
    }

    /// Consume the root into the exclusive Bluetooth route.
    ///
    /// This transition is ownership-only. It performs no controller reset,
    /// clock, interrupt, or enable transaction.
    pub(crate) fn into_bluetooth(self) -> BluetoothRoute {
        bluetooth_route(self.partitions, PhyRouteState::new(self.phy_registration))
    }

    /// Reconstruct the root from a Bluetooth route.
    pub(crate) fn from_bluetooth(
        task: BluetoothRegisters,
        modem_lp_timer: BluetoothModemLpTimerRegisters,
        interrupts: BluetoothInterruptSetup,
        phy: PhyRouteState,
        retained: RetainedWifi,
    ) -> Self {
        Self::returned(
            bluetooth_partitions(task, modem_lp_timer, interrupts, retained),
            phy,
        )
    }

    /// Consume the root into the exclusive IEEE 802.15.4 route.
    ///
    /// This ownership-only transition follows the same whole-radio rule as
    /// the Wi-Fi and Bluetooth routes. It performs no module clock,
    /// common-PHY, BTBB, coexistence, reset, DMA, or interrupt transaction.
    /// The route owns the IEEE 802.15.4 MAC and the shared resources required
    /// by the public ESP-IDF enable sequence; Wi-Fi and Bluetooth IRQ
    /// authority remain retained and inaccessible.
    pub(crate) fn into_ieee802154(self) -> Ieee802154Route {
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
        let phy = PhyRouteState::new(self.phy_registration);
        let (task, interrupts) = Ieee802154TaskRegisters::new(Ieee802154TaskParts {
            ieee802154,
            shared: SharedRadioRegisters::new(SharedRadioParts {
                radio_phy,
                coexistence,
                shared_radio,
            }),
            bluetooth,
        });
        Ieee802154Route {
            task,
            interrupts,
            phy,
            retained: RetainedIeee802154 {
                wifi_mac,
                wifi_interrupts,
                bluetooth_modem_lp_timer,
                bluetooth_interrupts,
            },
        }
    }

    /// Reconstruct the root from an IEEE 802.15.4 route.
    pub(crate) fn from_ieee802154(route: Ieee802154Route) -> Self {
        let Ieee802154Route {
            task,
            interrupts,
            phy,
            retained:
                RetainedIeee802154 {
                    wifi_mac,
                    wifi_interrupts,
                    bluetooth_modem_lp_timer,
                    bluetooth_interrupts,
                },
        } = route;
        let Ieee802154TaskParts {
            ieee802154,
            shared,
            bluetooth,
        } = task.into_parts(interrupts);
        let SharedRadioParts {
            radio_phy,
            coexistence,
            shared_radio,
        } = shared.into_parts();
        Self::returned(
            RadioPartitions {
                wifi_mac,
                wifi_interrupts,
                radio_phy,
                coexistence,
                bluetooth,
                bluetooth_modem_lp_timer,
                bluetooth_interrupts,
                shared_radio,
                ieee802154,
            },
            phy,
        )
    }
}

fn wifi_route(partitions: RadioPartitions, phy: PhyRouteState) -> WifiRoute {
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
    } = partitions;
    WifiRoute {
        registers: WifiRegisters::new(
            WifiRadioRegisters::new(wifi_mac),
            SharedRadioRegisters::new(SharedRadioParts {
                radio_phy,
                coexistence,
                shared_radio,
            }),
        ),
        interrupts: wifi_interrupts,
        phy,
        retained: RetainedBluetooth {
            bluetooth,
            modem_lp_timer: bluetooth_modem_lp_timer,
            interrupts: bluetooth_interrupts,
            ieee802154,
        },
    }
}

fn wifi_partitions(
    registers: WifiRegisters,
    interrupts: MacInterruptSetup,
    retained: RetainedBluetooth,
) -> RadioPartitions {
    let (registers, shared) = registers.into_parts();
    let SharedRadioParts {
        radio_phy,
        coexistence,
        shared_radio,
    } = shared.into_parts();
    let RetainedBluetooth {
        bluetooth,
        modem_lp_timer,
        interrupts: bluetooth_interrupts,
        ieee802154,
    } = retained;
    RadioPartitions {
        wifi_mac: registers.into_partition(),
        wifi_interrupts: interrupts,
        radio_phy,
        coexistence,
        bluetooth,
        bluetooth_modem_lp_timer: modem_lp_timer,
        bluetooth_interrupts,
        shared_radio,
        ieee802154,
    }
}

fn bluetooth_route(partitions: RadioPartitions, phy: PhyRouteState) -> BluetoothRoute {
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
    } = partitions;
    BluetoothRoute {
        task: BluetoothRegisters::new(
            BluetoothTaskRegisters::new(bluetooth),
            SharedRadioRegisters::new(SharedRadioParts {
                radio_phy,
                coexistence,
                shared_radio,
            }),
        ),
        modem_lp_timer: bluetooth_modem_lp_timer,
        interrupts: bluetooth_interrupts,
        phy,
        retained: RetainedWifi {
            wifi_mac,
            interrupts: wifi_interrupts,
            ieee802154,
        },
    }
}

fn bluetooth_partitions(
    task: BluetoothRegisters,
    modem_lp_timer: BluetoothModemLpTimerRegisters,
    interrupts: BluetoothInterruptSetup,
    retained: RetainedWifi,
) -> RadioPartitions {
    let (task, shared) = task.into_parts();
    let SharedRadioParts {
        radio_phy,
        coexistence,
        shared_radio,
    } = shared.into_parts();
    let RetainedWifi {
        wifi_mac,
        interrupts: wifi_interrupts,
        ieee802154,
    } = retained;
    RadioPartitions {
        wifi_mac,
        wifi_interrupts,
        radio_phy,
        coexistence,
        bluetooth: task.into_partition(),
        bluetooth_modem_lp_timer: modem_lp_timer,
        bluetooth_interrupts: interrupts,
        shared_radio,
        ieee802154,
    }
}

/// Neutral radio root whose shared PHY stays powered and registered between
/// two protocol routes.
///
/// A route that closed RF over its registered PHY returns this owner instead
/// of the cold [`RadioHardware`]: its protocol-specific leases are released,
/// but the common PHY power sequence, the PHY-I2C lease, the cold-power
/// baseline and the current registration epoch stay in effect. The next
/// route enters from it without repeating the power sequence, so a PHY layer
/// can wake the retained RF state instead of registering and calibrating
/// again. Only the route that finally returns the cold root restores the
/// baseline.
///
/// No other protocol may use RF while this owner exists; it is not a
/// coexistence grant.
#[must_use = "dropping the retained radio root permanently loses the powered radio"]
pub struct RetainedRadioHardware {
    partitions: RadioPartitions,
    phy: PhyRouteState,
    common: CommonPhyPower,
}

impl RetainedRadioHardware {
    /// The registration that still describes the retained PHY.
    pub const fn registration_epoch(&self) -> Option<crate::owner::PhyRegistrationEpoch> {
        self.phy.registration_epoch()
    }

    pub(crate) fn from_wifi(
        registers: WifiRegisters,
        interrupts: MacInterruptSetup,
        phy: PhyRouteState,
        retained: RetainedBluetooth,
        common: CommonPhyPower,
    ) -> Self {
        Self {
            partitions: wifi_partitions(registers, interrupts, retained),
            phy,
            common,
        }
    }

    pub(crate) fn into_wifi(self) -> (WifiRoute, CommonPhyPower) {
        (wifi_route(self.partitions, self.phy), self.common)
    }

    pub(crate) fn from_bluetooth(
        task: BluetoothRegisters,
        modem_lp_timer: BluetoothModemLpTimerRegisters,
        interrupts: BluetoothInterruptSetup,
        phy: PhyRouteState,
        retained: RetainedWifi,
        common: CommonPhyPower,
    ) -> Self {
        Self {
            partitions: bluetooth_partitions(task, modem_lp_timer, interrupts, retained),
            phy,
            common,
        }
    }

    pub(crate) fn into_bluetooth(self) -> (BluetoothRoute, CommonPhyPower) {
        (bluetooth_route(self.partitions, self.phy), self.common)
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

/// Why a route cannot hand its registered PHY to the retained root.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetainedRadioReleaseError {
    /// A PHY calibration still owns a restore obligation in this route.
    Restore(RadioPhyReleaseError),
    /// The route never established the common PHY power it would hand over.
    CommonPhyPower(CommonPhyPowerError),
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
    /// The route still retains its platform PLL-source lease.
    PlatformPllLease,
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

/// Registers and interrupt setup of one exclusive Wi-Fi route.
pub(crate) struct WifiRoute {
    pub(crate) registers: WifiRegisters,
    pub(crate) interrupts: MacInterruptSetup,
    pub(crate) phy: PhyRouteState,
    pub(crate) retained: RetainedBluetooth,
}

/// Registers and inactive interrupt bank of one exclusive Bluetooth route.
pub(crate) struct BluetoothRoute {
    pub(crate) task: BluetoothRegisters,
    pub(crate) modem_lp_timer: BluetoothModemLpTimerRegisters,
    pub(crate) interrupts: BluetoothInterruptSetup,
    pub(crate) phy: PhyRouteState,
    pub(crate) retained: RetainedWifi,
}

/// Task registers and inactive interrupt owner of one IEEE 802.15.4 route.
pub(crate) struct Ieee802154Route {
    pub(crate) task: Ieee802154TaskRegisters,
    pub(crate) interrupts: Ieee802154InterruptSetup,
    pub(crate) phy: PhyRouteState,
    pub(crate) retained: RetainedIeee802154,
}

/// Bluetooth and IEEE 802.15.4 partitions retained, but not exposed, while
/// Wi-Fi is exclusive.
pub(crate) struct RetainedBluetooth {
    bluetooth: BluetoothControllerPartition,
    modem_lp_timer: BluetoothModemLpTimerRegisters,
    interrupts: BluetoothInterruptSetup,
    ieee802154: Ieee802154Partition,
}

/// Wi-Fi partitions retained, but not exposed, while Bluetooth is exclusive.
pub(crate) struct RetainedWifi {
    wifi_mac: WifiMacPartition,
    interrupts: MacInterruptSetup,
    ieee802154: Ieee802154Partition,
}

/// Partitions IEEE 802.15.4 does not use during its exclusive epoch.
pub(crate) struct RetainedIeee802154 {
    wifi_mac: WifiMacPartition,
    wifi_interrupts: MacInterruptSetup,
    bluetooth_modem_lp_timer: BluetoothModemLpTimerRegisters,
    bluetooth_interrupts: BluetoothInterruptSetup,
}

#[cfg(test)]
mod tests;
