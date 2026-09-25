//! Hardware ownership, capabilities, and affine lifecycle transitions.

use super::*;

/// Reviewed Bluetooth low-power timer divider for the main crystal source.
pub const BLUETOOTH_MAIN_XTAL_LOW_POWER_DIVIDER: ModemLowPowerClockDivider =
    match ModemLowPowerClockDivider::new(399) {
        Some(divider) => divider,
        None => panic!("reviewed Bluetooth low-power divider exceeds its PAC field"),
    };

/// Private Wi-Fi and shared-radio owners used by one Wi-Fi register set.
pub(crate) struct WifiRadioPeripheralOwners {
    pub(crate) wifi_mac: svd::peripheral_ownership::WifiMacPeripherals,
    pub(crate) ieee802154: svd::peripheral_ownership::Ieee802154Peripherals,
    pub(crate) radio_phy: RadioPhyRegisters,
    pub(crate) coexistence: svd::peripheral_ownership::CoexistencePeripherals,
    pub(crate) shared_radio: svd::peripheral_ownership::SharedRadioPeripherals,
}

/// Physical owners used by one IEEE 802.15.4 task register set.
///
/// The Bluetooth controller partition is intentionally nested behind the
/// BTBB boundary. ESP-IDF's public IEEE 802.15.4 enable order calls the shared
/// `esp_btbb_enable` lifecycle, but does not grant IEEE 802.15.4 authority over
/// the Bluetooth controller. Keeping the complete generated partition private
/// lets reviewed BTBB transactions be added without exposing BLE/EDR methods.
pub(crate) struct Ieee802154TaskPeripheralOwners {
    pub(crate) ieee802154_mac: crate::ieee802154::ownership::TaskRegisters,
    pub(crate) ieee802154_interrupt_route: svd::Ieee802154InterruptRoute,
    pub(crate) radio_phy: RadioPhyRegisters,
    pub(crate) coexistence: svd::peripheral_ownership::CoexistencePeripherals,
    pub(crate) btbb: Ieee802154BtbbPeripheralOwners,
}

/// Generated partitions retained behind the narrow IEEE 802.15.4 BTBB role.
pub(crate) struct Ieee802154BtbbPeripheralOwners {
    pub(crate) bluetooth: svd::peripheral_ownership::BluetoothControllerPeripherals,
    pub(crate) shared_radio: svd::peripheral_ownership::SharedRadioPeripherals,
}

/// Unique restricted owner of the shared radio-PHY register partition.
///
/// The type exposes only reviewed, named PHY transactions. It contains no
/// Wi-Fi MAC, Bluetooth controller, coexistence, or shared-baseband owner.
/// The HAL decides which protocol route currently holds it.
#[must_use = "the shared PHY owner must remain inside its active radio route"]
pub struct RadioPhyRegisters {
    pub(crate) peripherals: svd::peripheral_ownership::RadioPhyPeripherals,
    pub(crate) restore_slot: baseband::RadioPhyRestoreSlot,
    pub(crate) registration: RegistrationEpochState,
}

/// Identity of one PHY registration on the unique radio-PHY partition.
///
/// A registration issues a new epoch before it touches hardware. The epoch
/// stays current until another registration begins or the HAL returns the
/// partition to the neutral radio root. A registration result held apart from
/// its hardware
/// is valid for that hardware only while its epoch equals
/// [`RadioPhyRegisters::registration_epoch`].
///
/// The value is 32 bits wide so that the registered PHY owners carrying it
/// keep their size; those owners live inside reviewed async stack frames.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRegistrationEpoch(core::num::NonZeroU32);

/// Software registration bookkeeping carried with the physical partition.
///
/// One word holds the last issued epoch in its low 31 bits and whether that
/// epoch is current in its top bit, so every radio owner carries it without
/// growing.
pub(crate) struct RegistrationEpochState(u32);

impl RegistrationEpochState {
    const CURRENT: u32 = 1 << 31;
    const COUNTER: u32 = Self::CURRENT - 1;

    const fn new() -> Self {
        Self(0)
    }

    const fn issued(&self) -> u32 {
        self.0 & Self::COUNTER
    }
}

impl RadioPhyRegisters {
    /// Begin a registration and retire every earlier epoch of this partition.
    ///
    /// Call this before the first registration hardware edge: a failed or
    /// abandoned attempt has still changed the hardware that older results
    /// describe.
    pub fn begin_registration_epoch(&mut self) -> PhyRegistrationEpoch {
        // Each registration runs a full calibration graph, so 2^31 of them
        // cannot occur in one boot. Wrapping past zero keeps this total.
        let next = self.registration.issued().wrapping_add(1) & RegistrationEpochState::COUNTER;
        let issued = core::num::NonZeroU32::new(next).unwrap_or(core::num::NonZeroU32::MIN);
        self.registration = RegistrationEpochState(issued.get() | RegistrationEpochState::CURRENT);
        PhyRegistrationEpoch(issued)
    }

    /// The registration that currently describes this partition, if any.
    pub const fn registration_epoch(&self) -> Option<PhyRegistrationEpoch> {
        if self.registration.0 & RegistrationEpochState::CURRENT == 0 {
            return None;
        }
        match core::num::NonZeroU32::new(self.registration.issued()) {
            Some(issued) => Some(PhyRegistrationEpoch(issued)),
            None => None,
        }
    }

    /// Retire the current registration as the partition leaves its route.
    ///
    /// The HAL calls this whenever a route returns the partition to the
    /// neutral radio root.
    #[doc(hidden)]
    pub fn retire_registration_epoch(&mut self) {
        self.registration = RegistrationEpochState(self.registration.issued());
    }
}

/// Opaque Wi-Fi MAC register partition.
#[must_use = "dropping a radio partition permanently loses its register authority"]
pub struct WifiMacPartition(svd::peripheral_ownership::WifiMacPeripherals);

/// Opaque coexistence register partition.
#[must_use = "dropping a radio partition permanently loses its register authority"]
pub struct CoexistencePartition(svd::peripheral_ownership::CoexistencePeripherals);

/// Opaque Bluetooth controller register partition.
#[must_use = "dropping a radio partition permanently loses its register authority"]
pub struct BluetoothControllerPartition(svd::peripheral_ownership::BluetoothControllerPeripherals);

/// Opaque shared-baseband register partition.
#[must_use = "dropping a radio partition permanently loses its register authority"]
pub struct SharedRadioPartition(svd::peripheral_ownership::SharedRadioPeripherals);

/// Opaque IEEE 802.15.4 MAC and interrupt-route register partition.
#[must_use = "dropping a radio partition permanently loses its register authority"]
pub struct Ieee802154Partition(svd::peripheral_ownership::Ieee802154Peripherals);

/// Every reviewed ESP32-S31 radio register partition.
///
/// This is the sole production acquisition point. The partitions carry no
/// protocol route or lifecycle policy: the HAL composes them into register
/// sets and returns them here only as an ownership bundle.
///
/// ```compile_fail
/// use oer_esp32s31_pac::RadioPartitions;
///
/// let partitions = RadioPartitions::take().unwrap();
/// let _first = partitions.wifi_mac;
/// let _second = partitions.wifi_mac;
/// ```
#[must_use = "dropping the radio partitions permanently loses the unique hardware capability"]
pub struct RadioPartitions {
    pub wifi_mac: WifiMacPartition,
    pub wifi_interrupts: MacInterruptSetup,
    pub radio_phy: RadioPhyRegisters,
    pub coexistence: CoexistencePartition,
    pub bluetooth: BluetoothControllerPartition,
    pub bluetooth_modem_lp_timer: BluetoothModemLpTimerRegisters,
    pub bluetooth_interrupts: BluetoothInterruptSetup,
    pub shared_radio: SharedRadioPartition,
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
            wifi_mac,
            wifi_interrupts,
            radio_phy,
            coexistence,
            bluetooth,
            bluetooth_modem_lp_timer,
            bluetooth_interrupts,
            shared_radio,
            ieee802154,
        } = svd::peripheral_ownership::partition(peripherals);
        Self {
            wifi_mac: WifiMacPartition(wifi_mac),
            wifi_interrupts: MacInterruptSetup::from_peripherals(wifi_interrupts),
            radio_phy: RadioPhyRegisters {
                peripherals: radio_phy,
                restore_slot: baseband::RadioPhyRestoreSlot::new(),
                registration: RegistrationEpochState::new(),
            },
            coexistence: CoexistencePartition(coexistence),
            bluetooth: BluetoothControllerPartition(bluetooth),
            bluetooth_modem_lp_timer: BluetoothModemLpTimerRegisters::new(bluetooth_modem_lp_timer),
            bluetooth_interrupts: BluetoothInterruptSetup {
                peripherals: bluetooth_interrupts,
            },
            shared_radio: SharedRadioPartition(shared_radio),
            ieee802154: Ieee802154Partition(ieee802154),
        }
    }

    /// Construct every partition inside one isolated validation image.
    #[cfg(any(test, feature = "validation-probes"))]
    #[doc(hidden)]
    pub fn for_validation() -> Self {
        Self::from_peripherals(svd::peripheral_ownership::peripherals_for_validation())
    }
}

/// Semantic MAC work causes recovered from reviewed vendor transactions.
///
/// This type is deliberately not a register image. The generated PAC owns
/// STATUS field geometry; higher layers can only combine named causes.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MacInterruptEvents {
    pub(crate) tx_complete: bool,
    pub(crate) collision: bool,
    pub(crate) rx_success: bool,
    pub(crate) tx_timeout: bool,
}

impl MacInterruptEvents {
    pub const TX_COMPLETE: Self = Self::from_causes(true, false, false, false);
    pub const COLLISION: Self = Self::from_causes(false, true, false, false);
    pub const RX_SUCCESS: Self = Self::from_causes(false, false, true, false);
    pub const TX_TIMEOUT: Self = Self::from_causes(false, false, false, true);

    pub const fn empty() -> Self {
        Self::from_causes(false, false, false, false)
    }

    pub const fn from_causes(
        tx_complete: bool,
        collision: bool,
        rx_success: bool,
        tx_timeout: bool,
    ) -> Self {
        Self {
            tx_complete,
            collision,
            rx_success,
            tx_timeout,
        }
    }

    pub const fn union(self, other: Self) -> Self {
        Self::from_causes(
            self.tx_complete || other.tx_complete,
            self.collision || other.collision,
            self.rx_success || other.rx_success,
            self.tx_timeout || other.tx_timeout,
        )
    }

    pub const fn contains(self, other: Self) -> bool {
        (!other.tx_complete || self.tx_complete)
            && (!other.collision || self.collision)
            && (!other.rx_success || self.rx_success)
            && (!other.tx_timeout || self.tx_timeout)
    }

    pub const fn is_empty(self) -> bool {
        !self.tx_complete && !self.collision && !self.rx_success && !self.tx_timeout
    }

    pub const fn tx_complete(self) -> bool {
        self.tx_complete
    }

    pub const fn collision(self) -> bool {
        self.collision
    }

    pub const fn rx_success(self) -> bool {
        self.rx_success
    }

    pub const fn tx_timeout(self) -> bool {
        self.tx_timeout
    }
}

impl core::ops::BitOr for MacInterruptEvents {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        self.union(rhs)
    }
}

/// PAC-owned semantic partition of one sampled MAC interrupt image.
///
/// The raw W1C geometry stays private to this crate. Higher layers can route
/// qualified work, acknowledge-only auxiliary events, and opaque evidence
/// without applying masks to the sampled register themselves.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MacInterruptObservation {
    pub(crate) work_events: MacInterruptEvents,
    pub(crate) auxiliary_event: bool,
    pub(crate) unhandled_event: bool,
}

impl MacInterruptObservation {
    /// Construct semantic evidence for a host-side interrupt implementation.
    ///
    /// No register image enters this API: test doubles name the causes they
    /// simulate in the same vocabulary consumed by the MAC driver.
    pub const fn from_semantic_events(
        work_events: MacInterruptEvents,
        auxiliary_event: bool,
        unhandled_event: bool,
    ) -> Self {
        Self {
            work_events,
            auxiliary_event,
            unhandled_event,
        }
    }

    pub const fn is_empty(self) -> bool {
        self.work_events.is_empty() && !self.auxiliary_event && !self.unhandled_event
    }

    pub const fn work_events(self) -> MacInterruptEvents {
        self.work_events
    }

    pub const fn has_auxiliary_event(self) -> bool {
        self.auxiliary_event
    }

    pub const fn has_unhandled_event(self) -> bool {
        self.unhandled_event
    }
}

/// One sampled MAC interrupt image which can be acknowledged exactly once.
pub struct MacInterruptSnapshot(pub(crate) svd::interrupt_snapshot::MacInterruptSnapshot);

impl MacInterruptSnapshot {
    pub fn observation(&self) -> MacInterruptObservation {
        let work_events = MacInterruptEvents::from_causes(
            self.0.tx_complete(),
            self.0.bss_color_collision(),
            self.0.rx_success(),
            self.0.tx_timeout(),
        );
        let auxiliary_event =
            self.0.rx_associated_auxiliary_5() || self.0.rx_associated_auxiliary_24();
        let unhandled_event = self.0.unknown_0_4() != 0
            || self.0.cold_rx_enable_6_unknown()
            || self.0.unknown_9_10() != 0
            || self.0.watchdog()
            || self.0.cold_rx_enable_12_unknown()
            || self.0.cold_rx_enable_13_unknown()
            || self.0.sta_beacon_filter()
            || self.0.unknown_16_18() != 0
            || self.0.unknown_20()
            || self.0.cold_rx_enable_21_unknown()
            || self.0.unknown_22()
            || self.0.cold_rx_enable_23_unknown()
            || self.0.unknown_25_26() != 0
            || self.0.cold_rx_enable_27_unknown()
            || self.0.cold_rx_enable_28_unknown()
            || self.0.unknown_29_31() != 0;
        MacInterruptObservation::from_semantic_events(work_events, auxiliary_event, unhandled_event)
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_bits(&self) -> u32 {
        self.0.bits()
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn for_validation(bits: u32) -> Self {
        Self(svd::interrupt_snapshot::mac_interrupt_for_validation(bits))
    }
}

/// One sampled power-interrupt image with intentionally opaque bit semantics.
pub struct MacPowerInterruptSnapshot(pub(crate) svd::interrupt_snapshot::MacPowerInterruptSnapshot);

impl MacPowerInterruptSnapshot {
    pub fn observation(&self) -> MacPowerInterruptObservation {
        MacPowerInterruptObservation::from_semantic_events(
            self.0.tsf_timer_0(),
            self.0.tsf_timer_1(),
            self.0.tsf_timer_2(),
            self.0.tsf_timer_3(),
            self.0.unknown_0_3() != 0 || self.0.unknown_8_31() != 0,
        )
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_bits(&self) -> u32 {
        self.0.bits()
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn for_validation(bits: u32) -> Self {
        Self(svd::interrupt_snapshot::mac_power_interrupt_for_validation(
            bits,
        ))
    }
}

/// Semantic WDEVPWR causes sampled through generated PAC field accessors.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MacPowerInterruptObservation(pub(crate) u8);

impl MacPowerInterruptObservation {
    // Driver-local semantic flags. Their positions intentionally do not match
    // WDEVPWR register geometry, which remains behind generated accessors.
    pub(crate) const TSF_TIMER_0: u8 = 0x01;
    pub(crate) const TSF_TIMER_1: u8 = 0x02;
    pub(crate) const TSF_TIMER_2: u8 = 0x04;
    pub(crate) const TSF_TIMER_3: u8 = 0x08;
    pub(crate) const UNHANDLED_EVENT: u8 = 0x10;

    pub const fn from_semantic_events(
        tsf_timer_0: bool,
        tsf_timer_1: bool,
        tsf_timer_2: bool,
        tsf_timer_3: bool,
        unhandled_event: bool,
    ) -> Self {
        Self(
            (if tsf_timer_0 { Self::TSF_TIMER_0 } else { 0 })
                | (if tsf_timer_1 { Self::TSF_TIMER_1 } else { 0 })
                | (if tsf_timer_2 { Self::TSF_TIMER_2 } else { 0 })
                | (if tsf_timer_3 { Self::TSF_TIMER_3 } else { 0 })
                | (if unhandled_event {
                    Self::UNHANDLED_EVENT
                } else {
                    0
                }),
        )
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn tsf_timer_0(self) -> bool {
        self.0 & Self::TSF_TIMER_0 != 0
    }

    pub const fn tsf_timer_1(self) -> bool {
        self.0 & Self::TSF_TIMER_1 != 0
    }

    pub const fn tsf_timer_2(self) -> bool {
        self.0 & Self::TSF_TIMER_2 != 0
    }

    pub const fn tsf_timer_3(self) -> bool {
        self.0 & Self::TSF_TIMER_3 != 0
    }

    pub const fn has_unhandled_event(self) -> bool {
        self.0 & Self::UNHANDLED_EVENT != 0
    }
}

#[inline]
pub(crate) fn device_fence() {
    svd::device_access::fence();
}

/// Unique logical owner of the ESP32-S31 radio register regions after cold
/// MAC initialization has completed.
///
/// The generated [`svd::Peripherals`] singleton is kept private. This running
/// owner deliberately has no typed access to the MAC interrupt enable/clear or
/// WDEVPWR status/clear transactions. Those disjoint banks belong to
/// [`MacInterruptSetup`] and then to [`MacInterruptRegisters`] plus
/// [`MacPowerInterruptRegisters`].
///
/// Raw PAC types are deliberately not part of this crate's public API:
///
/// ```compile_fail
/// use oer_esp32s31_pac::svd;
/// ```
///
/// No address-bearing register catalog is exposed:
///
/// ```compile_fail
/// use oer_esp32s31_pac::Register32;
///
/// let forged = Register32::new(0x2010_4000);
/// ```
///
/// Finally, the owner has no generic address/value escape hatch. Every
/// writable transaction must be an explicitly reviewed capability:
///
/// ```compile_fail
/// use oer_esp32s31_pac::WifiRadioRegisters;
///
/// let unreviewed_write = WifiRadioRegisters::write_register;
/// ```
pub struct WifiRadioRegisters {
    pub(crate) peripherals: WifiRadioPeripheralOwners,
    pub(crate) station_tbtt_wake_prepared: bool,
    pub(crate) station_modem_wakeup: crate::wifi::mac::modem_wakeup::StaModemWakeOwnership,
}

/// Partitions consumed by one Wi-Fi register set.
pub struct WifiRadioParts {
    pub wifi_mac: WifiMacPartition,
    pub ieee802154: Ieee802154Partition,
    pub radio_phy: RadioPhyRegisters,
    pub coexistence: CoexistencePartition,
    pub shared_radio: SharedRadioPartition,
}

impl WifiRadioRegisters {
    /// Assemble the Wi-Fi register set. This performs no MMIO.
    pub fn new(parts: WifiRadioParts) -> Self {
        let WifiRadioParts {
            wifi_mac: WifiMacPartition(wifi_mac),
            ieee802154: Ieee802154Partition(ieee802154),
            radio_phy,
            coexistence: CoexistencePartition(coexistence),
            shared_radio: SharedRadioPartition(shared_radio),
        } = parts;
        Self {
            peripherals: WifiRadioPeripheralOwners {
                wifi_mac,
                ieee802154,
                radio_phy,
                coexistence,
                shared_radio,
            },
            station_tbtt_wake_prepared: false,
            station_modem_wakeup: crate::wifi::mac::modem_wakeup::StaModemWakeOwnership::new(),
        }
    }

    /// Return the partitions. This performs no MMIO.
    pub fn into_parts(self) -> WifiRadioParts {
        let WifiRadioPeripheralOwners {
            wifi_mac,
            ieee802154,
            radio_phy,
            coexistence,
            shared_radio,
        } = self.peripherals;
        WifiRadioParts {
            wifi_mac: WifiMacPartition(wifi_mac),
            ieee802154: Ieee802154Partition(ieee802154),
            radio_phy,
            coexistence: CoexistencePartition(coexistence),
            shared_radio: SharedRadioPartition(shared_radio),
        }
    }

    /// Prepare the shared state maps and retain the PHY-I2C clock for this
    /// complete Wi-Fi route epoch.
    pub fn prepare_shared_modem_clock_map(&mut self) {
        self.peripherals.radio_phy.prepare_shared_modem_clock_map();
    }

    /// Read the route-owned shared clock checkpoint.
    pub fn shared_modem_clock_observation(&self) -> SharedModemClockObservation {
        self.peripherals.radio_phy.shared_modem_clock_observation()
    }

    /// Preserve the vendor two-read coexistence clock sampling rule.
    #[doc(hidden)]
    pub fn sample_coexistence_low_power_clock(
        &self,
    ) -> Option<CoexistenceLowPowerClockObservation> {
        self.peripherals
            .radio_phy
            .sample_coexistence_low_power_clock()
    }

    /// Borrow the shared PHY component without exposing another owner.
    #[doc(hidden)]
    pub fn radio_phy(&self) -> &RadioPhyRegisters {
        &self.peripherals.radio_phy
    }

    /// Mutably borrow the shared PHY component without exposing another owner.
    #[doc(hidden)]
    pub fn radio_phy_mut(&mut self) -> &mut RadioPhyRegisters {
        &mut self.peripherals.radio_phy
    }

    /// Order descriptor memory and MMIO at a hardware ownership boundary.
    pub fn order_device_accesses(&mut self) {
        device_fence();
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) const fn contains(address: usize) -> bool {
        // The host-only catalog is limited to the custom modem/radio aperture.
        matches!(address, 0x2010_0000..=0x2010_ffff)
    }
}

/// Complete register ownership for one exclusive IEEE 802.15.4 epoch.
///
/// Raw generated partitions remain private. The public surface is extended
/// only with reviewed MAC, common-PHY, BTBB, and coexistence transactions;
/// Wi-Fi and Bluetooth-controller operations cannot be reached through this
/// role even though their generated owners must be retained for a lossless
/// protocol switch.
#[must_use = "the IEEE 802.15.4 radio owner must be released as one epoch"]
pub struct Ieee802154TaskRegisters {
    pub(crate) peripherals: Ieee802154TaskPeripheralOwners,
}

/// Partitions consumed by one IEEE 802.15.4 task register set.
pub struct Ieee802154TaskParts {
    pub ieee802154: Ieee802154Partition,
    pub radio_phy: RadioPhyRegisters,
    pub coexistence: CoexistencePartition,
    pub bluetooth: BluetoothControllerPartition,
    pub shared_radio: SharedRadioPartition,
}

impl Ieee802154TaskRegisters {
    /// Assemble the task register set and split the shared MAC block into its
    /// disjoint task and interrupt owners. This performs no MMIO.
    pub fn new(parts: Ieee802154TaskParts) -> (Self, Ieee802154InterruptSetup) {
        let Ieee802154TaskParts {
            ieee802154: Ieee802154Partition(ieee802154),
            radio_phy,
            coexistence: CoexistencePartition(coexistence),
            bluetooth: BluetoothControllerPartition(bluetooth),
            shared_radio: SharedRadioPartition(shared_radio),
        } = parts;
        let svd::peripheral_ownership::Ieee802154Peripherals {
            ieee802154_mac,
            ieee802154_interrupt_route,
        } = ieee802154;
        let (task_mac, interrupt_mac) = crate::ieee802154::ownership::split(ieee802154_mac);
        (
            Self {
                peripherals: Ieee802154TaskPeripheralOwners {
                    ieee802154_mac: task_mac,
                    ieee802154_interrupt_route,
                    radio_phy,
                    coexistence,
                    btbb: Ieee802154BtbbPeripheralOwners {
                        bluetooth,
                        shared_radio,
                    },
                },
            },
            Ieee802154InterruptSetup {
                registers: interrupt_mac,
            },
        )
    }

    /// Reunite the MAC block with its inactive interrupt owner and return the
    /// partitions. This performs no MMIO.
    pub fn into_parts(self, interrupts: Ieee802154InterruptSetup) -> Ieee802154TaskParts {
        let Ieee802154TaskPeripheralOwners {
            ieee802154_mac: task_mac,
            ieee802154_interrupt_route,
            radio_phy,
            coexistence,
            btbb:
                Ieee802154BtbbPeripheralOwners {
                    bluetooth,
                    shared_radio,
                },
        } = self.peripherals;
        let ieee802154_mac = crate::ieee802154::ownership::reunite(task_mac, interrupts.registers);
        Ieee802154TaskParts {
            ieee802154: Ieee802154Partition(svd::peripheral_ownership::Ieee802154Peripherals {
                ieee802154_mac,
                ieee802154_interrupt_route,
            }),
            radio_phy,
            coexistence: CoexistencePartition(coexistence),
            bluetooth: BluetoothControllerPartition(bluetooth),
            shared_radio: SharedRadioPartition(shared_radio),
        }
    }

    pub fn prepare_shared_modem_clock_map(&mut self) {
        self.peripherals.radio_phy.prepare_shared_modem_clock_map();
    }
    pub fn shared_modem_clock_observation(&self) -> SharedModemClockObservation {
        self.peripherals.radio_phy.shared_modem_clock_observation()
    }

    #[doc(hidden)]
    pub fn select_hp_active_modem_icg(&mut self) {
        self.peripherals.radio_phy.select_hp_active_modem_icg();
    }

    #[doc(hidden)]
    pub fn apply_modem_icg_selection(&mut self) {
        self.peripherals.radio_phy.apply_modem_icg_selection();
    }

    #[doc(hidden)]
    pub fn apply_sleep_icg_selection(&mut self) {
        self.peripherals.radio_phy.apply_sleep_icg_selection();
    }

    #[doc(hidden)]
    pub fn enable_modem_register_bus_clock(&mut self) {
        self.peripherals.radio_phy.enable_modem_register_bus_clock();
    }

    #[doc(hidden)]
    pub fn configure_modem_source_clocks(&mut self) {
        self.peripherals.radio_phy.configure_modem_source_clocks();
    }

    #[doc(hidden)]
    pub fn platform_clock_power_observation(&self) -> PlatformClockPowerObservation {
        self.peripherals
            .radio_phy
            .platform_clock_power_observation()
    }

    #[doc(hidden)]
    pub fn set_wifi_baseband_and_mac_reset(&mut self, asserted: bool) {
        self.peripherals
            .radio_phy
            .set_wifi_baseband_and_mac_reset(asserted);
    }

    #[doc(hidden)]
    pub fn set_wifi_baseband_reset(&mut self, asserted: bool) {
        self.peripherals.radio_phy.set_wifi_baseband_reset(asserted);
    }

    #[doc(hidden)]
    pub fn configure_wifi_power_clock_map(&mut self) {
        self.peripherals.radio_phy.configure_wifi_power_clock_map();
    }

    #[doc(hidden)]
    pub fn enable_phy_calibration_clocks(&mut self) {
        self.peripherals.radio_phy.enable_phy_calibration_clocks();
    }

    #[doc(hidden)]
    pub fn select_phy_i2c_160mhz_source(&mut self) {
        self.peripherals.radio_phy.select_phy_i2c_160mhz_source();
    }

    #[doc(hidden)]
    pub fn modem_syscon_power_observation(&self) -> ModemSysconPowerObservation {
        self.peripherals.radio_phy.modem_syscon_power_observation()
    }

    #[doc(hidden)]
    pub fn configure_modem_syscon_clock_maps(&mut self) {
        self.peripherals
            .radio_phy
            .configure_ieee802154_modem_clock_maps();
    }

    #[doc(hidden)]
    pub fn enable_ieee802154_wifi_bb_clock(&mut self) {
        self.peripherals.radio_phy.enable_ieee802154_wifi_bb_clock();
    }

    #[doc(hidden)]
    pub fn enable_ieee802154_etm_clock(&mut self) {
        self.peripherals.radio_phy.enable_ieee802154_etm_clock();
    }

    #[doc(hidden)]
    pub fn enable_ieee802154_bt_apb_clocks(&mut self) {
        self.peripherals.radio_phy.enable_ieee802154_bt_apb_clocks();
    }

    #[doc(hidden)]
    pub fn enable_ieee802154_common_baseband_clock(&mut self) {
        self.peripherals
            .radio_phy
            .enable_ieee802154_common_baseband_clock();
    }

    #[doc(hidden)]
    pub fn enable_ieee802154_mac_clocks(&mut self) {
        self.peripherals.radio_phy.enable_ieee802154_mac_clocks();
    }

    #[doc(hidden)]
    pub fn modem_syscon_ieee802154_clock_observation(
        &self,
    ) -> ModemSysconIeee802154ClockObservation {
        self.peripherals.radio_phy.ieee802154_clock_observation()
    }

    #[doc(hidden)]
    pub fn set_ieee802154_mac_reset(&mut self, asserted: bool) {
        self.peripherals
            .radio_phy
            .set_ieee802154_mac_reset(asserted);
    }

    #[doc(hidden)]
    pub fn set_ieee802154_apb_reset(&mut self, asserted: bool) {
        self.peripherals
            .radio_phy
            .set_ieee802154_apb_reset(asserted);
    }

    #[doc(hidden)]
    pub fn modem_syscon_ieee802154_reset_observation(
        &self,
    ) -> ModemSysconIeee802154ResetObservation {
        self.peripherals.radio_phy.ieee802154_reset_observation()
    }
    /// Borrow the protocol-neutral PHY partition without creating another
    /// owner.
    #[doc(hidden)]
    pub const fn radio_phy(&self) -> &RadioPhyRegisters {
        &self.peripherals.radio_phy
    }

    /// Mutably borrow the protocol-neutral PHY partition without creating
    /// another owner.
    #[doc(hidden)]
    pub fn radio_phy_mut(&mut self) -> &mut RadioPhyRegisters {
        &mut self.peripherals.radio_phy
    }

    /// Order descriptor memory and MMIO at a hardware ownership boundary.
    pub fn order_device_accesses(&mut self) {
        device_fence();
    }
}

/// Task-side setup token before one IEEE 802.15.4 hard-IRQ epoch.
///
/// The raw interrupt handle is already disjoint from
/// [`Ieee802154TaskRegisters`], but remains inactive until the reviewed setup
/// transaction consumes this value.
#[must_use = "the IEEE 802.15.4 interrupt setup must remain paired with its task owner"]
pub struct Ieee802154InterruptSetup {
    pub(crate) registers: crate::ieee802154::ownership::InterruptRegisters,
}

/// Disjoint IEEE 802.15.4 event/status capability for the hard ISR.
///
/// Task command, DMA, policy, and event-enable operations are absent from this
/// type. It must be deactivated after the platform CPU route is disabled and
/// reunited with the task owner before the whole radio can be released.
#[must_use = "the IEEE 802.15.4 interrupt owner must be deactivated and reunited"]
pub struct Ieee802154InterruptRegisters {
    pub(crate) registers: crate::ieee802154::ownership::InterruptRegisters,
}

/// Ordinary task-side owner for one exclusive standalone Bluetooth route.
///
/// Wi-Fi and all shared resources remain retained privately. Methods on this
/// owner are individually reviewed register transactions; possessing it does
/// not itself prove that common PHY, BTBB or controller lifecycle prerequisites
/// have run.
#[must_use = "the Bluetooth task owner must be reunited before release"]
pub struct BluetoothTaskRegisters {
    pub(crate) bluetooth: svd::peripheral_ownership::BluetoothControllerPeripherals,
    pub(crate) modem_lp_timer: Option<BluetoothModemLpTimerRegisters>,
    pub(crate) radio_phy: RadioPhyRegisters,
    pub(crate) coexistence: svd::peripheral_ownership::CoexistencePeripherals,
    pub(crate) shared_radio: svd::peripheral_ownership::SharedRadioPeripherals,
    pub(crate) controller_time_latch:
        crate::bluetooth::controller::time::BluetoothControllerTimeLatchOwnership,
}

/// Partitions consumed by one Bluetooth task register set.
pub struct BluetoothTaskParts {
    pub bluetooth: BluetoothControllerPartition,
    pub modem_lp_timer: BluetoothModemLpTimerRegisters,
    pub radio_phy: RadioPhyRegisters,
    pub coexistence: CoexistencePartition,
    pub shared_radio: SharedRadioPartition,
}

impl BluetoothTaskRegisters {
    /// Assemble the Bluetooth task register set. This performs no MMIO.
    pub fn new(parts: BluetoothTaskParts) -> Self {
        let BluetoothTaskParts {
            bluetooth: BluetoothControllerPartition(bluetooth),
            modem_lp_timer,
            radio_phy,
            coexistence: CoexistencePartition(coexistence),
            shared_radio: SharedRadioPartition(shared_radio),
        } = parts;
        Self {
            bluetooth,
            modem_lp_timer: Some(modem_lp_timer),
            radio_phy,
            coexistence,
            shared_radio,
            controller_time_latch:
                crate::bluetooth::controller::time::BluetoothControllerTimeLatchOwnership::new(),
        }
    }

    /// Return the partitions. This performs no MMIO.
    ///
    /// # Panics
    ///
    /// Panics while the modem LP-timer partition is separated; check
    /// [`Self::modem_lp_timer_separated`] first.
    pub fn into_parts(mut self) -> BluetoothTaskParts {
        let modem_lp_timer = self
            .modem_lp_timer
            .take()
            .expect("a cold Bluetooth owner retains its modem LP-timer partition");
        BluetoothTaskParts {
            bluetooth: BluetoothControllerPartition(self.bluetooth),
            modem_lp_timer,
            radio_phy: self.radio_phy,
            coexistence: CoexistencePartition(self.coexistence),
            shared_radio: SharedRadioPartition(self.shared_radio),
        }
    }

    /// Whether source 127 still owns the disjoint modem LP-timer partition.
    pub const fn modem_lp_timer_separated(&self) -> bool {
        self.modem_lp_timer.is_none()
    }

    /// Return the drained LP-timer partition during physical shutdown.
    #[doc(hidden)]
    pub fn restore_modem_lp_timer(&mut self, timer: BluetoothModemLpTimerRegisters) {
        assert!(
            self.modem_lp_timer.is_none(),
            "the Bluetooth task already owns its modem LP-timer partition"
        );
        self.modem_lp_timer = Some(timer);
    }

    #[doc(hidden)]
    pub fn select_hp_active_modem_icg(&mut self) {
        self.radio_phy.select_hp_active_modem_icg();
    }

    #[doc(hidden)]
    pub fn apply_modem_icg_selection(&mut self) {
        self.radio_phy.apply_modem_icg_selection();
    }

    #[doc(hidden)]
    pub fn apply_sleep_icg_selection(&mut self) {
        self.radio_phy.apply_sleep_icg_selection();
    }

    #[doc(hidden)]
    pub fn enable_modem_register_bus_clock(&mut self) {
        self.radio_phy.enable_modem_register_bus_clock();
    }

    #[doc(hidden)]
    pub fn configure_modem_source_clocks(&mut self) {
        self.radio_phy.configure_modem_source_clocks();
    }

    #[doc(hidden)]
    pub fn set_wifi_baseband_and_mac_reset(&mut self, asserted: bool) {
        self.radio_phy.set_wifi_baseband_and_mac_reset(asserted);
    }

    #[doc(hidden)]
    pub fn set_wifi_baseband_reset(&mut self, asserted: bool) {
        self.radio_phy.set_wifi_baseband_reset(asserted);
    }

    #[doc(hidden)]
    pub fn configure_wifi_power_clock_map(&mut self) {
        self.radio_phy.configure_wifi_power_clock_map();
    }

    #[doc(hidden)]
    pub fn enable_phy_calibration_clocks(&mut self) {
        self.radio_phy.enable_phy_calibration_clocks();
    }

    #[doc(hidden)]
    pub fn select_phy_i2c_160mhz_source(&mut self) {
        self.radio_phy.select_phy_i2c_160mhz_source();
    }

    #[doc(hidden)]
    pub fn platform_clock_power_observation(&self) -> PlatformClockPowerObservation {
        self.radio_phy.platform_clock_power_observation()
    }

    #[doc(hidden)]
    pub fn modem_syscon_power_observation(&self) -> ModemSysconPowerObservation {
        self.radio_phy.modem_syscon_power_observation()
    }

    #[doc(hidden)]
    pub fn shared_modem_clock_observation(&self) -> SharedModemClockObservation {
        self.radio_phy.shared_modem_clock_observation()
    }

    #[doc(hidden)]
    pub fn prepare_shared_modem_clock_map(&mut self) {
        self.radio_phy.prepare_shared_modem_clock_map();
    }

    #[doc(hidden)]
    pub fn bluetooth_shared_clock_observation(
        &self,
    ) -> (
        SharedModemClockObservation,
        BluetoothLowPowerClockObservation,
    ) {
        (
            self.radio_phy.shared_modem_clock_observation(),
            self.radio_phy.bluetooth_low_power_clock_observation(),
        )
    }

    /// Reset the Bluetooth controller domains and fence the reset edge.
    #[doc(hidden)]
    pub fn reset_controller_domains(&mut self) {
        self.radio_phy.reset_bluetooth_controller_domains();
        device_fence();
    }

    /// Whether the Bluetooth controller domain resets read back released.
    pub fn controller_resets_released(&self) -> bool {
        self.radio_phy
            .bluetooth_clock_observation()
            .controller_resets_released
    }

    /// Borrow the protocol-neutral PHY partition for one read-only HAL scope.
    #[doc(hidden)]
    pub const fn radio_phy(&self) -> &RadioPhyRegisters {
        &self.radio_phy
    }

    /// Borrow the protocol-neutral PHY partition for one named HAL scope.
    ///
    /// The returned owner cannot be taken, released, or retained after this
    /// task owner is reunited with its interrupt bank.
    #[doc(hidden)]
    pub fn radio_phy_mut(&mut self) -> &mut RadioPhyRegisters {
        &mut self.radio_phy
    }
}

/// Inactive owner of the reviewed Bluetooth interrupt partition.
#[must_use = "the Bluetooth interrupt setup must remain paired with its task owner"]
pub struct BluetoothInterruptSetup {
    pub(crate) peripherals: svd::peripheral_ownership::BluetoothInterruptPeripherals,
}

/// Bluetooth interrupt-bank capability staged for a future powered ISR epoch.
///
/// [`BluetoothInterruptOutputPrepared::stage_for_cpu_routes`] constructs this
/// value only after the reviewed baseline masks and controller output have
/// been prepared. The platform must retain it in stable storage shared by the
/// primary and NRT handlers before enabling either CPU route.
#[must_use = "the Bluetooth interrupt owner must be deactivated and reunited"]
pub struct BluetoothInterruptRegisters {
    pub(crate) peripherals: svd::peripheral_ownership::BluetoothInterruptPeripherals,
}

#[cfg(test)]
pub(crate) mod test_support;

#[cfg(test)]
mod tests;
