#![no_std]
#![deny(unsafe_code)]

#[cfg(test)]
extern crate std;

mod agc;
mod agc_runtime;
mod baseband;
mod bluetooth_baseband;
mod bluetooth_controller_hal_init;
mod bluetooth_controller_time;
mod bluetooth_interrupt;
mod bluetooth_memory_lists;
mod bluetooth_modem_lp_timer;
mod bluetooth_phy_init;
mod bluetooth_scheduler;
mod bluetooth_scheduler_lock_modify;
mod bluetooth_scheduler_runtime;
mod bluetooth_scheduler_stop;
mod cfr;
pub mod clock;
mod coex;
mod frequency;
// The generated capability catalog is intentionally broader than this crate's
// restricted ownership facade. Some reviewed leaves stay unreachable until an
// owner transition exposes them; do not reopen full-block access to make them
// appear used.
#[allow(
    dead_code,
    reason = "generated capability catalog is wider than the restricted ownership facade"
)]
mod generated;
mod ieee802154;
mod ieee802154_timing;
mod iq_estimator;
mod lp_phy_aux;
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
mod modem_lpcon_phy;
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
    BluetoothControllerHalInitConfig, BluetoothControllerTimeScale, BluetoothHalInitPeriod,
    BluetoothHalInitScale, BluetoothRawTimeDeltaProjection,
};
pub use bluetooth_controller_time::{
    BluetoothControllerLatchedTime, BluetoothControllerTimeLatchBeginError,
    BluetoothControllerTimeLatchObservation, BluetoothControllerTimeLatchRequest,
    BluetoothControllerTimeLatchStep, BluetoothControllerTimeLatchStepError,
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
pub use bluetooth_modem_lp_timer::{
    BluetoothModemLpTimerCompareDisposition, BluetoothModemLpTimerCounterObservation,
    BluetoothModemLpTimerEpoch, BluetoothModemLpTimerHandlerPending,
    BluetoothModemLpTimerHandlerRegisterObservation, BluetoothModemLpTimerHandlerRegisterStep,
    BluetoothModemLpTimerInstant, BluetoothModemLpTimerInterruptObservation,
    BluetoothModemLpTimerInterruptReady, BluetoothModemLpTimerInterruptStep,
    BluetoothModemLpTimerRegistersPrepared, BluetoothModemLpTimerSoftwarePending,
};
pub use bluetooth_scheduler_lock_modify::{
    BluetoothSchedulerLockModifyInterruptObservation, BluetoothSchedulerLockModifyObservation,
    BluetoothSchedulerLockModifyPublished, BluetoothSchedulerLockModifyRequest,
    BluetoothSchedulerLockModifyRequestError, BluetoothSchedulerLockModifyTaskObservation,
};
pub use bluetooth_scheduler_runtime::{
    BluetoothSchedulerFinishedListObservation, BluetoothSchedulerReferenceCleared,
    BluetoothSchedulerReferenceGateObservation, BluetoothSchedulerWorkObservation,
};
pub use bluetooth_scheduler_stop::{
    BluetoothSchedulerDisableBeginError, BluetoothSchedulerDisableBeginFailure,
    BluetoothSchedulerDisableBusyObserved, BluetoothSchedulerDisableIdleObserved,
    BluetoothSchedulerDisableRequest, BluetoothSchedulerDisableStep,
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
pub use ieee802154::Ieee802154RouteRawReadback;
pub use ieee802154::{
    Ieee802154AckTimeoutUnits, Ieee802154CcaMode, Ieee802154EdCcaSnapshot, Ieee802154EdCommand,
    Ieee802154EdDurationUnits, Ieee802154EdSampleRate, Ieee802154EventEnableMask,
    Ieee802154EventObservation, Ieee802154FoundationSnapshot, Ieee802154FrequencyCode,
    Ieee802154InterruptSnapshot, Ieee802154MacCommand, Ieee802154MacConfigurationReadback,
    Ieee802154MacControl, Ieee802154MacPolicySnapshot, Ieee802154MultipanEnableMask,
    Ieee802154MultipanIndex, Ieee802154OperationEventEnableObservation,
    Ieee802154OperationRxAbortEnableObservation, Ieee802154PanIdentity, Ieee802154Pti,
    Ieee802154RxAbortEnableMask, Ieee802154RxStateCode, Ieee802154RxStatusObservation,
    Ieee802154SecurityPayloadOffset, Ieee802154StateSnapshot, Ieee802154Timer0ThresholdWord,
    Ieee802154Timer0ValueWord, Ieee802154Timer1ThresholdWord, Ieee802154Timer1ValueWord,
    Ieee802154TimerLease, Ieee802154TransmitSecurityControl, Ieee802154TxPowerCode,
    Ieee802154TxStateCode,
};
#[doc(hidden)]
pub use ieee802154::{Ieee802154PolledRegisterLease, Ieee802154RegisterLease};
pub use ieee802154_timing::{Ieee802154TimingPrerequisite, Ieee802154TimingReady};
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

/// Physical owners used by one exclusive IEEE 802.15.4 route.
///
/// The Bluetooth controller partition is intentionally nested behind the
/// BTBB boundary. ESP-IDF's public IEEE 802.15.4 enable order calls the shared
/// `esp_btbb_enable` lifecycle, but does not grant IEEE 802.15.4 authority over
/// the Bluetooth controller. Keeping the complete generated partition private
/// lets reviewed BTBB transactions be added without exposing BLE/EDR methods.
struct Ieee802154TaskPeripheralOwners {
    ieee802154_mac: svd::ieee802154_mac_ownership::TaskRegisters,
    radio_phy: RadioPhyRegisters,
    coexistence: svd::peripheral_ownership::CoexistencePeripherals,
    btbb: Ieee802154BtbbPeripheralOwners,
}

/// Generated partitions retained behind the narrow IEEE 802.15.4 BTBB role.
struct Ieee802154BtbbPeripheralOwners {
    bluetooth: svd::peripheral_ownership::BluetoothControllerPeripherals,
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

/// Protocol owners that IEEE 802.15.4 does not use during its exclusive epoch.
struct RetainedIeee802154PeripheralOwners {
    wifi_mac: svd::peripheral_ownership::WifiMacPeripherals,
    wifi_interrupts: svd::peripheral_ownership::WifiInterruptPeripherals,
    bluetooth_interrupts: svd::peripheral_ownership::BluetoothInterruptPeripherals,
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
/// let _ieee802154 = hardware.into_ieee802154();
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
                controller_time_latch:
                    bluetooth_controller_time::BluetoothControllerTimeLatchOwnership::new(),
            },
            interrupts: BluetoothInterruptSetup {
                peripherals: bluetooth_interrupts,
            },
        }
    }

    /// Consume the neutral root into the exclusive standalone IEEE 802.15.4
    /// route.
    ///
    /// This ownership-only transition follows the same whole-radio rule as
    /// [`Self::into_wifi`] and [`Self::into_bluetooth`]. It performs no module
    /// clock, common-PHY, BTBB, coexistence, reset, DMA, or interrupt
    /// transaction. The route owns the IEEE 802.15.4 MAC and shared resources
    /// required by the public ESP-IDF enable sequence; Wi-Fi and Bluetooth IRQ
    /// authority remain retained and inaccessible.
    pub fn into_ieee802154(self) -> Ieee802154ColdRegisters {
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
        let (task_mac, interrupt_mac) =
            svd::ieee802154_mac_ownership::split(ieee802154.ieee802154_mac);
        Ieee802154ColdRegisters {
            task: Ieee802154TaskRegisters {
                peripherals: Ieee802154TaskPeripheralOwners {
                    ieee802154_mac: task_mac,
                    radio_phy: RadioPhyRegisters {
                        peripherals: radio_phy,
                    },
                    coexistence,
                    btbb: Ieee802154BtbbPeripheralOwners {
                        bluetooth,
                        shared_radio,
                    },
                },
                retained: RetainedIeee802154PeripheralOwners {
                    wifi_mac,
                    wifi_interrupts,
                    bluetooth_interrupts,
                },
            },
            interrupts: Ieee802154InterruptSetup {
                registers: interrupt_mac,
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
/// No address-bearing register catalog is exposed:
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

/// Exclusive standalone IEEE 802.15.4 owner before any IRQ handoff.
///
/// Unlike the old temporary Wi-Fi borrow, this value is produced only by
/// consuming the complete [`RadioHardware`] root. It therefore owns the MAC,
/// common PHY, shared BTBB words, and coexistence partition in one affine
/// epoch. Possession alone does not claim that their enable sequence ran.
#[must_use = "the cold IEEE 802.15.4 route retains every radio owner"]
pub struct Ieee802154ColdRegisters {
    task: Ieee802154TaskRegisters,
    interrupts: Ieee802154InterruptSetup,
}

impl Ieee802154ColdRegisters {
    /// Separate the ordinary task owner from the inactive interrupt owner.
    ///
    /// This conversion performs no MMIO. The returned interrupt setup cannot
    /// sample or acknowledge events until its consuming activation
    /// transaction has published the runtime event mask and cleared stale
    /// status.
    pub fn separate_interrupt_owner(self) -> (Ieee802154TaskRegisters, Ieee802154InterruptSetup) {
        (self.task, self.interrupts)
    }

    /// Return every Wi-Fi, Bluetooth, IEEE 802.15.4, and shared owner to the
    /// protocol-neutral root.
    ///
    /// This conversion performs no MMIO. A higher layer must first finish its
    /// state-specific STOP and shared-resource teardown sequence.
    pub fn release(self) -> RadioHardware {
        self.task.into_hardware(self.interrupts)
    }

    /// Borrow the dedicated IEEE 802.15.4 radio owner during cold setup.
    #[doc(hidden)]
    pub const fn radio(&self) -> &Ieee802154TaskRegisters {
        &self.task
    }

    /// Mutably borrow the dedicated IEEE 802.15.4 radio owner during cold
    /// setup.
    #[doc(hidden)]
    pub fn radio_mut(&mut self) -> &mut Ieee802154TaskRegisters {
        &mut self.task
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
    peripherals: Ieee802154TaskPeripheralOwners,
    retained: RetainedIeee802154PeripheralOwners,
}

impl Ieee802154TaskRegisters {
    fn into_hardware(self, interrupts: Ieee802154InterruptSetup) -> RadioHardware {
        let Self {
            peripherals:
                Ieee802154TaskPeripheralOwners {
                    ieee802154_mac: task_mac,
                    radio_phy,
                    coexistence,
                    btbb:
                        Ieee802154BtbbPeripheralOwners {
                            bluetooth,
                            shared_radio,
                        },
                },
            retained:
                RetainedIeee802154PeripheralOwners {
                    wifi_mac,
                    wifi_interrupts,
                    bluetooth_interrupts,
                },
        } = self;
        let ieee802154_mac = svd::ieee802154_mac_ownership::reunite(task_mac, interrupts.registers);
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

    /// Reunite a quiescent task owner with its inactive IRQ owner.
    ///
    /// This conversion performs no MMIO. The caller must first disable the
    /// CPU route and deactivate the finite interrupt epoch.
    pub fn into_cold(self, interrupts: Ieee802154InterruptSetup) -> Ieee802154ColdRegisters {
        Ieee802154ColdRegisters {
            task: self,
            interrupts,
        }
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
    registers: svd::ieee802154_mac_ownership::InterruptRegisters,
}

/// Disjoint IEEE 802.15.4 event/status capability for the hard ISR.
///
/// Task command, DMA, policy, and event-enable operations are absent from this
/// type. It must be deactivated after the platform CPU route is disabled and
/// reunited with the task owner before the whole radio can be released.
#[must_use = "the IEEE 802.15.4 interrupt owner must be deactivated and reunited"]
pub struct Ieee802154InterruptRegisters {
    registers: svd::ieee802154_mac_ownership::InterruptRegisters,
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
    controller_time_latch: bluetooth_controller_time::BluetoothControllerTimeLatchOwnership,
}

/// Why a task owner cannot be reunited with its inactive interrupt bank.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothTaskReuniteError {
    /// A controller-time request still belongs to the task-side worker.
    ControllerTimeLatchInFlight,
}

/// Failed Bluetooth owner reunion retaining both unique owners.
#[must_use = "the Bluetooth task and interrupt owners remain live after failed reunion"]
pub struct BluetoothTaskReuniteFailure {
    task: BluetoothTaskRegisters,
    interrupts: BluetoothInterruptSetup,
    error: BluetoothTaskReuniteError,
}

impl BluetoothTaskReuniteFailure {
    /// Return the finite reason without releasing either owner.
    pub const fn error(&self) -> BluetoothTaskReuniteError {
        self.error
    }

    /// Recover both unchanged owners and the failure reason.
    pub fn into_parts(
        self,
    ) -> (
        BluetoothTaskRegisters,
        BluetoothInterruptSetup,
        BluetoothTaskReuniteError,
    ) {
        (self.task, self.interrupts, self.error)
    }
}

impl core::fmt::Debug for BluetoothTaskReuniteFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothTaskReuniteFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
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
            controller_time_latch: _,
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
    /// This conversion performs no MMIO. It fails while the task owner retains
    /// an unfinished controller-time latch, returning both owners unchanged.
    pub fn into_cold(
        self,
        interrupts: BluetoothInterruptSetup,
    ) -> Result<BluetoothColdRegisters, BluetoothTaskReuniteFailure> {
        if self.controller_time_latch.in_flight() {
            return Err(BluetoothTaskReuniteFailure {
                task: self,
                interrupts,
                error: BluetoothTaskReuniteError::ControllerTimeLatchInFlight,
            });
        }
        Ok(BluetoothColdRegisters {
            task: self,
            interrupts,
        })
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
    use super::{MacInterruptMask, RadioHardware};

    #[test]
    fn cold_owner_is_consumed_by_interrupt_setup_split() {
        let registers = RadioHardware::for_validation().into_wifi();
        let (_running, _setup) = registers.into_running();
    }

    #[test]
    fn wifi_route_roundtrip_returns_the_complete_root() {
        let wifi = RadioHardware::for_validation().into_wifi();
        let (task, setup) = wifi.into_running();
        let hardware = task.into_cold(setup).release();

        let bluetooth = hardware.into_bluetooth();
        let _hardware = bluetooth.release();
    }

    #[test]
    fn bluetooth_task_and_interrupt_owners_roundtrip_without_mmio() {
        let bluetooth = RadioHardware::for_validation().into_bluetooth();
        let (task, setup) = bluetooth.separate_interrupt_owner();
        let hardware = task
            .into_cold(setup)
            .expect("an idle Bluetooth task owner can be reunited")
            .release();

        let wifi = hardware.into_wifi();
        let _hardware = wifi.release();
    }

    #[test]
    fn ieee802154_route_roundtrip_returns_every_other_protocol_owner() {
        let ieee802154 = RadioHardware::for_validation().into_ieee802154();
        let hardware = ieee802154.release();

        // The IEEE 802.15.4 epoch retains the complete generated Bluetooth
        // controller partition behind its BTBB role and never consumes either
        // protocol's interrupt owner.
        let bluetooth = hardware.into_bluetooth();
        let hardware = bluetooth.release();
        let wifi = hardware.into_wifi();
        let _hardware = wifi.release();
    }

    #[test]
    fn ieee802154_task_and_interrupt_owners_reunite_without_mmio() {
        let ieee802154 = RadioHardware::for_validation().into_ieee802154();
        let (task, setup) = ieee802154.separate_interrupt_owner();
        let hardware = task.into_cold(setup).release();

        let bluetooth = hardware.into_bluetooth();
        let _hardware = bluetooth.release();
    }

    #[test]
    fn mac_hal_tail_rejects_out_of_range_calibration_before_mmio() {
        let mut registers = RadioHardware::for_validation().into_wifi();
        assert!(!registers.initialize_mac_hal_tail(MacInterruptMask::COLD_RX, 0x0004_0000));
        assert!(!registers.initialize_mac_hal_tail(MacInterruptMask::NONE, u32::MAX));
    }

    #[test]
    fn mac_txrx_callbacks_reject_out_of_range_slot_before_mmio() {
        let mut registers = RadioHardware::for_validation().into_wifi().into_running().0;
        assert!(!registers.initialize_mac_txrx_callbacks(11));
        assert!(!registers.initialize_mac_txrx_callbacks(u8::MAX));
    }
}
