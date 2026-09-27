//! Hardware ownership, capabilities, and affine lifecycle transitions.

use super::*;

/// Physical owners used by one IEEE 802.15.4 task register set.
///
/// The MAC, its interrupt route and its ETM channels are the whole IEEE
/// 802.15.4 authority. Clocks, resets and the common PHY it depends on are
/// not modelled by this crate yet.
pub(crate) struct Ieee802154TaskPeripheralOwners {
    pub(crate) ieee802154_mac: crate::ieee802154::ownership::TaskRegisters,
    pub(crate) ieee802154_interrupt_route: svd::Ieee802154InterruptRoute,
    pub(crate) etm: crate::modem::etm::Ieee802154EtmChannels,
}

/// Opaque IEEE 802.15.4 MAC and interrupt-route register partition.
#[must_use = "dropping a radio partition permanently loses its register authority"]
pub struct Ieee802154Partition {
    peripherals: svd::peripheral_ownership::Ieee802154Peripherals,
    etm: crate::modem::etm::Ieee802154EtmChannels,
}

/// Every reviewed ESP32-C5 radio register partition.
///
/// This is the sole production acquisition point.
///
/// ```compile_fail
/// use oer_esp32c5_pac::RadioPartitions;
///
/// let partitions = RadioPartitions::take().unwrap();
/// let _first = partitions.ieee802154;
/// let _second = partitions.ieee802154;
/// ```
#[must_use = "dropping the radio partitions permanently loses the unique hardware capability"]
pub struct RadioPartitions {
    pub ieee802154: Ieee802154Partition,
}

impl RadioPartitions {
    /// Acquire the generated radio singleton once.
    pub fn take() -> Option<Self> {
        svd::Peripherals::take().map(Self::from_peripherals)
    }

    /// Bind the generated singleton to the opaque partition owners.
    pub(crate) fn from_peripherals(peripherals: svd::Peripherals) -> Self {
        let svd::peripheral_ownership::PeripheralPartitions {
            ieee802154,
            modem_etm,
        } = svd::peripheral_ownership::partition(peripherals);
        Self {
            ieee802154: Ieee802154Partition {
                peripherals: ieee802154,
                etm: crate::modem::etm::ieee802154_channels(modem_etm),
            },
        }
    }

    /// Construct every partition inside one isolated validation image.
    #[cfg(any(test, feature = "validation-probes"))]
    #[doc(hidden)]
    pub fn for_validation() -> Self {
        Self::from_peripherals(svd::peripheral_ownership::peripherals_for_validation())
    }
}

#[inline]
pub(crate) fn device_fence() {
    svd::device_access::fence();
}

/// Ordinary task-side owner of the IEEE 802.15.4 MAC partition.
#[must_use = "the IEEE 802.15.4 task owner must be reunited before release"]
pub struct Ieee802154TaskRegisters {
    pub(crate) peripherals: Ieee802154TaskPeripheralOwners,
}

impl Ieee802154TaskRegisters {
    /// Assemble the task register set and split the MAC block into its
    /// disjoint task and interrupt owners. This performs no MMIO.
    pub fn new(partition: Ieee802154Partition) -> (Self, Ieee802154InterruptSetup) {
        let Ieee802154Partition {
            peripherals:
                svd::peripheral_ownership::Ieee802154Peripherals {
                    ieee802154_mac,
                    ieee802154_interrupt_route,
                },
            etm,
        } = partition;
        let (task_mac, interrupt_mac) = crate::ieee802154::ownership::split(ieee802154_mac);
        (
            Self {
                peripherals: Ieee802154TaskPeripheralOwners {
                    ieee802154_mac: task_mac,
                    ieee802154_interrupt_route,
                    etm,
                },
            },
            Ieee802154InterruptSetup {
                registers: interrupt_mac,
            },
        )
    }

    /// Reunite the MAC block with its inactive interrupt owner and return the
    /// partition. This performs no MMIO.
    pub fn into_partition(self, interrupts: Ieee802154InterruptSetup) -> Ieee802154Partition {
        let Ieee802154TaskPeripheralOwners {
            ieee802154_mac: task_mac,
            ieee802154_interrupt_route,
            etm,
        } = self.peripherals;
        let ieee802154_mac = crate::ieee802154::ownership::reunite(task_mac, interrupts.registers);
        Ieee802154Partition {
            peripherals: svd::peripheral_ownership::Ieee802154Peripherals {
                ieee802154_mac,
                ieee802154_interrupt_route,
            },
            etm,
        }
    }

    /// Order descriptor memory and MMIO at a hardware ownership boundary.
    pub fn order_device_accesses(&mut self) {
        device_fence();
    }
}

/// Task-side setup token before one IEEE 802.15.4 hard-IRQ epoch.
#[must_use = "the IEEE 802.15.4 interrupt setup must remain paired with its task owner"]
pub struct Ieee802154InterruptSetup {
    pub(crate) registers: crate::ieee802154::ownership::InterruptRegisters,
}

/// Disjoint IEEE 802.15.4 event/status capability for the hard ISR.
#[must_use = "the IEEE 802.15.4 interrupt owner must be deactivated and reunited"]
pub struct Ieee802154InterruptRegisters {
    pub(crate) registers: crate::ieee802154::ownership::InterruptRegisters,
}

#[cfg(test)]
pub(crate) mod test_support;
