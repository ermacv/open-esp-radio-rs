#![no_std]
#![deny(unsafe_code)]

#[cfg(test)]
extern crate std;

mod agc;
mod agc_runtime;
mod baseband;
mod bluetooth_baseband;
mod bluetooth_controller_hal_init;
mod bluetooth_interrupt;
mod bluetooth_memory_lists;
mod bluetooth_phy_init;
mod bluetooth_scheduler;
mod bluetooth_scheduler_runtime;
mod cfr;
pub mod clock;
mod coex;
mod frequency;
mod generated;
mod ieee802154;
mod iq_estimator;
#[cfg(test)]
#[doc(hidden)]
#[allow(dead_code)]
pub mod mac;
mod mac_antenna_init;
mod mac_block_ack;
mod mac_channel;
mod mac_coex_init;
mod mac_coex_runtime;
mod mac_cold_start;
mod mac_crypto;
mod mac_enable;
mod mac_hal_init_tail;
mod mac_he_beamforming;
mod mac_he_init;
mod mac_he_init_suffix;
mod mac_he_ofdma;
mod mac_he_peer;
mod mac_he_tb;
mod mac_interface_address;
mod mac_interrupt;
mod mac_last_rx_buffer;
mod mac_modem_wakeup;
mod mac_rx_dma;
mod mac_rx_policy;
mod mac_rx_statistics;
mod mac_sniffer;
mod mac_softap_tsf;
mod mac_tsf;
mod mac_tx;
mod mac_tx_power_init;
mod mac_tx_queue;
mod mac_txrx_init;
pub mod pbus;
pub mod phy;
pub mod phy_i2c;
mod table_memory;
#[cfg(feature = "validation-probes")]
pub mod validation;
#[cfg(feature = "validation-probes")]
mod validation_transactions;
pub use agc_runtime::ForcedRxGain;
pub use bluetooth_controller_hal_init::{
    BluetoothControllerHalInitConfig, BluetoothHalInitPeriod, BluetoothHalInitScale,
};
pub use bluetooth_interrupt::{
    BLUETOOTH_PRIMARY_BASELINE_BANK_0_MASK, BLUETOOTH_PRIMARY_BASELINE_BANK_1_MASK,
    BLUETOOTH_PRIMARY_DYNAMIC_BANK_0_MASK, BLUETOOTH_PRIMARY_DYNAMIC_BANK_1_MASK,
    BluetoothInterruptOutputPrepared, BluetoothNrtInterruptObservation,
    BluetoothPrimaryFaultEvidence, BluetoothPrimaryInterruptEpoch,
    BluetoothPrimaryInterruptObservation,
};
pub use bluetooth_memory_lists::{
    BluetoothControllerSramAddress, BluetoothControllerSramAddressError,
    BluetoothMemoryListPointerImage, BluetoothMemoryListSelector, BluetoothMemoryListSlot,
};
pub use bluetooth_scheduler_runtime::{
    BluetoothSchedulerDiagnosticObservation, BluetoothSchedulerReferenceGateObservation,
    BluetoothSchedulerWorkObservation,
};
pub use cfr::CfrValue;
pub use coex::{COEX_TIMER_COUNT, CoexTimerRegister};
/// Reviewed writable MAC interrupt mask.
///
/// The generated domain deliberately has no public integer constructor:
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_pac::MacInterruptMask;
///
/// let invented = MacInterruptMask(0xdead_beef);
/// ```
pub use generated::MacInterruptMask;
pub use generated::{
    CoexTimerClientValue, CoexTimerPtiValue, CoexTimerTickImage,
    MacExtraSoftApRxBlockAckEntryIndex, MacInterface, MacItwtClearIndex, MacKeyEntryIndex, MacPti,
    MacRxBlockAckEntryIndex, MacRxBlockAckStartingSequence, MacRxBlockAckTid, MacRxBlockAckWindow,
    MacTxPtiCount, MacTxQueueIndex,
};
#[doc(hidden)]
pub use ieee802154::Ieee802154RegisterLease;
#[cfg(feature = "validation-probes")]
#[doc(hidden)]
pub use ieee802154::Ieee802154RouteRawReadback;
pub use ieee802154::{
    Ieee802154AckTimeoutUnits, Ieee802154CcaMode, Ieee802154FoundationSnapshot,
    Ieee802154FrequencyCode, Ieee802154MacControl, Ieee802154MacPolicySnapshot,
    Ieee802154PanIdentity, Ieee802154Pti, Ieee802154RxStateCode, Ieee802154StateSnapshot,
    Ieee802154TxStateCode,
};
pub use mac_block_ack::{
    ExtraSoftApRxBlockAckEntrySnapshot, InternalTxBlockAckSnapshot, RxBlockAckEntrySnapshot,
    TxBlockAckDiagnosticSnapshot, TxBlockAckPayload, TxBlockAckRegisterImage,
};
pub use mac_crypto::MacKeyInstallOutcome;
pub use mac_he_beamforming::{
    MacHeBeamformingReportProfile, MacHeBeamformingReportProfileError, MacHeErSuAckRateProfile,
};
pub use mac_he_init_suffix::MacHeTxMpduLengthLink;
pub use mac_he_ofdma::{
    MacBeamformingAverageSnr, MacHeBeamformingConfigurationSnapshot, MacHeBeamformingDiagnostics,
    MacHeBufferStatusSnapshot, MacHeCustomReceiveType, MacHeEdcaQueueConfiguration,
    MacHeMuEdcaTimerSnapshot, MacHeQueueSchedulingSnapshot, MacHeReceiveConfigurationSnapshot,
    MacHeRxPowerSaveSnapshot, MacHeTbLinkReservation, MacHeTbProgramError, MacHeTbTidLimit,
    MacHeTid, MacHeTriggerQueueConfiguration, MacHeTriggerRxDiagnostics,
    MacHeTriggerTxQueueSnapshot,
};
pub use mac_he_peer::{MacHe20PeerConfig, MacHe20PeerError};
pub use mac_he_tb::{MacHeTbStatistics, MacHeTbTxDiagnostics};
pub use mac_interrupt::{
    ConnectedStaWithoutPowerSavePrepared, MacInterruptRegisters, MacInterruptSetup,
    MacPowerInterruptRegisters, MacPowerWakeCause, MacTsfTimerIndex,
};
pub use mac_modem_wakeup::{
    StaBeaconMissLimit, StaBeaconMissTimeoutRaw, StaModemSleepLimit, StaModemWakeConfig,
    StaModemWakePrepareError, StaModemWakeRestore, StaModemWakeRestoreError,
    StaModemWakeRestoreFailure, StaTbttAutoPeriod, StaWakeProtectEarlyTimeRaw,
};
pub use mac_rx_dma::MacRxDmaSnapshot;
pub use mac_rx_policy::{
    MacApReceivePolicySnapshot, MacRoleReceivePolicy, MacStaApReceivePlan, MacStaPolicyMode,
    MacStaReceivePolicySnapshot,
};
pub use mac_rx_statistics::{
    MacHeColorCollisionSnapshot, MacRxDecodeErrorStatistics, MacRxDecodeErrorStatisticsDelta,
    MacRxHangStatistics, MacRxHangStatisticsDelta, MacRxPrimaryStatistics,
    MacRxPrimaryStatisticsDelta, MacRxStatisticsSnapshot,
};
pub use mac_tsf::{
    StaTbttWakePrepareError, StaTbttWakeRestore, StaTbttWakeRestoreError, StaTbttWakeRestoreFailure,
};
pub use mac_tx::{
    MacHeTxProgram, MacHeTxVectorSnapshot, MacHtAmpduCompletionRegisters, MacHtTxProgram,
    MacLegacyTxProgram, MacTxCompletionRegisters, MacTxDetachOutcome, MacTxDetachReason,
    MacTxPtiProgram, MacTxQueueDetached,
};
pub use mac_tx_power_init::{
    MAC_TX_POWER_RATE_COUNT, MacPartialRuPowerSelector, MacTxPowerIndex, MacTxPowerPair,
    MacTxPowerTable,
};
use open_esp_radio_esp32s31_pac_raw as svd;
pub use table_memory::{PbusMemoryGroupBoundary, PhyMemoryError};

/// Private Wi-Fi and shared-radio owners used by one exclusive Wi-Fi route.
struct WifiRadioPeripheralOwners {
    wifi_mac: svd::peripheral_ownership::WifiMacPeripherals,
    ieee802154_mac: svd::Ieee802154Mac,
    radio_phy: RadioPhyRegisters,
    coexistence: svd::peripheral_ownership::CoexistencePeripherals,
    shared_radio: svd::peripheral_ownership::SharedRadioPeripherals,
}

/// Unique restricted owner of the shared radio-PHY register partition.
///
/// This component is created only while the complete [`RadioHardware`] root
/// is routed into Wi-Fi or Bluetooth. It has no acquisition or release API:
/// callers can only borrow it through the active protocol route, so the
/// physical PHY partition can never outlive or diverge from that route.
///
/// The type exposes only reviewed, named PHY transactions. It contains no
/// Wi-Fi MAC, Bluetooth controller, coexistence, or shared-baseband owner.
#[must_use = "the shared PHY owner must remain inside its active radio route"]
pub struct RadioPhyRegisters {
    peripherals: svd::peripheral_ownership::RadioPhyPeripherals,
}

/// Bluetooth owners retained, but not exposed, while Wi-Fi is exclusive.
struct RetainedBluetoothPeripheralOwners {
    bluetooth: svd::peripheral_ownership::BluetoothControllerPeripherals,
    bluetooth_interrupts: svd::peripheral_ownership::BluetoothInterruptPeripherals,
}

/// Wi-Fi owners retained, but not exposed, while Bluetooth is exclusive.
struct RetainedWifiPeripheralOwners {
    wifi_mac: svd::peripheral_ownership::WifiMacPeripherals,
    wifi_interrupts: svd::peripheral_ownership::WifiInterruptPeripherals,
    ieee802154: svd::peripheral_ownership::Ieee802154Peripherals,
}

/// Unique protocol-neutral owner of every reviewed ESP32-S31 radio region.
///
/// This is the sole production acquisition root. It can be consumed by
/// exactly one standalone protocol route; neither route can manufacture a
/// second owner. Both routes can return the complete root after their task
/// and interrupt capabilities have been reunited. Protocol-specific cached
/// state is deliberately not retained in this neutral owner.
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_pac::RadioHardware;
///
/// let hardware = RadioHardware::take().unwrap();
/// let _wifi = hardware.into_wifi();
/// let _bluetooth = hardware.into_bluetooth();
/// ```
#[must_use = "dropping the radio root permanently loses the unique hardware capability"]
pub struct RadioHardware {
    wifi_mac: svd::peripheral_ownership::WifiMacPeripherals,
    wifi_interrupts: svd::peripheral_ownership::WifiInterruptPeripherals,
    radio_phy: svd::peripheral_ownership::RadioPhyPeripherals,
    coexistence: svd::peripheral_ownership::CoexistencePeripherals,
    bluetooth: svd::peripheral_ownership::BluetoothControllerPeripherals,
    bluetooth_interrupts: svd::peripheral_ownership::BluetoothInterruptPeripherals,
    shared_radio: svd::peripheral_ownership::SharedRadioPeripherals,
    ieee802154: svd::peripheral_ownership::Ieee802154Peripherals,
}

impl RadioHardware {
    /// Acquire the generated radio singleton once.
    pub fn take() -> Option<Self> {
        svd::Peripherals::take().map(Self::from_peripherals)
    }

    /// Bind the generated singleton to the protocol-neutral restricted root.
    fn from_peripherals(peripherals: svd::Peripherals) -> Self {
        let svd::peripheral_ownership::PeripheralPartitions {
            wifi_mac,
            wifi_interrupts,
            radio_phy,
            coexistence,
            bluetooth,
            bluetooth_interrupts,
            shared_radio,
            ieee802154,
        } = svd::peripheral_ownership::partition(peripherals);
        Self {
            wifi_mac,
            wifi_interrupts,
            radio_phy,
            coexistence,
            bluetooth,
            bluetooth_interrupts,
            shared_radio,
            ieee802154,
        }
    }

    /// Construct the complete root inside one isolated validation image.
    #[cfg(any(test, feature = "validation-probes"))]
    #[doc(hidden)]
    pub fn for_validation() -> Self {
        Self::from_peripherals(svd::peripheral_ownership::peripherals_for_validation())
    }

    /// Consume the neutral root into the exclusive standalone Wi-Fi route.
    pub fn into_wifi(self) -> WifiColdRegisters {
        let Self {
            wifi_mac,
            wifi_interrupts,
            radio_phy,
            coexistence,
            bluetooth,
            bluetooth_interrupts,
            shared_radio,
            ieee802154,
        } = self;
        WifiColdRegisters {
            registers: WifiRadioRegisters {
                peripherals: WifiRadioPeripheralOwners {
                    wifi_mac,
                    ieee802154_mac: ieee802154.ieee802154_mac,
                    radio_phy: RadioPhyRegisters {
                        peripherals: radio_phy,
                    },
                    coexistence,
                    shared_radio,
                },
                retained_bluetooth: RetainedBluetoothPeripheralOwners {
                    bluetooth,
                    bluetooth_interrupts,
                },
                wifi_baseband_enabled: false,
                station_tbtt_wake_prepared: false,
                station_modem_wakeup: mac_modem_wakeup::StaModemWakeOwnership::new(),
            },
            interrupts: wifi_interrupts,
        }
    }

    /// Consume the neutral root into the exclusive standalone Bluetooth route.
    ///
    /// This transition is ownership-only. It performs no controller reset,
    /// clock, interrupt, or enable transaction.
    pub fn into_bluetooth(self) -> BluetoothColdRegisters {
        let Self {
            wifi_mac,
            wifi_interrupts,
            radio_phy,
            coexistence,
            bluetooth,
            bluetooth_interrupts,
            shared_radio,
            ieee802154,
        } = self;
        BluetoothColdRegisters {
            task: BluetoothTaskRegisters {
                bluetooth,
                radio_phy: RadioPhyRegisters {
                    peripherals: radio_phy,
                },
                coexistence,
                shared_radio,
                retained_wifi: RetainedWifiPeripheralOwners {
                    wifi_mac,
                    wifi_interrupts,
                    ieee802154,
                },
            },
            interrupts: BluetoothInterruptSetup {
                peripherals: bluetooth_interrupts,
            },
        }
    }
}

/// Known MAC interrupt bits recovered from reviewed vendor transactions.
///
/// Construction from a raw integer is deliberately crate-private. Public
/// code may combine known constants, but cannot invent a writable bit.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MacInterruptEvents(u32);

impl MacInterruptEvents {
    pub const TX_COMPLETE: Self = Self(0x0000_0080);
    pub const COLLISION: Self = Self(0x0000_0100);
    pub const WATCHDOG: Self = Self(0x0000_0800);
    pub const RX_SUCCESS: Self = Self(0x0000_4000);
    pub const TX_TIMEOUT: Self = Self(0x0008_0000);
    pub const RX_ASSOCIATED_AUXILIARY_5: Self = Self(1 << 5);
    pub const RX_ASSOCIATED_AUXILIARY_24: Self = Self(1 << 24);

    pub const fn empty() -> Self {
        Self(0)
    }

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Numeric observation for protocol dispatch and diagnostics.
    ///
    /// This is read-only evidence: there is no public inverse constructor.
    pub const fn bits(self) -> u32 {
        self.0
    }

    const fn from_observation(bits: u32) -> Self {
        Self(bits)
    }
}

impl core::ops::BitOr for MacInterruptEvents {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        self.union(rhs)
    }
}

/// One sampled MAC interrupt image which can be acknowledged exactly once.
pub struct MacInterruptSnapshot(svd::interrupt_snapshot::MacInterruptSnapshot);

impl MacInterruptSnapshot {
    pub fn events(&self) -> MacInterruptEvents {
        MacInterruptEvents::from_observation(self.0.bits())
    }

    pub fn bits(&self) -> u32 {
        self.events().bits()
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn for_validation(bits: u32) -> Self {
        Self(svd::interrupt_snapshot::mac_interrupt_for_validation(bits))
    }
}

/// One sampled power-interrupt image with intentionally opaque bit semantics.
pub struct MacPowerInterruptSnapshot(svd::interrupt_snapshot::MacPowerInterruptSnapshot);

impl MacPowerInterruptSnapshot {
    pub fn bits(&self) -> u32 {
        self.0.bits()
    }

    /// Test one reviewed WDEVPWR cause without assigning names to any other
    /// bit in the opaque status image.
    pub const fn contains(&self, cause: MacPowerWakeCause) -> bool {
        self.0.bits() & cause.event_mask() != 0
    }

    /// Preserve every cause outside the four reviewed TSF-timer sources as
    /// opaque evidence for a later qualification slice.
    pub const fn unknown_bits(&self) -> u32 {
        self.0.bits() & !MacPowerWakeCause::REVIEWED_MASK
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn for_validation(bits: u32) -> Self {
        Self(svd::interrupt_snapshot::mac_power_interrupt_for_validation(
            bits,
        ))
    }
}

#[inline]
fn device_fence() {
    svd::device_access::fence();
}

/// Host-test representation of one recovered MMIO access policy.
///
/// This catalog is absent from ordinary builds. Production code uses closed
/// capabilities and cannot obtain raw register descriptors.
#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum RegisterAccess {
    /// Software may only observe the register.
    ReadOnly,
    /// Software may only publish a value or trigger.
    WriteOnly,
    /// Software may observe and update the register.
    ReadWrite,
}

/// One test-only, PAC-described 32-bit MMIO register.
#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Register32 {
    address: usize,
    access: RegisterAccess,
    reset_value: Option<u32>,
}

#[cfg(test)]
impl Register32 {
    pub(crate) const fn new(address: usize) -> Self {
        Self {
            address,
            access: RegisterAccess::ReadWrite,
            reset_value: None,
        }
    }

    pub(crate) const fn described(
        address: usize,
        access: RegisterAccess,
        reset_value: Option<u32>,
    ) -> Self {
        Self {
            address,
            access,
            reset_value,
        }
    }

    pub(crate) const fn address(self) -> usize {
        self.address
    }

    /// SVD-described software access policy.
    pub const fn access(self) -> RegisterAccess {
        self.access
    }

    /// Reset value when the recovered source names one.
    ///
    /// `None` means unknown, not necessarily zero.
    pub const fn reset_value(self) -> Option<u32> {
        self.reset_value
    }
}

/// One named bit-field inside a 32-bit register.
///
/// Instances are generated from the recovered SVD. Construction remains
/// crate-private so higher layers cannot assign guessed bit positions.
#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Field32 {
    offset: u8,
    width: u8,
}

#[cfg(test)]
impl Field32 {
    pub(crate) const fn new(offset: u8, width: u8) -> Self {
        assert!(width != 0 && width <= 32);
        assert!(offset < 32 && (offset as u16) + (width as u16) <= 32);
        Self { offset, width }
    }

    /// Least-significant bit position recovered for this field.
    pub const fn offset(self) -> u8 {
        self.offset
    }

    /// Number of adjacent bits recovered for this field.
    pub const fn width(self) -> u8 {
        self.width
    }

    /// Register mask occupied by this field.
    pub const fn mask(self) -> u32 {
        if self.width == 32 {
            u32::MAX
        } else {
            ((1_u32 << self.width) - 1) << self.offset
        }
    }

    /// Maximum unshifted value representable by this field.
    pub const fn max_value(self) -> u32 {
        if self.width == 32 {
            u32::MAX
        } else {
            (1_u32 << self.width) - 1
        }
    }

    /// Encode a value, returning `None` rather than truncating an invalid one.
    pub const fn checked_value(self, value: u32) -> Option<u32> {
        if value <= self.max_value() {
            Some(value << self.offset)
        } else {
            None
        }
    }

    /// Extract the unshifted field value from a register image.
    pub const fn extract(self, register: u32) -> u32 {
        (register & self.mask()) >> self.offset
    }

    /// Replace this field while preserving every other register bit.
    ///
    /// Returns `None` when `value` does not fit the recovered field width.
    pub const fn checked_insert(self, register: u32, value: u32) -> Option<u32> {
        match self.checked_value(value) {
            Some(encoded) => Some((register & !self.mask()) | encoded),
            None => None,
        }
    }
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
/// use open_esp_radio_esp32s31_pac::svd;
/// ```
///
/// The address-bearing host catalog is also unavailable in production:
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_pac::Register32;
///
/// let forged = Register32::new(0x2010_4000);
/// ```
///
/// Finally, the owner has no generic address/value escape hatch. Every
/// writable transaction must be an explicitly reviewed capability:
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_pac::WifiRadioRegisters;
///
/// let unreviewed_write = WifiRadioRegisters::write_register;
/// ```
pub struct WifiRadioRegisters {
    peripherals: WifiRadioPeripheralOwners,
    retained_bluetooth: RetainedBluetoothPeripheralOwners,
    wifi_baseband_enabled: bool,
    station_tbtt_wake_prepared: bool,
    station_modem_wakeup: mac_modem_wakeup::StaModemWakeOwnership,
}

impl WifiRadioRegisters {
    #[cfg(test)]
    fn for_test() -> Self {
        RadioHardware::for_validation().into_wifi().into_running().0
    }

    /// Reunite a quiescent Wi-Fi task owner with its inactive interrupt setup.
    ///
    /// The caller must first disable the CPU routes and recover `interrupts`
    /// from the finite ISR epoch. This conversion performs no MMIO.
    pub fn into_cold(self, interrupts: MacInterruptSetup) -> WifiColdRegisters {
        WifiColdRegisters {
            registers: self,
            interrupts: interrupts.into_peripherals(),
        }
    }

    /// Synchronize the owned Wi-Fi-enable image after a platform PAC update.
    ///
    /// This state replaces deep calibration reads through a second, custom
    /// `MODEM_SYSCON` description. The unique [`WifiRadioRegisters`] owner and
    /// its platform token update it together.
    #[doc(hidden)]
    pub fn set_wifi_baseband_enabled_image(&mut self, enabled: bool) {
        self.wifi_baseband_enabled = enabled;
    }

    /// Return the Wi-Fi-enable image owned by this radio instance.
    #[doc(hidden)]
    pub fn wifi_baseband_enabled_image(&self) -> bool {
        self.wifi_baseband_enabled
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
        // The official platform PAC owns HP, PMU and LP peripherals. Legacy
        // The host-only catalog is limited to the custom modem/radio aperture.
        matches!(address, 0x2010_0000..=0x2010_ffff)
    }
}

/// Pre-runtime radio owner that still controls the cold MAC interrupt fields.
///
/// PHY setup, cold MAC initialization and polling-only scan/authentication use
/// this owner. Consuming [`into_running`](Self::into_running) permanently
/// removes MAC and WDEVPWR interrupt operations from the ordinary task owner
/// and returns the initial setup token for a later dual-ISR handoff. A closed
/// ISR epoch can return the same peripheral ownership to another setup token.
pub struct WifiColdRegisters {
    registers: WifiRadioRegisters,
    interrupts: svd::peripheral_ownership::WifiInterruptPeripherals,
}

impl WifiColdRegisters {
    /// Complete the one-way cold-to-running ownership transition.
    ///
    /// This operation itself performs no MMIO. The returned setup token keeps
    /// MAC interrupts masked until its consuming activation transaction
    /// creates the ISR-only [`MacInterruptRegisters`] and
    /// [`MacPowerInterruptRegisters`] capabilities.
    pub fn into_running(self) -> (WifiRadioRegisters, MacInterruptSetup) {
        (
            self.registers,
            MacInterruptSetup::from_peripherals(self.interrupts),
        )
    }

    /// Return every Wi-Fi, Bluetooth and shared owner to the neutral root.
    ///
    /// This is an ownership-only transition. Higher layers remain responsible
    /// for completing their clock/reset shutdown before changing protocols;
    /// the next Wi-Fi route starts with a fresh disabled baseband cache.
    pub fn release(self) -> RadioHardware {
        let Self {
            registers:
                WifiRadioRegisters {
                    peripherals:
                        WifiRadioPeripheralOwners {
                            wifi_mac,
                            ieee802154_mac,
                            radio_phy,
                            coexistence,
                            shared_radio,
                        },
                    retained_bluetooth:
                        RetainedBluetoothPeripheralOwners {
                            bluetooth,
                            bluetooth_interrupts,
                        },
                    wifi_baseband_enabled: _,
                    station_tbtt_wake_prepared: _,
                    station_modem_wakeup: _,
                },
            interrupts: wifi_interrupts,
        } = self;
        RadioHardware {
            wifi_mac,
            wifi_interrupts,
            radio_phy: radio_phy.peripherals,
            coexistence,
            bluetooth,
            bluetooth_interrupts,
            shared_radio,
            ieee802154: svd::peripheral_ownership::Ieee802154Peripherals { ieee802154_mac },
        }
    }

    /// Borrow the radio-register capability during the cold lifecycle.
    ///
    /// This explicit bridge exists for the HAL crate, which owns the cold
    /// hardware sequence.  Unlike the former `Deref` implementation it does
    /// not let an arbitrary method call silently widen cold authority into a
    /// runtime register owner.  Production crates above HAL never receive
    /// either side of this borrow.
    #[doc(hidden)]
    pub fn radio(&self) -> &WifiRadioRegisters {
        &self.registers
    }

    /// Mutably borrow the radio-register capability during the cold lifecycle.
    #[doc(hidden)]
    pub fn radio_mut(&mut self) -> &mut WifiRadioRegisters {
        &mut self.registers
    }

    /// Read the cold initializer's currently published interrupt mask.
    pub fn mac_interrupt_enable(&self) -> u32 {
        self.interrupts
            .wifi_mac_interrupt
            .enable()
            .read()
            .event_mask()
            .bits()
    }

    /// Mask every MAC event and acknowledge every stale cold event.
    pub fn mask_and_clear_all_mac_interrupts(&mut self) {
        let interrupt = &self.interrupts.wifi_mac_interrupt;
        generated::mac_interrupt_enable(interrupt, MacInterruptMask::NONE);
        generated::mac_interrupt_clear(interrupt, generated::MacInterruptClearImage::new(u32::MAX));
        device_fence();
    }
}

/// Exclusive standalone Bluetooth owner before task/interrupt separation.
///
/// The cold type exposes no controller transaction. It only preserves the
/// reviewed raw partitions until a higher layer establishes lifecycle order.
#[must_use = "the cold Bluetooth route retains every radio owner"]
pub struct BluetoothColdRegisters {
    task: BluetoothTaskRegisters,
    interrupts: BluetoothInterruptSetup,
}

impl BluetoothColdRegisters {
    /// Separate the ordinary task owner from the inactive interrupt owner.
    ///
    /// This conversion performs no MMIO and does not claim that the hardware
    /// interrupt route has been configured or enabled.
    pub fn separate_interrupt_owner(self) -> (BluetoothTaskRegisters, BluetoothInterruptSetup) {
        (self.task, self.interrupts)
    }

    /// Return every Wi-Fi, Bluetooth and shared owner to the neutral root.
    pub fn release(self) -> RadioHardware {
        let Self { task, interrupts } = self;
        task.into_hardware(interrupts)
    }
}

/// Ordinary task-side owner for one exclusive standalone Bluetooth route.
///
/// Wi-Fi and all shared resources remain retained privately. Methods on this
/// owner are individually reviewed register transactions; possessing it does
/// not itself prove that common PHY, BTBB or controller lifecycle prerequisites
/// have run.
#[must_use = "the Bluetooth task owner must be reunited before release"]
pub struct BluetoothTaskRegisters {
    bluetooth: svd::peripheral_ownership::BluetoothControllerPeripherals,
    radio_phy: RadioPhyRegisters,
    coexistence: svd::peripheral_ownership::CoexistencePeripherals,
    shared_radio: svd::peripheral_ownership::SharedRadioPeripherals,
    retained_wifi: RetainedWifiPeripheralOwners,
}

impl BluetoothTaskRegisters {
    fn into_hardware(self, interrupts: BluetoothInterruptSetup) -> RadioHardware {
        let Self {
            bluetooth,
            radio_phy,
            coexistence,
            shared_radio,
            retained_wifi:
                RetainedWifiPeripheralOwners {
                    wifi_mac,
                    wifi_interrupts,
                    ieee802154,
                },
        } = self;
        RadioHardware {
            wifi_mac,
            wifi_interrupts,
            radio_phy: radio_phy.peripherals,
            coexistence,
            bluetooth,
            bluetooth_interrupts: interrupts.peripherals,
            shared_radio,
            ieee802154,
        }
    }

    /// Reunite a quiescent Bluetooth task owner with its inactive IRQ owner.
    ///
    /// This conversion performs no MMIO.
    pub fn into_cold(self, interrupts: BluetoothInterruptSetup) -> BluetoothColdRegisters {
        BluetoothColdRegisters {
            task: self,
            interrupts,
        }
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
    peripherals: svd::peripheral_ownership::BluetoothInterruptPeripherals,
}

/// Bluetooth interrupt-bank capability staged for a future powered ISR epoch.
///
/// [`BluetoothInterruptOutputPrepared::stage_for_cpu_routes`] constructs this
/// value only after the reviewed baseline masks and controller output have
/// been prepared. The platform must retain it in stable storage shared by the
/// primary and NRT handlers before enabling either CPU route.
#[must_use = "the Bluetooth interrupt owner must be deactivated and reunited"]
pub struct BluetoothInterruptRegisters {
    peripherals: svd::peripheral_ownership::BluetoothInterruptPeripherals,
}

#[cfg(test)]
mod tests {
    use super::{Field32, MacInterruptMask, RadioHardware, Register32, WifiRadioRegisters, mac};

    fn assert_valid(register: Register32) {
        assert!(WifiRadioRegisters::contains(register.address()));
        assert_eq!(register.address() & 3, 0);
    }

    #[test]
    fn fields_reject_values_that_do_not_fit() {
        let field = Field32::new(8, 4);
        assert_eq!(field.mask(), 0x0000_0f00);
        assert_eq!(field.checked_value(6), Some(0x0000_0600));
        assert_eq!(field.checked_value(16), None);
        assert_eq!(field.checked_insert(0xffff_f0ff, 6), Some(0xffff_f6ff));
        assert_eq!(field.extract(0x0000_0a00), 10);
    }

    #[test]
    fn cold_owner_is_consumed_by_interrupt_setup_split() {
        // This host test does not access MMIO and creates no second
        // radio owner; it exercises only the type-level ownership transition.
        let registers = RadioHardware::for_validation().into_wifi();
        let (_running, _setup) = registers.into_running();
    }

    #[test]
    fn wifi_route_roundtrip_returns_the_complete_root() {
        let wifi = RadioHardware::for_validation().into_wifi();
        let (task, setup) = wifi.into_running();
        let hardware = task.into_cold(setup).release();

        // A lossless Wi-Fi release makes the mutually exclusive Bluetooth
        // route available without stealing a second raw singleton.
        let bluetooth = hardware.into_bluetooth();
        let _hardware = bluetooth.release();
    }

    #[test]
    fn bluetooth_task_and_interrupt_owners_roundtrip_without_mmio() {
        let bluetooth = RadioHardware::for_validation().into_bluetooth();
        let (task, setup) = bluetooth.separate_interrupt_owner();
        let hardware = task.into_cold(setup).release();

        // The inverse route proves that Wi-Fi partitions were retained while
        // Bluetooth task and IRQ authority were separated.
        let wifi = hardware.into_wifi();
        let _hardware = wifi.release();
    }

    #[test]
    fn generated_tx_banks_reverse_physical_order_exactly_once() {
        // This host test inspects generated register pointers only and
        // performs no volatile access.
        let registers = WifiRadioRegisters::for_test();
        let control = &registers.peripherals.wifi_mac.wifi_mac_tx_queue_control;
        let vector = &registers.peripherals.wifi_mac.wifi_mac_tx_queue_vector;
        let completion = &registers.peripherals.wifi_mac.wifi_mac_tx_completion;
        assert_eq!(control.control(3).as_ptr() as usize, 0x2010_4d70);
        assert_eq!(control.control(0).as_ptr() as usize, 0x2010_4d40);
        assert_eq!(vector.plcp1(3).as_ptr() as usize, 0x2010_54d8);
        assert_eq!(vector.plcp1(0).as_ptr() as usize, 0x2010_5364);
        assert_eq!(vector.he_control(3).as_ptr() as usize, 0x2010_54e4);
        assert_eq!(vector.he_control_config(3).as_ptr() as usize, 0x2010_5518);
        assert_eq!(vector.he_control(0).as_ptr() as usize, 0x2010_5370);
        assert_eq!(vector.he_control_config(0).as_ptr() as usize, 0x2010_53a4);
        assert_eq!(completion.primary(3).as_ptr() as usize, 0x2010_553c);
        assert_eq!(completion.primary(0).as_ptr() as usize, 0x2010_53c8);
    }

    #[test]
    fn generated_tx_block_ack_debug_geometry_matches_complete_decoders() {
        // This host test inspects generated register pointers only and
        // performs no volatile access.
        let registers = WifiRadioRegisters::for_test();
        let queues = &registers.peripherals.wifi_mac.wifi_mac_rx_dma;
        assert_eq!(
            queues.tx_queue_information_q0().as_ptr() as usize,
            0x2010_5524
        );
        assert_eq!(
            queues.tx_block_ack_bitmap_high_q0().as_ptr() as usize,
            0x2010_5528
        );
        assert_eq!(
            queues.tx_block_ack_transmitter_address_low_q0().as_ptr() as usize,
            0x2010_5538
        );
        assert_eq!(
            queues.tx_queue_information_q7().as_ptr() as usize,
            0x2010_51c0
        );
        assert_eq!(
            queues.tx_block_ack_bitmap_high_q7().as_ptr() as usize,
            0x2010_51c4
        );
        assert_eq!(
            queues.tx_block_ack_transmitter_address_low_q7().as_ptr() as usize,
            0x2010_51d4
        );

        let internal = &registers
            .peripherals
            .wifi_mac
            .wifi_mac_internal_tx_block_ack;
        assert_eq!(internal.bitmap_high().as_ptr() as usize, 0x2010_429c);
        assert_eq!(internal.control_sequence().as_ptr() as usize, 0x2010_42ac);
    }

    #[test]
    fn generated_rx_block_ack_banks_match_complete_hal_ampdu_geometry() {
        // This host test inspects generated register pointers only and
        // performs no volatile access.
        let registers = WifiRadioRegisters::for_test();
        let block = &registers.peripherals.wifi_mac.wifi_mac_rx_dma;
        for physical_index in 0..8 {
            let base = 0x2010_4178 + physical_index * 0x24;
            assert_eq!(
                block.rx_block_ack_entry_control(physical_index).as_ptr() as usize,
                base
            );
            assert_eq!(
                block
                    .rx_block_ack_entry_current_sequence(physical_index)
                    .as_ptr() as usize,
                base + 0x0c
            );
            assert_eq!(
                block
                    .rx_block_ack_entry_start_sequence_load(physical_index)
                    .as_ptr() as usize,
                base + 0x10
            );
            assert_eq!(
                block
                    .rx_block_ack_entry_bitmap_low_status(physical_index)
                    .as_ptr() as usize,
                base + 0x14
            );
            assert_eq!(
                block
                    .rx_block_ack_entry_bitmap_low_load(physical_index)
                    .as_ptr() as usize,
                base + 0x18
            );
            assert_eq!(
                block
                    .rx_block_ack_entry_bitmap_high_status(physical_index)
                    .as_ptr() as usize,
                base + 0x1c
            );
            assert_eq!(
                block
                    .rx_block_ack_entry_bitmap_high_load(physical_index)
                    .as_ptr() as usize,
                base + 0x20
            );
        }
        assert_eq!(
            block.rx_block_ack_entry0_control().as_ptr() as usize,
            0x2010_4274
        );
        assert_eq!(
            block.rx_block_ack_entry7_control().as_ptr() as usize,
            0x2010_4178
        );
        assert_eq!(
            block.rx_block_ack_agreement_update().as_ptr() as usize,
            0x2010_4298
        );
        assert_eq!(
            block.extra_softap_rx_block_ack_control().as_ptr() as usize,
            0x2010_4ea4
        );
    }

    #[test]
    fn generated_sta_rx_policy_registers_match_complete_leaf_geometry() {
        // This host test inspects generated register pointers only and
        // performs no volatile access.
        let registers = WifiRadioRegisters::for_test();
        assert_eq!(
            registers
                .peripherals
                .wifi_mac
                .wifi_mac_bssid_policy
                .bssid_high(0)
                .as_ptr() as usize,
            0x2010_4004
        );
        assert_eq!(
            registers
                .peripherals
                .wifi_mac
                .wifi_mac_interface_address
                .address_high(0)
                .as_ptr() as usize,
            0x2010_4060
        );
        assert_eq!(
            registers
                .peripherals
                .wifi_mac
                .wifi_mac_rx_filter
                .policy(0)
                .as_ptr() as usize,
            0x2010_40d8
        );
        assert_eq!(
            registers
                .peripherals
                .wifi_mac
                .wifi_mac_rx_filter
                .policy(3)
                .as_ptr() as usize,
            0x2010_40e4
        );
        assert_eq!(
            registers
                .peripherals
                .wifi_mac
                .wifi_mac_rx_filter
                .misc_packet_policy()
                .as_ptr() as usize,
            0x2010_40f4
        );
        assert_eq!(
            registers
                .peripherals
                .wifi_mac
                .wifi_mac_control
                .control()
                .as_ptr() as usize,
            0x2010_4cac
        );
        assert_eq!(
            registers
                .peripherals
                .wifi_mac
                .wifi_mac_regdma_control
                .control()
                .as_ptr() as usize,
            0x2010_d83c
        );
    }

    #[test]
    fn generated_interface_address_pairs_match_complete_leaf_stride() {
        // This host test inspects generated register pointers only and
        // performs no volatile access.
        let registers = WifiRadioRegisters::for_test();
        let addresses = &registers.peripherals.wifi_mac.wifi_mac_interface_address;
        for interface in 0..4 {
            assert_eq!(
                addresses.address_low(interface).as_ptr() as usize,
                0x2010_405c + interface * 8
            );
            assert_eq!(
                addresses.address_high(interface).as_ptr() as usize,
                0x2010_4060 + interface * 8
            );
        }
    }

    #[test]
    fn generated_station_tsf_load_matches_complete_hal_tsf_geometry() {
        // This host test inspects generated register pointers only and
        // performs no volatile access.
        let registers = WifiRadioRegisters::for_test();
        let load = &registers.peripherals.wifi_mac.wifi_mac_sta_tsf_load;
        assert_eq!(load.control().as_ptr() as usize, 0x2010_d814);
        assert_eq!(load.value_low().as_ptr() as usize, 0x2010_d818);
        assert_eq!(load.value_high().as_ptr() as usize, 0x2010_d81c);
        assert_eq!(load.snapshot_low().as_ptr() as usize, 0x2010_d820);
        assert_eq!(load.snapshot_high().as_ptr() as usize, 0x2010_d824);
        assert_eq!(
            registers
                .peripherals
                .wifi_mac
                .wifi_mac_rtc_timer_update
                .sta_tsf_control()
                .as_ptr() as usize,
            0x2010_d858
        );
    }

    #[test]
    fn generated_cold_handshake_matches_complete_hal_init_prefix() {
        // This host test inspects a generated register pointer only
        // and performs no volatile access.
        let registers = WifiRadioRegisters::for_test();
        assert_eq!(
            registers
                .peripherals
                .wifi_mac
                .wifi_mac_cold_handshake
                .control()
                .as_ptr() as usize,
            0x2010_4de0
        );
    }

    #[test]
    fn generated_crypto_aux_register_matches_complete_cold_leaf() {
        // This host test inspects a generated register pointer only
        // and performs no volatile access.
        let registers = WifiRadioRegisters::for_test();
        assert_eq!(
            registers
                .peripherals
                .wifi_mac
                .wifi_mac_crypto_control
                .init_aux_unknown()
                .as_ptr() as usize,
            0x2010_480c
        );
    }

    #[test]
    fn generated_rx_cold_prefix_registers_match_complete_leaf() {
        // This host test inspects generated register pointers only and
        // performs no volatile access.
        let registers = WifiRadioRegisters::for_test();
        let dma = &registers.peripherals.wifi_mac.wifi_mac_rx_dma;
        assert_eq!(dma.rx_cold_control_unknown().as_ptr() as usize, 0x2010_407c);
        assert_eq!(dma.rx_buffer_limit_unknown().as_ptr() as usize, 0x2010_4c68);
        assert_eq!(dma.rx_buffer_base_unknown().as_ptr() as usize, 0x2010_4c6c);
        assert_eq!(
            dma.rx_descriptor_high_window().as_ptr() as usize,
            0x2010_4c70
        );
    }

    #[test]
    fn generated_mac_enable_gate_matches_complete_leaf() {
        // This host test inspects generated register pointers only and
        // performs no volatile access.
        let registers = RadioHardware::for_validation().into_wifi();
        assert_eq!(
            registers
                .radio()
                .peripherals
                .wifi_mac
                .wifi_mac_core_enable
                .control()
                .as_ptr() as usize,
            0x2010_4c00
        );
        assert_eq!(
            registers.interrupts.wifi_mac_interrupt.enable().as_ptr() as usize,
            0x2010_4c40
        );
        assert_eq!(
            registers.interrupts.wifi_mac_interrupt.raw().as_ptr() as usize,
            0x2010_4c44
        );
    }

    #[test]
    fn generated_debug_oracle_registers_keep_one_canonical_owner() {
        // This host test inspects generated register pointers only and
        // performs no volatile access.
        let registers = RadioHardware::for_validation().into_wifi();
        let he = &registers
            .radio()
            .peripherals
            .wifi_mac
            .wifi_mac_he_init_prefix;
        assert_eq!(he.parent_enable().as_ptr() as usize, 0x2010_4c2c);
        assert_eq!(he.interrupt_1_raw().as_ptr() as usize, 0x2010_4c30);
        assert_eq!(he.interrupt_1_status().as_ptr() as usize, 0x2010_4c34);

        let power = &registers.interrupts.wifi_mac_power_interrupt;
        assert_eq!(power.enable().as_ptr() as usize, 0x2010_d8b4);
        assert_eq!(power.raw().as_ptr() as usize, 0x2010_d8b8);
        assert_eq!(power.status().as_ptr() as usize, 0x2010_d8bc);
        assert_eq!(power.clear().as_ptr() as usize, 0x2010_d8c0);

        assert_eq!(
            registers
                .radio()
                .peripherals
                .wifi_mac
                .wifi_mac_rx_dma
                .csi_dump_config()
                .as_ptr() as usize,
            0x2010_411c
        );
    }

    #[test]
    fn generated_phy_low_rate_registers_match_complete_rom_leaves() {
        // This host test inspects generated register pointers only and
        // performs no volatile access.
        let registers = WifiRadioRegisters::for_test();
        let bb = &registers.peripherals.radio_phy.peripherals.phy_agc_oracle;
        assert_eq!(bb.low_rate_primary_control().as_ptr() as usize, 0x2010_8060);
        assert_eq!(
            bb.low_rate_secondary_control().as_ptr() as usize,
            0x2010_807c
        );
    }

    #[test]
    fn generated_phy_feature_and_watchdog_fields_match_complete_rom_leaves() {
        // This host test inspects generated register pointers only and
        // performs no volatile access.
        let registers = WifiRadioRegisters::for_test();
        let agc = &registers.peripherals.radio_phy.peripherals.phy_agc_oracle;
        let frequency = &registers
            .peripherals
            .radio_phy
            .peripherals
            .phy_frequency_channel_oracle;
        let baseband = &registers
            .peripherals
            .radio_phy
            .peripherals
            .phy_baseband_config_oracle;

        assert_eq!(agc.csi_dump_force_control().as_ptr() as usize, 0x2010_70a4);
        assert_eq!(
            frequency.channel_cbw_control_1().as_ptr() as usize,
            0x2010_7ce4
        );
        assert_eq!(
            baseband.tx_output_filter_control().as_ptr() as usize,
            0x2010_7440
        );
        assert_eq!(
            baseband.baseband_watchdog_status().as_ptr() as usize,
            0x2010_7c08
        );
    }

    #[test]
    fn generated_last_rx_buffer_table_matches_complete_leaf_geometry() {
        // This host test inspects generated register pointers only and
        // performs no volatile access.
        let registers = WifiRadioRegisters::for_test();
        let table = &registers.peripherals.wifi_mac.wifi_mac_last_rx_buffer;
        assert_eq!(table.control().as_ptr() as usize, 0x2010_4120);
        for entry in 0..6 {
            assert_eq!(
                table.entry_control(entry).as_ptr() as usize,
                0x2010_4124 + entry * 4
            );
            assert_eq!(
                table.entry_parameter_a(entry).as_ptr() as usize,
                0x2010_4140 + entry * 4
            );
            assert_eq!(
                table.entry_parameter_b(entry).as_ptr() as usize,
                0x2010_415c + entry * 4
            );
        }
        assert_eq!(
            registers
                .peripherals
                .wifi_mac
                .wifi_mac_rx_csi_control
                .control()
                .as_ptr() as usize,
            0x2010_4098
        );
    }

    #[test]
    fn generated_mac_txrx_prefix_matches_complete_leaf_geometry() {
        // This host test inspects generated register pointers only and
        // performs no volatile access.
        let registers = WifiRadioRegisters::for_test();
        let init = &registers.peripherals.wifi_mac.wifi_mac_txrx_prefix;
        for queue in 0..4 {
            assert_eq!(
                init.rx_queue_default(queue).as_ptr() as usize,
                0x2010_40fc + queue * 4
            );
        }
        assert_eq!(init.control_edges().as_ptr() as usize, 0x2010_4114);
        assert_eq!(init.timing_control().as_ptr() as usize, 0x2010_4118);
        assert_eq!(init.feature_edges().as_ptr() as usize, 0x2010_4c8c);
        assert_eq!(init.mode_control().as_ptr() as usize, 0x2010_4c98);
        assert_eq!(init.shared_enable_control().as_ptr() as usize, 0x2010_4ca0);
    }

    #[test]
    fn generated_mac_antenna_init_matches_complete_leaf_geometry() {
        // This host test inspects generated register pointers only and
        // performs no volatile access.
        let registers = WifiRadioRegisters::for_test();
        let init = &registers.peripherals.wifi_mac.wifi_mac_antenna_init;
        assert_eq!(init.common_control().as_ptr() as usize, 0x2010_42b0);
        for physical_bank in 0..4 {
            assert_eq!(
                init.bank_control(physical_bank).as_ptr() as usize,
                0x2010_51ac + physical_bank * 0x7c
            );
        }
        for physical_bank in 4..8 {
            assert_eq!(
                registers
                    .peripherals
                    .wifi_mac
                    .wifi_mac_tx_queue_vector
                    .length_control(physical_bank - 4)
                    .as_ptr() as usize,
                0x2010_51ac + physical_bank * 0x7c
            );
        }
    }

    #[test]
    fn generated_mac_coex_init_matches_complete_setter_geometry() {
        // This host test inspects generated register pointers only and
        // performs no volatile access.
        let registers = WifiRadioRegisters::for_test();
        let coex = &registers.peripherals.coexistence.wifi_mac_coex_init;
        assert_eq!(coex.rx_pti().as_ptr() as usize, 0x2010_42fc);
        assert_eq!(
            coex.ofdma_tb_and_beamforming().as_ptr() as usize,
            0x2010_4dd4
        );
        assert_eq!(coex.beamforming().as_ptr() as usize, 0x2010_4dd8);
        assert_eq!(coex.default_control().as_ptr() as usize, 0x2010_4ddc);
    }

    #[test]
    fn generated_mac_he_prefix_matches_complete_leaf_geometry() {
        // This host test inspects generated register pointers only and
        // performs no volatile access.
        let registers = WifiRadioRegisters::for_test();
        let init = &registers.peripherals.wifi_mac.wifi_mac_he_init_prefix;
        assert_eq!(init.rx_field_control().as_ptr() as usize, 0x2010_4048);
        assert_eq!(init.bf_mode_control().as_ptr() as usize, 0x2010_409c);
        assert_eq!(init.bf_report_rate().as_ptr() as usize, 0x2010_4464);
        assert_eq!(init.bf_timing_control().as_ptr() as usize, 0x2010_4c78);
        assert_eq!(init.tb_tx_control().as_ptr() as usize, 0x2010_4e04);
        assert_eq!(
            registers
                .peripherals
                .wifi_mac
                .wifi_mac_beamforming_feedback_test
                .configuration()
                .as_ptr() as usize,
            0x2010_4e00
        );
        assert_eq!(
            registers
                .peripherals
                .radio_phy
                .peripherals
                .phy_agc_oracle
                .agc_init_high_control()
                .as_ptr() as usize,
            0x2010_7128
        );
    }

    #[test]
    fn generated_mac_he_tb_diagnostics_match_complete_blob_geometry() {
        // This host test inspects generated register pointers only and
        // performs no volatile access.
        let registers = WifiRadioRegisters::for_test();
        let statistics = &registers.peripherals.wifi_mac.wifi_mac_he_tb_statistics;
        assert_eq!(statistics.rx_trigger().as_ptr() as usize, 0x2010_43a0);
        assert_eq!(statistics.tb_transmission().as_ptr() as usize, 0x2010_43a4);

        let diagnostics = &registers.peripherals.wifi_mac.wifi_mac_he_tb_diagnostics;
        assert_eq!(diagnostics.timing().as_ptr() as usize, 0x2010_44f4);
        assert_eq!(diagnostics.psdu().as_ptr() as usize, 0x2010_44f8);
        assert_eq!(diagnostics.trigger().as_ptr() as usize, 0x2010_44fc);
        assert_eq!(diagnostics.user().as_ptr() as usize, 0x2010_4500);
    }

    #[test]
    fn generated_mac_he_ofdma_diagnostics_match_complete_blob_geometry() {
        // This host test inspects generated register pointers only and
        // performs no volatile access.
        let registers = WifiRadioRegisters::for_test();
        let bsr = &registers.peripherals.wifi_mac.wifi_mac_he_buffer_status;
        for tid in 0..8 {
            assert_eq!(
                bsr.hardware_bsr(tid).as_ptr() as usize,
                0x2010_4d74 + tid * 8
            );
            assert_eq!(
                bsr.software_bsr(tid).as_ptr() as usize,
                0x2010_4d78 + tid * 8
            );
        }
        assert_eq!(bsr.control().as_ptr() as usize, 0x2010_4db8);

        let vectors = &registers.peripherals.wifi_mac.wifi_mac_tx_queue_vector;
        for physical_queue in 0..4 {
            assert_eq!(
                vectors.vht_signal_1(physical_queue).as_ptr() as usize,
                0x2010_5378 + physical_queue * 0x7c
            );
            assert_eq!(
                vectors.vht_mode(physical_queue).as_ptr() as usize,
                0x2010_537c + physical_queue * 0x7c
            );
            assert_eq!(
                vectors.he_mpdu_length_tail(physical_queue).as_ptr() as usize,
                0x2010_5388 + physical_queue * 0x7c
            );
        }
        assert_eq!(
            registers
                .peripherals
                .wifi_mac
                .wifi_mac_beamforming_report
                .average_snr()
                .as_ptr() as usize,
            0x2010_5f94
        );

        let trigger = &registers
            .peripherals
            .wifi_mac
            .wifi_mac_he_trigger_rx_diagnostics;
        assert_eq!(trigger.state().as_ptr() as usize, 0x2010_4508);
        assert_eq!(trigger.basic_user().as_ptr() as usize, 0x2010_451c);
        assert_eq!(trigger.common_phy().as_ptr() as usize, 0x2010_4520);
        assert_eq!(trigger.common_trigger().as_ptr() as usize, 0x2010_4524);
        assert_eq!(trigger.packet_counts().as_ptr() as usize, 0x2010_452c);

        let obss = &registers
            .peripherals
            .wifi_mac
            .wifi_mac_he_obss_narrow_band_ru;
        assert_eq!(obss.disable_bitmap().as_ptr() as usize, 0x2010_4e9c);
        assert_eq!(obss.control().as_ptr() as usize, 0x2010_4ea0);

        let timers = &registers.peripherals.wifi_mac.wifi_mac_he_mu_edca_timer;
        for index in 0..4 {
            assert_eq!(
                timers.timer(index).as_ptr() as usize,
                0x2010_4dbc + index * 4
            );
        }
    }

    #[test]
    fn generated_mac_rx_statistics_match_complete_blob_geometry() {
        // This host test inspects generated register pointers only and
        // performs no volatile access.
        let registers = WifiRadioRegisters::for_test();
        let colors = &registers.peripherals.wifi_mac.wifi_mac_he_color_collision;
        assert_eq!(colors.bss_color_bitmap_low().as_ptr() as usize, 0x2010_4040);
        assert_eq!(
            colors.bss_color_bitmap_high().as_ptr() as usize,
            0x2010_4044
        );

        let statistics = &registers.peripherals.wifi_mac.wifi_mac_rx_statistics;
        assert_eq!(statistics.mpdu_and_cfo().as_ptr() as usize, 0x2010_430c);
        assert_eq!(
            statistics.nrx_error_power_drop().as_ptr() as usize,
            0x2010_432c
        );
        assert_eq!(
            statistics.nrx_he_sig_b_error().as_ptr() as usize,
            0x2010_4348
        );
        assert_eq!(statistics.cts_interrupt().as_ptr() as usize, 0x2010_4384);
        assert_eq!(
            statistics.last_unmatched_error().as_ptr() as usize,
            0x2010_4398
        );
        assert_eq!(statistics.trigger().as_ptr() as usize, 0x2010_439c);

        let hangs = &registers.peripherals.wifi_mac.wifi_mac_rx_hang_statistics;
        assert_eq!(hangs.hang().as_ptr() as usize, 0x2010_4c64);
        assert_eq!(hangs.rx_tx_hang().as_ptr() as usize, 0x2010_4e18);
        assert_eq!(hangs.rx_tx_panic().as_ptr() as usize, 0x2010_4e1c);

        let tx = &registers.peripherals.wifi_mac.wifi_mac_tx_statistics;
        assert_eq!(tx.tx_rts().as_ptr() as usize, 0x2010_4e08);
        assert_eq!(tx.trcts().as_ptr() as usize, 0x2010_4e14);

        let diagnostic = &registers
            .peripherals
            .wifi_mac
            .wifi_mac_diagnostic_statistics;
        assert_eq!(diagnostic.diag4().as_ptr() as usize, 0x2010_43b4);
        assert_eq!(diagnostic.diag8().as_ptr() as usize, 0x2010_44e0);
        assert_eq!(diagnostic.diag0().as_ptr() as usize, 0x2010_4e50);
        assert_eq!(diagnostic.diag_select().as_ptr() as usize, 0x2010_4e64);
    }

    #[test]
    fn generated_mac_rx_misc_matches_complete_blob_geometry() {
        // This host test inspects generated register pointers only and
        // performs no volatile access.
        let registers = WifiRadioRegisters::for_test();
        assert_eq!(
            registers
                .peripherals
                .wifi_mac
                .wifi_mac_rx_power_save
                .control()
                .as_ptr() as usize,
            0x2010_40a0
        );
        assert_eq!(
            registers
                .peripherals
                .wifi_mac
                .wifi_mac_rx_bssid_list
                .control()
                .as_ptr() as usize,
            0x2010_4110
        );
        assert_eq!(
            registers
                .peripherals
                .wifi_mac
                .wifi_mac_rx_custom_type
                .control()
                .as_ptr() as usize,
            0x2010_4ca4
        );
    }

    #[test]
    fn generated_mac_he_suffix_matches_complete_leaf_geometry() {
        // This host test inspects generated register pointers only and
        // performs no volatile access.
        let registers = WifiRadioRegisters::for_test();
        let init = &registers.peripherals.wifi_mac.wifi_mac_he_init_suffix;
        assert_eq!(init.multi_bssid_control().as_ptr() as usize, 0x2010_4020);
        assert_eq!(init.broadcast_ru_low().as_ptr() as usize, 0x2010_4038);
        assert_eq!(init.tx_mode_control().as_ptr() as usize, 0x2010_42b8);
        assert_eq!(
            registers
                .peripherals
                .radio_phy
                .peripherals
                .phy_frequency_channel_oracle
                .channel_tx_offset_control()
                .as_ptr() as usize,
            0x2010_4400
        );
        assert_eq!(init.ersu_ack_rate().as_ptr() as usize, 0x2010_4404);
        assert_eq!(init.ersu_and_vht_control().as_ptr() as usize, 0x2010_4c7c);
        assert_eq!(init.he_default_control().as_ptr() as usize, 0x2010_4c80);
        for physical in 0..8 {
            assert_eq!(
                init.queue_control(physical).as_ptr() as usize,
                0x2010_4cf8 + physical * 0x10
            );
        }
        for physical in 0..4 {
            assert_eq!(
                registers
                    .peripherals
                    .wifi_mac
                    .wifi_mac_tx_queue_control
                    .protection(physical)
                    .as_ptr() as usize,
                0x2010_4d34 + physical * 0x10
            );
        }
        for word in 0..120 {
            assert_eq!(
                init.he_scratch(word).as_ptr() as usize,
                0x2010_55f0 + word * 4
            );
        }
    }

    #[test]
    fn generated_mac_tx_power_init_matches_complete_leaf_geometry() {
        // This host test inspects generated register pointers only and
        // performs no volatile access.
        let registers = WifiRadioRegisters::for_test();
        let power = &registers.peripherals.wifi_mac.wifi_mac_tx_power_init;
        for word in 0..10 {
            assert_eq!(
                power.immediate_response(word).as_ptr() as usize,
                0x2010_4408 + word * 4
            );
        }
        for word in 0..3 {
            assert_eq!(
                power.tb_power(word).as_ptr() as usize,
                0x2010_4430 + word * 4
            );
            assert_eq!(
                power.tb_ru_power(word).as_ptr() as usize,
                0x2010_4440 + word * 4
            );
        }
        assert_eq!(power.tb_ru_power_tail().as_ptr() as usize, 0x2010_443c);
    }

    #[test]
    fn generated_mac_hal_tail_matches_complete_leaf_geometry() {
        // This host test inspects generated register pointers only and
        // performs no volatile access.
        let registers = WifiRadioRegisters::for_test();
        let rtc = &registers.peripherals.wifi_mac.wifi_mac_rtc_timer_update;
        assert_eq!(rtc.control().as_ptr() as usize, 0x2010_d830);
        assert_eq!(rtc.sta_tsf_control().as_ptr() as usize, 0x2010_d858);
        assert_eq!(rtc.slow_clock_calibration().as_ptr() as usize, 0x2010_d878);
        assert_eq!(
            registers
                .peripherals
                .wifi_mac
                .wifi_mac_rx_csi_control
                .control()
                .as_ptr() as usize,
            0x2010_4098
        );
    }

    #[test]
    fn mac_hal_tail_rejects_out_of_range_calibration_before_mmio() {
        // SAFETY: the rejected input returns before any generated register is
        // accessed; the host test therefore performs no volatile MMIO.
        let mut registers = RadioHardware::for_validation().into_wifi();
        assert!(!registers.initialize_mac_hal_tail(MacInterruptMask::COLD_RX, 0x0004_0000));
        assert!(!registers.initialize_mac_hal_tail(MacInterruptMask::NONE, u32::MAX));
    }

    #[test]
    fn generated_mac_txrx_callbacks_match_complete_leaf_geometry() {
        // This host test inspects generated register pointers only and
        // performs no volatile access.
        let registers = WifiRadioRegisters::for_test();
        let callbacks = &registers.peripherals.wifi_mac.wifi_mac_txrx_callbacks;
        assert_eq!(callbacks.ack_rate_table().as_ptr() as usize, 0x2010_444c);
        assert_eq!(
            callbacks.ack_cck_rate_table().as_ptr() as usize,
            0x2010_4450
        );
        assert_eq!(
            callbacks.ack_scck_rate_table().as_ptr() as usize,
            0x2010_4454
        );
        assert_eq!(callbacks.cts_rate_table().as_ptr() as usize, 0x2010_4458);
        assert_eq!(
            callbacks.cts_cck_rate_table().as_ptr() as usize,
            0x2010_445c
        );
        assert_eq!(
            callbacks.cts_scck_rate_table().as_ptr() as usize,
            0x2010_4460
        );
        assert_eq!(
            callbacks.bb_rx_hang_control().as_ptr() as usize,
            0x2010_4c1c
        );
        assert_eq!(callbacks.delay_secondary().as_ptr() as usize, 0x2010_4c54);
        assert_eq!(callbacks.delay_primary().as_ptr() as usize, 0x2010_4c58);
    }

    #[test]
    fn mac_txrx_callbacks_reject_out_of_range_slot_before_mmio() {
        // SAFETY: the rejected input returns before any generated register is
        // accessed; the host test therefore performs no volatile MMIO.
        let mut registers = WifiRadioRegisters::for_test();
        assert!(!registers.initialize_mac_txrx_callbacks(11));
        assert!(!registers.initialize_mac_txrx_callbacks(u8::MAX));
    }

    #[test]
    fn generated_mac_txrx_suffix_matches_complete_leaf_geometry() {
        // This host test inspects generated register pointers only and
        // performs no volatile access.
        let registers = WifiRadioRegisters::for_test();
        let init = &registers.peripherals.wifi_mac.wifi_mac_txrx_suffix;
        assert_eq!(init.aux_enable().as_ptr() as usize, 0x2010_4308);
        assert_eq!(
            registers
                .peripherals
                .wifi_mac
                .wifi_mac_txrx_callbacks
                .bb_rx_hang_control()
                .as_ptr() as usize,
            0x2010_4c1c
        );
        assert_eq!(init.default_image_a().as_ptr() as usize, 0x2010_4c20);
        assert_eq!(
            registers
                .peripherals
                .shared_radio
                .shared_radio_init_control
                .control()
                .as_ptr() as usize,
            0x2010_4c24
        );
        assert_eq!(init.gate_control().as_ptr() as usize, 0x2010_4c60);
        assert_eq!(init.field_control().as_ptr() as usize, 0x2010_4ca8);
        assert_eq!(
            registers
                .peripherals
                .wifi_mac
                .wifi_mac_rx_dma
                .rx_control()
                .as_ptr() as usize,
            0x2010_4080
        );
    }

    #[test]
    fn indexed_mac_registers_are_bounded_and_aligned() {
        for group in [
            &mac::init::BSSID_LOW[..],
            &mac::init::INTERFACE_ADDRESS_LOW[..],
            &mac::init::INTERFACE_ADDRESS_HIGH,
            &mac::init::RX_FILTER,
            &mac::init::BSSID_HIGH,
            &mac::init::RX_QUEUE_DEFAULT,
            &mac::init::LAST_RX_BUFFER,
            &mac::init::CRYPTO_BYPASS,
        ] {
            for &register in group {
                assert_valid(register);
            }
        }
    }

    #[test]
    fn mac_init_aliases_share_canonical_register_identities() {
        assert_eq!(mac::init::R_4098, mac::RX_CSI_CONFIG);
    }
}
