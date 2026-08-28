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
mod modem_shared_clock;
mod modem_syscon;
pub mod pbus;
pub mod phy;
pub mod phy_i2c;
mod platform_clock_power;
mod table_memory;
#[cfg(feature = "validation-probes")]
pub mod validation;
#[cfg(feature = "validation-probes")]
mod validation_transactions;
pub use agc_runtime::ForcedRxGain;
pub use baseband::{
    BluetoothTxPowerControlPrepareError, BluetoothTxPowerControlRestoreError,
    RxDcoControlPrepareError, RxDcoControlRestoreError, TxDcPwdetLifecycleError,
    TxDcPwdetPrepareError, TxDcPwdetRestoreError, TxIqToneControlPrepareError,
    TxIqToneControlRestoreError,
};
pub use bluetooth_controller_hal_init::{
    BluetoothControllerHalInitConfig, BluetoothControllerTimeScale, BluetoothHalInitPeriod,
    BluetoothHalInitScale, BluetoothRawTimeDeltaProjection,
};
pub use bluetooth_controller_time::{
    BluetoothControllerLatchedTime, BluetoothControllerTimeLatchBeginError,
    BluetoothControllerTimeLatchRequest, BluetoothControllerTimeLatchStep,
    BluetoothControllerTimeLatchStepError,
};
pub use bluetooth_interrupt::{
    BluetoothInterruptOutputPrepared, BluetoothNrtInterruptAcknowledged,
    BluetoothPrimaryFaultSources, BluetoothPrimaryInterruptEpoch,
    BluetoothSchedulerRunInterruptsPrepared,
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
pub use bluetooth_scheduler::{
    BluetoothSchedulerHardwareListHead, BluetoothSchedulerHardwareListHeadError,
    BluetoothSchedulerHardwareListHeadPublished, BluetoothSchedulerHardwareRunCommandPublished,
    BluetoothSchedulerInsertionCommand, BluetoothSchedulerInsertionCommandStartCleared,
};
pub use bluetooth_scheduler_lock_modify::{
    BluetoothSchedulerLockModifyInterruptObservation, BluetoothSchedulerLockModifyObservation,
    BluetoothSchedulerLockModifyPublished, BluetoothSchedulerLockModifyRequest,
    BluetoothSchedulerLockModifyRequestError, BluetoothSchedulerLockModifyTaskObservation,
};
pub use bluetooth_scheduler_runtime::{
    BluetoothSchedulerFinishedListObservation, BluetoothSchedulerFinishedListPop,
    BluetoothSchedulerHardwareListIndex, BluetoothSchedulerReferenceCleared,
    BluetoothSchedulerReferenceGateObservation, BluetoothSchedulerWorkObservation,
};
pub use bluetooth_scheduler_stop::{
    BluetoothSchedulerDisableBeginError, BluetoothSchedulerDisableBeginFailure,
    BluetoothSchedulerDisableBusyObserved, BluetoothSchedulerDisableIdleObserved,
    BluetoothSchedulerDisableRequest, BluetoothSchedulerDisableStep,
};
pub use cfr::CfrValue;
pub use coex::{COEX_TIMER_COUNT, CoexTimerRegister};
pub use frequency::PhyFrequencyI2cNumberAddresses;
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
    CoexTimerClientValue, CoexTimerPtiValue, CoexTimerTickImage, MacAssociationId,
    MacExtraSoftApRxBlockAckEntryIndex, MacHeBssColor, MacHeDefaultPacketExtensionDuration,
    MacHePacketPaddingDuration, MacInterface, MacItwtClearIndex, MacKeyEntryIndex,
    MacMinimumMpduStartSpacing, MacPti, MacRxBlockAckEntryIndex, MacRxBlockAckStartingSequence,
    MacRxBlockAckTid, MacRxBlockAckWindow, MacTxPtiCount, MacTxQueueIndex,
    ModemLowPowerClockDivider, PhyForcedPowerIndex, PhyFtmEnableVendorArgument,
};
pub use mac_crypto::MacCcmpKeyIdentity;

const BLUETOOTH_MAIN_XTAL_LOW_POWER_DIVIDER: ModemLowPowerClockDivider =
    match ModemLowPowerClockDivider::new(399) {
        Some(divider) => divider,
        None => panic!("reviewed Bluetooth low-power divider exceeds its PAC field"),
    };
pub use ieee802154::{
    Ieee802154AckTimeoutUnits, Ieee802154CcaMode, Ieee802154EdCcaSnapshot, Ieee802154EdCommand,
    Ieee802154EdDurationUnits, Ieee802154EdSampleRate, Ieee802154Event, Ieee802154EventEnableState,
    Ieee802154EventMask, Ieee802154EventObservation, Ieee802154EventObservationError,
    Ieee802154FoundationSnapshot, Ieee802154FrequencyCode, Ieee802154InterruptSnapshot,
    Ieee802154MacCommand, Ieee802154MacConfigurationReadback, Ieee802154MacControl,
    Ieee802154MacPolicySnapshot, Ieee802154MultipanEnableState, Ieee802154MultipanIndex,
    Ieee802154ObservedEventState, Ieee802154OperationEventEnableObservation,
    Ieee802154OperationRxAbortEnableObservation, Ieee802154PanIdentity, Ieee802154Pti,
    Ieee802154RouteState, Ieee802154RxAbortEnableState, Ieee802154RxAbortReason,
    Ieee802154RxAbortReasonObservation, Ieee802154RxStateCode, Ieee802154SecurityPayloadOffset,
    Ieee802154StateSnapshot, Ieee802154Timer0ThresholdWord, Ieee802154Timer0ValueWord,
    Ieee802154Timer1ThresholdWord, Ieee802154Timer1ValueWord, Ieee802154TimerLease,
    Ieee802154TransmitSecurityControl, Ieee802154TxAbortReason, Ieee802154TxAbortReasonObservation,
    Ieee802154TxPowerCode, Ieee802154TxStateCode, Ieee802154ValidationEdDurationState,
    Ieee802154ValidationEventEnableState,
};
#[doc(hidden)]
pub use ieee802154::{Ieee802154PolledRegisterLease, Ieee802154RegisterLease};
pub use ieee802154_timing::{Ieee802154TimingPrerequisite, Ieee802154TimingReady};
pub use mac_block_ack::{
    ExtraSoftApRxBlockAckEntrySnapshot, InternalTxBlockAckSnapshot, RxBlockAckEntrySnapshot,
    TxBlockAckDiagnosticSnapshot, TxBlockAckPayload,
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
    ConnectedStaWithoutPowerSavePrepared, MacInterruptEnableState, MacInterruptRegisters,
    MacInterruptSetup, MacPowerInterruptRegisters, MacPowerWakeCause, MacTsfTimerIndex,
};
pub use mac_modem_wakeup::{
    StaBeaconMissLimit, StaBeaconMissTimeoutRaw, StaModemSleepLimit, StaModemWakeConfig,
    StaModemWakePrepareError, StaModemWakeRestore, StaModemWakeRestoreError,
    StaModemWakeRestoreFailure, StaTbttAutoPeriod, StaWakeProtectEarlyTimeRaw,
};
pub use mac_rx_dma::{MacRxDmaSnapshot, MacRxNextDescriptorObservation};
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
    MacHeFecCoding, MacHeGuardIntervalAndLtf, MacHeMcs, MacHeRate, MacHeTxFormat,
    MacHeTxParameters, MacHeTxProgram, MacHtAmpduCompletionObservation, MacHtChannelWidth,
    MacHtGuardInterval, MacHtMcs, MacHtProtectionSpacing, MacHtRate, MacHtTxFormat,
    MacHtTxParameters, MacHtTxProgram, MacLegacyRate, MacLegacyTxParameters, MacLegacyTxProgram,
    MacTxCompletionObservation, MacTxDetachOutcome, MacTxDetachReason, MacTxPtiProgram,
    MacTxQueueDetached,
};
pub use mac_tx_power_init::{
    MAC_TX_POWER_RATE_COUNT, MacPartialRuPowerSelector, MacTxPowerIndex, MacTxPowerPair,
    MacTxPowerTable,
};
pub use modem_shared_clock::{
    BluetoothLowPowerClockObservation, CoexistenceLowPowerClockObservation,
    CoexistenceLowPowerClockSource, ModemLowPowerClockSource, SharedModemClockObservation,
};
use modem_shared_clock::{BluetoothLowPowerTimerLease, SharedModemClock, SharedModemClockLease};
use modem_syscon::BluetoothModemSysconClockState;
pub use modem_syscon::{
    ModemSysconBluetoothObservation, ModemSysconIeee802154ClockObservation,
    ModemSysconIeee802154ResetObservation, ModemSysconPowerObservation, WifiBasebandAgcUpdate,
};
use open_esp_radio_esp32s31_pac_raw as svd;
pub use phy_i2c::{
    BluetoothTxPowerControlAction, BluetoothTxPowerControlCompletion, BluetoothTxPowerControlError,
    BluetoothTxPowerControlObservation, BluetoothTxPowerControlOperation,
    BluetoothTxPowerControlTransaction, PhyAdcRate, PhyFilterDcapInputs, PhyI2cAccessError,
    PhyI2cAddress, PhyI2cBlock, PhyI2cCommandMemoryInputs, PhyI2cConfigurationAction,
    PhyI2cConfigurationError, PhyI2cConfigurationObservation, PhyI2cConfigurationOperation,
    PhyI2cConfigurationTransaction, PhyI2cField, PhyI2cHost, PhyI2cInitializationStageOneInputs,
    analog_registers,
};
pub use platform_clock_power::PlatformClockPowerObservation;
pub use table_memory::{PbusMemoryGroupBoundary, PhyGainMemoryEntry, PhyMemoryError};

/// Private Wi-Fi and shared-radio owners used by one exclusive Wi-Fi route.
struct WifiRadioPeripheralOwners {
    wifi_mac: svd::peripheral_ownership::WifiMacPeripherals,
    ieee802154: svd::peripheral_ownership::Ieee802154Peripherals,
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
    ieee802154_interrupt_route: svd::Ieee802154InterruptRoute,
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
    shared_clock: modem_shared_clock::SharedModemClockState,
    platform_clock_power: platform_clock_power::PlatformClockPowerState,
    restore_slot: baseband::RadioPhyRestoreSlot,
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
}

/// Failed neutral-root release retaining the cold route owner unchanged.
#[must_use = "the cold route owner remains live after failed release"]
pub struct RadioPhyReleaseFailure<Owner> {
    owner: Owner,
    error: RadioPhyReleaseError,
}

impl<Owner> RadioPhyReleaseFailure<Owner> {
    pub const fn error(&self) -> RadioPhyReleaseError {
        self.error
    }

    pub fn into_parts(self) -> (Owner, RadioPhyReleaseError) {
        (self.owner, self.error)
    }
}

impl<Owner> core::fmt::Debug for RadioPhyReleaseFailure<Owner> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("RadioPhyReleaseFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
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
    radio_phy: RadioPhyRegisters,
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
            radio_phy: RadioPhyRegisters {
                peripherals: radio_phy,
                shared_clock: modem_shared_clock::SharedModemClockState::new(),
                platform_clock_power: platform_clock_power::PlatformClockPowerState::new(),
                restore_slot: baseband::RadioPhyRestoreSlot::new(),
            },
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
                    ieee802154,
                    radio_phy,
                    coexistence,
                    shared_radio,
                },
                retained_bluetooth: RetainedBluetoothPeripheralOwners {
                    bluetooth,
                    bluetooth_interrupts,
                },
                phy_i2c_clock: None,
                coexistence_clock: None,
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
                radio_phy,
                coexistence,
                shared_radio,
                retained_wifi: RetainedWifiPeripheralOwners {
                    wifi_mac,
                    wifi_interrupts,
                    ieee802154,
                },
                coexistence_clock: None,
                low_power_timer_clock: None,
                platform_pll_source: None,
                modem_syscon_clocks: BluetoothModemSysconClockState::new(),
                modem_syscon_controller_clocks_retained: false,
                modem_syscon_apb_clocks_retained: false,
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
        let svd::peripheral_ownership::Ieee802154Peripherals {
            ieee802154_mac,
            ieee802154_interrupt_route,
        } = ieee802154;
        let (task_mac, interrupt_mac) = svd::ieee802154_mac_ownership::split(ieee802154_mac);
        Ieee802154ColdRegisters {
            task: Ieee802154TaskRegisters {
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
                retained: RetainedIeee802154PeripheralOwners {
                    wifi_mac,
                    wifi_interrupts,
                    bluetooth_interrupts,
                },
                phy_i2c_clock: None,
                coexistence_clock: None,
            },
            interrupts: Ieee802154InterruptSetup {
                registers: interrupt_mac,
            },
        }
    }
}

/// Semantic MAC work causes recovered from reviewed vendor transactions.
///
/// This type is deliberately not a register image. The generated PAC owns
/// STATUS field geometry; higher layers can only combine named causes.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MacInterruptEvents {
    tx_complete: bool,
    collision: bool,
    rx_success: bool,
    tx_timeout: bool,
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
    work_events: MacInterruptEvents,
    auxiliary_event: bool,
    unhandled_event: bool,
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
pub struct MacInterruptSnapshot(svd::interrupt_snapshot::MacInterruptSnapshot);

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
pub struct MacPowerInterruptSnapshot(svd::interrupt_snapshot::MacPowerInterruptSnapshot);

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
pub struct MacPowerInterruptObservation(u8);

impl MacPowerInterruptObservation {
    // Driver-local semantic flags. Their positions intentionally do not match
    // WDEVPWR register geometry, which remains behind generated accessors.
    const TSF_TIMER_0: u8 = 0x01;
    const TSF_TIMER_1: u8 = 0x02;
    const TSF_TIMER_2: u8 = 0x04;
    const TSF_TIMER_3: u8 = 0x08;
    const UNHANDLED_EVENT: u8 = 0x10;

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
    phy_i2c_clock: Option<SharedModemClockLease>,
    coexistence_clock: Option<SharedModemClockLease>,
    station_tbtt_wake_prepared: bool,
    station_modem_wakeup: mac_modem_wakeup::StaModemWakeOwnership,
}

impl WifiRadioRegisters {
    /// Prepare the shared state maps and retain the PHY-I2C clock for this
    /// complete Wi-Fi route epoch.
    pub fn prepare_shared_modem_clock_map(&mut self) {
        self.peripherals.radio_phy.prepare_shared_modem_clock_map();
    }

    /// Retain the PHY-I2C gate at its exact late power-sequence edge.
    pub fn retain_phy_i2c_master_clock(&mut self) {
        if self.phy_i2c_clock.is_none() {
            self.phy_i2c_clock = Some(
                self.peripherals
                    .radio_phy
                    .retain_shared_modem_clock(SharedModemClock::PhyI2cMaster),
            );
        }
    }

    /// Retain the coexistence clock once for the Wi-Fi MAC epoch.
    pub fn retain_coexistence_clock(&mut self) {
        if self.coexistence_clock.is_none() {
            self.coexistence_clock = Some(
                self.peripherals
                    .radio_phy
                    .retain_shared_modem_clock(SharedModemClock::Coexistence),
            );
        }
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

    fn release_retained_shared_clocks(&mut self) {
        if let Some(lease) = self.coexistence_clock.take() {
            self.peripherals.radio_phy.release_shared_modem_clock(lease);
        }
        if let Some(lease) = self.phy_i2c_clock.take() {
            self.peripherals.radio_phy.release_shared_modem_clock(lease);
        }
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
    /// releasing the owner does not claim to modify baseband hardware state.
    ///
    /// # Errors
    ///
    /// Returns a failure retaining this owner while TX-DC PWDET, TX-IQ,
    /// RX-DCO, or Bluetooth TX-power control still awaits restoration. Recover it with
    /// [`RadioPhyReleaseFailure::into_parts`] and complete the pending restore
    /// first.
    pub fn release(mut self) -> Result<RadioHardware, RadioPhyReleaseFailure<Self>> {
        if self
            .registers
            .peripherals
            .radio_phy
            .txdc_pwdet_restore_pending()
        {
            return Err(RadioPhyReleaseFailure {
                owner: self,
                error: RadioPhyReleaseError::TxDcPwdetRestorePending,
            });
        }
        if self
            .registers
            .peripherals
            .radio_phy
            .txiq_tone_control_restore_pending()
        {
            return Err(RadioPhyReleaseFailure {
                owner: self,
                error: RadioPhyReleaseError::TxIqToneControlRestorePending,
            });
        }
        if self
            .registers
            .peripherals
            .radio_phy
            .rx_dco_control_restore_pending()
        {
            return Err(RadioPhyReleaseFailure {
                owner: self,
                error: RadioPhyReleaseError::RxDcoControlRestorePending,
            });
        }
        if self
            .registers
            .peripherals
            .radio_phy
            .bluetooth_tx_power_control_restore_pending()
        {
            return Err(RadioPhyReleaseFailure {
                owner: self,
                error: RadioPhyReleaseError::BluetoothTxPowerControlRestorePending,
            });
        }
        self.registers.release_retained_shared_clocks();
        let Self {
            registers:
                WifiRadioRegisters {
                    peripherals:
                        WifiRadioPeripheralOwners {
                            wifi_mac,
                            ieee802154,
                            radio_phy,
                            coexistence,
                            shared_radio,
                        },
                    retained_bluetooth:
                        RetainedBluetoothPeripheralOwners {
                            bluetooth,
                            bluetooth_interrupts,
                        },
                    phy_i2c_clock: _,
                    coexistence_clock: _,
                    station_tbtt_wake_prepared: _,
                    station_modem_wakeup: _,
                },
            interrupts: wifi_interrupts,
        } = self;
        Ok(RadioHardware {
            wifi_mac,
            wifi_interrupts,
            radio_phy,
            coexistence,
            bluetooth,
            bluetooth_interrupts,
            shared_radio,
            ieee802154,
        })
    }

    /// Prepare shared route clocks before the platform-owned power sequence
    /// reaches its readback checkpoint.
    #[doc(hidden)]
    pub fn prepare_shared_modem_clock_map(&mut self) {
        self.registers.prepare_shared_modem_clock_map();
    }

    /// Retain the route-owned PHY-I2C clock at its ordered power edge.
    #[doc(hidden)]
    pub fn retain_phy_i2c_master_clock(&mut self) {
        self.registers.retain_phy_i2c_master_clock();
    }

    /// Read the shared route-owned portion of the power checkpoint.
    #[doc(hidden)]
    pub fn shared_modem_clock_observation(&self) -> SharedModemClockObservation {
        self.registers.shared_modem_clock_observation()
    }

    #[doc(hidden)]
    pub fn select_hp_active_modem_icg(&mut self) {
        self.registers
            .peripherals
            .radio_phy
            .select_hp_active_modem_icg();
    }

    #[doc(hidden)]
    pub fn apply_modem_icg_selection(&mut self) {
        self.registers
            .peripherals
            .radio_phy
            .apply_modem_icg_selection();
    }

    #[doc(hidden)]
    pub fn apply_sleep_icg_selection(&mut self) {
        self.registers
            .peripherals
            .radio_phy
            .apply_sleep_icg_selection();
    }

    #[doc(hidden)]
    pub fn enable_modem_register_bus_clock(&mut self) {
        self.registers
            .peripherals
            .radio_phy
            .enable_modem_register_bus_clock();
    }

    #[doc(hidden)]
    pub fn configure_modem_source_clocks(&mut self) {
        self.registers
            .peripherals
            .radio_phy
            .configure_modem_source_clocks();
    }

    #[doc(hidden)]
    pub fn platform_clock_power_observation(&self) -> PlatformClockPowerObservation {
        self.registers
            .peripherals
            .radio_phy
            .platform_clock_power_observation()
    }

    #[doc(hidden)]
    pub fn set_wifi_baseband_and_mac_reset(&mut self, asserted: bool) {
        self.registers
            .peripherals
            .radio_phy
            .set_wifi_baseband_and_mac_reset(asserted);
    }

    #[doc(hidden)]
    pub fn set_wifi_baseband_reset(&mut self, asserted: bool) {
        self.registers
            .peripherals
            .radio_phy
            .set_wifi_baseband_reset(asserted);
    }

    #[doc(hidden)]
    pub fn enable_wifi_mac_clocks(&mut self) {
        self.registers
            .peripherals
            .radio_phy
            .enable_wifi_mac_clocks();
    }

    #[doc(hidden)]
    pub fn set_wifi_mac_reset(&mut self, asserted: bool) {
        self.registers
            .peripherals
            .radio_phy
            .set_wifi_mac_reset(asserted);
    }

    #[doc(hidden)]
    pub fn configure_wifi_power_clock_map(&mut self) {
        self.registers
            .peripherals
            .radio_phy
            .configure_wifi_power_clock_map();
    }

    #[doc(hidden)]
    pub fn enable_phy_calibration_clocks(&mut self) {
        self.registers
            .peripherals
            .radio_phy
            .enable_phy_calibration_clocks();
    }

    #[doc(hidden)]
    pub fn select_phy_i2c_160mhz_source(&mut self) {
        self.registers
            .peripherals
            .radio_phy
            .select_phy_i2c_160mhz_source();
    }

    #[doc(hidden)]
    pub fn modem_syscon_power_observation(&self) -> ModemSysconPowerObservation {
        self.registers
            .peripherals
            .radio_phy
            .modem_syscon_power_observation()
    }

    /// Preserve the vendor two-read coexistence clock sampling rule.
    #[doc(hidden)]
    pub fn sample_coexistence_low_power_clock(
        &self,
    ) -> Option<CoexistenceLowPowerClockObservation> {
        self.registers.sample_coexistence_low_power_clock()
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
    pub fn mac_interrupt_enable(&self) -> MacInterruptEnableState {
        mac_interrupt::observe_mac_interrupt_enable(&self.interrupts.wifi_mac_interrupt)
    }

    /// Mask every MAC event and acknowledge every stale cold event.
    pub fn mask_and_clear_all_mac_interrupts(&mut self) {
        let interrupt = &self.interrupts.wifi_mac_interrupt;
        mac_interrupt::publish_mac_interrupt_mask(interrupt, MacInterruptMask::NONE);
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
    ///
    /// # Errors
    ///
    /// Returns a failure retaining this owner while TX-DC PWDET, TX-IQ,
    /// RX-DCO, or Bluetooth TX-power control still awaits restoration. Recover it with
    /// [`RadioPhyReleaseFailure::into_parts`] and complete the pending restore
    /// first.
    pub fn release(self) -> Result<RadioHardware, RadioPhyReleaseFailure<Self>> {
        if self.task.peripherals.radio_phy.txdc_pwdet_restore_pending() {
            return Err(RadioPhyReleaseFailure {
                owner: self,
                error: RadioPhyReleaseError::TxDcPwdetRestorePending,
            });
        }
        if self
            .task
            .peripherals
            .radio_phy
            .txiq_tone_control_restore_pending()
        {
            return Err(RadioPhyReleaseFailure {
                owner: self,
                error: RadioPhyReleaseError::TxIqToneControlRestorePending,
            });
        }
        if self
            .task
            .peripherals
            .radio_phy
            .rx_dco_control_restore_pending()
        {
            return Err(RadioPhyReleaseFailure {
                owner: self,
                error: RadioPhyReleaseError::RxDcoControlRestorePending,
            });
        }
        if self
            .task
            .peripherals
            .radio_phy
            .bluetooth_tx_power_control_restore_pending()
        {
            return Err(RadioPhyReleaseFailure {
                owner: self,
                error: RadioPhyReleaseError::BluetoothTxPowerControlRestorePending,
            });
        }
        Ok(self.task.into_hardware(self.interrupts))
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
    phy_i2c_clock: Option<SharedModemClockLease>,
    coexistence_clock: Option<SharedModemClockLease>,
}

impl Ieee802154TaskRegisters {
    fn into_hardware(mut self, interrupts: Ieee802154InterruptSetup) -> RadioHardware {
        self.release_retained_shared_clocks();
        let Self {
            peripherals:
                Ieee802154TaskPeripheralOwners {
                    ieee802154_mac: task_mac,
                    ieee802154_interrupt_route,
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
            phy_i2c_clock: _,
            coexistence_clock: _,
        } = self;
        let ieee802154_mac = svd::ieee802154_mac_ownership::reunite(task_mac, interrupts.registers);
        RadioHardware {
            wifi_mac,
            wifi_interrupts,
            radio_phy,
            coexistence,
            bluetooth,
            bluetooth_interrupts,
            shared_radio,
            ieee802154: svd::peripheral_ownership::Ieee802154Peripherals {
                ieee802154_mac,
                ieee802154_interrupt_route,
            },
        }
    }

    pub fn prepare_shared_modem_clock_map(&mut self) {
        self.peripherals.radio_phy.prepare_shared_modem_clock_map();
    }

    pub fn retain_phy_i2c_master_clock(&mut self) {
        if self.phy_i2c_clock.is_none() {
            let lease = self
                .peripherals
                .radio_phy
                .retain_shared_modem_clock(SharedModemClock::PhyI2cMaster);
            self.phy_i2c_clock = Some(lease);
        }
    }

    pub fn retain_coexistence_clock(&mut self) {
        if self.coexistence_clock.is_none() {
            let lease = self
                .peripherals
                .radio_phy
                .retain_shared_modem_clock(SharedModemClock::Coexistence);
            self.coexistence_clock = Some(lease);
        }
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

    fn release_retained_shared_clocks(&mut self) {
        if let Some(lease) = self.coexistence_clock.take() {
            self.peripherals.radio_phy.release_shared_modem_clock(lease);
        }
        if let Some(lease) = self.phy_i2c_clock.take() {
            self.peripherals.radio_phy.release_shared_modem_clock(lease);
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
    ///
    /// # Errors
    ///
    /// Returns a failure retaining this owner while TX-DC PWDET, TX-IQ,
    /// RX-DCO, or Bluetooth TX-power control still awaits restoration. Recover it with
    /// [`RadioPhyReleaseFailure::into_parts`] and complete the pending restore
    /// first.
    pub fn release(self) -> Result<RadioHardware, RadioPhyReleaseFailure<Self>> {
        if self.task.radio_phy.txdc_pwdet_restore_pending() {
            return Err(RadioPhyReleaseFailure {
                owner: self,
                error: RadioPhyReleaseError::TxDcPwdetRestorePending,
            });
        }
        if self.task.radio_phy.txiq_tone_control_restore_pending() {
            return Err(RadioPhyReleaseFailure {
                owner: self,
                error: RadioPhyReleaseError::TxIqToneControlRestorePending,
            });
        }
        if self.task.radio_phy.rx_dco_control_restore_pending() {
            return Err(RadioPhyReleaseFailure {
                owner: self,
                error: RadioPhyReleaseError::RxDcoControlRestorePending,
            });
        }
        if self
            .task
            .radio_phy
            .bluetooth_tx_power_control_restore_pending()
        {
            return Err(RadioPhyReleaseFailure {
                owner: self,
                error: RadioPhyReleaseError::BluetoothTxPowerControlRestorePending,
            });
        }
        let Self { task, interrupts } = self;
        Ok(task.into_hardware(interrupts))
    }

    #[doc(hidden)]
    pub fn prepare_shared_modem_clock_map(&mut self) {
        self.task.prepare_shared_modem_clock_map();
    }

    #[doc(hidden)]
    pub fn retain_coexistence_clock(&mut self) {
        self.task.retain_coexistence_clock();
    }

    #[doc(hidden)]
    pub fn release_coexistence_clock(&mut self) {
        self.task.release_coexistence_clock();
    }

    #[doc(hidden)]
    pub fn retain_main_xtal_bluetooth_low_power_clock(&mut self) {
        self.task.retain_main_xtal_bluetooth_low_power_clock();
    }

    #[doc(hidden)]
    pub fn release_bluetooth_low_power_timer(&mut self) {
        self.task.release_bluetooth_low_power_timer();
    }

    #[doc(hidden)]
    pub fn bluetooth_shared_clock_observation(
        &self,
    ) -> (
        SharedModemClockObservation,
        BluetoothLowPowerClockObservation,
    ) {
        self.task.bluetooth_shared_clock_observation()
    }

    #[doc(hidden)]
    pub fn retain_platform_pll_source(&mut self) {
        self.task.retain_platform_pll_source();
    }

    #[doc(hidden)]
    pub fn release_platform_pll_source(&mut self) {
        self.task.release_platform_pll_source();
    }

    #[doc(hidden)]
    pub fn platform_clock_power_observation(&self) -> PlatformClockPowerObservation {
        self.task.radio_phy.platform_clock_power_observation()
    }

    #[doc(hidden)]
    pub fn prepare_modem_syscon_clock_map(&mut self) {
        self.task.radio_phy.prepare_modem_syscon_clock_map();
    }

    #[doc(hidden)]
    pub fn reset_modem_syscon_bluetooth_domains(&mut self) {
        self.task.radio_phy.reset_bluetooth_controller_domains();
    }

    #[doc(hidden)]
    pub fn modem_syscon_bluetooth_observation(&self) -> ModemSysconBluetoothObservation {
        self.task.radio_phy.bluetooth_clock_observation()
    }

    #[doc(hidden)]
    pub fn retain_modem_syscon_bluetooth_controller_clocks(&mut self) {
        self.task.retain_modem_syscon_bluetooth_controller_clocks();
    }

    #[doc(hidden)]
    pub fn retain_modem_syscon_bluetooth_apb_clocks(&mut self) {
        self.task.retain_modem_syscon_bluetooth_apb_clocks();
    }

    #[doc(hidden)]
    pub fn release_modem_syscon_bluetooth_apb_clocks(&mut self) {
        self.task.release_modem_syscon_bluetooth_apb_clocks();
    }

    #[doc(hidden)]
    pub fn release_modem_syscon_bluetooth_controller_clocks(&mut self) {
        self.task.release_modem_syscon_bluetooth_controller_clocks();
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
    coexistence_clock: Option<SharedModemClockLease>,
    low_power_timer_clock: Option<BluetoothLowPowerTimerLease>,
    platform_pll_source: Option<platform_clock_power::PlatformPllSourceLease>,
    modem_syscon_clocks: BluetoothModemSysconClockState,
    modem_syscon_controller_clocks_retained: bool,
    modem_syscon_apb_clocks_retained: bool,
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
    fn retain_platform_pll_source(&mut self) {
        if self.platform_pll_source.is_none() {
            self.platform_pll_source = Some(self.radio_phy.retain_platform_pll_source());
        }
    }

    fn release_platform_pll_source(&mut self) {
        if let Some(lease) = self.platform_pll_source.take() {
            self.radio_phy.release_platform_pll_source(lease);
        }
    }

    fn retain_modem_syscon_bluetooth_controller_clocks(&mut self) {
        if !self.modem_syscon_controller_clocks_retained {
            self.radio_phy
                .retain_bluetooth_controller_clocks(&mut self.modem_syscon_clocks);
            self.modem_syscon_controller_clocks_retained = true;
        }
    }

    fn retain_modem_syscon_bluetooth_apb_clocks(&mut self) {
        if !self.modem_syscon_apb_clocks_retained {
            self.radio_phy
                .retain_bluetooth_apb_clocks(&mut self.modem_syscon_clocks);
            self.modem_syscon_apb_clocks_retained = true;
        }
    }

    fn release_modem_syscon_bluetooth_apb_clocks(&mut self) {
        if self.modem_syscon_apb_clocks_retained {
            self.radio_phy
                .release_bluetooth_apb_clocks(&mut self.modem_syscon_clocks);
            self.modem_syscon_apb_clocks_retained = false;
        }
    }

    fn release_modem_syscon_bluetooth_controller_clocks(&mut self) {
        if self.modem_syscon_controller_clocks_retained {
            self.radio_phy
                .release_bluetooth_controller_clocks(&mut self.modem_syscon_clocks);
            self.modem_syscon_controller_clocks_retained = false;
        }
    }

    fn prepare_shared_modem_clock_map(&mut self) {
        self.radio_phy.prepare_shared_modem_clock_map();
    }

    fn retain_coexistence_clock(&mut self) {
        if self.coexistence_clock.is_none() {
            self.coexistence_clock = Some(
                self.radio_phy
                    .retain_shared_modem_clock(SharedModemClock::Coexistence),
            );
        }
    }

    fn release_coexistence_clock(&mut self) {
        if let Some(lease) = self.coexistence_clock.take() {
            self.radio_phy.release_shared_modem_clock(lease);
        }
    }

    fn retain_main_xtal_bluetooth_low_power_clock(&mut self) {
        if self.low_power_timer_clock.is_none() {
            self.low_power_timer_clock = Some(self.radio_phy.retain_bluetooth_low_power_timer(
                ModemLowPowerClockSource::Crystal,
                BLUETOOTH_MAIN_XTAL_LOW_POWER_DIVIDER,
            ));
        }
    }

    fn release_bluetooth_low_power_timer(&mut self) {
        if let Some(lease) = self.low_power_timer_clock.take() {
            self.radio_phy.release_bluetooth_low_power_timer(lease);
        }
    }

    fn bluetooth_shared_clock_observation(
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

    fn into_hardware(mut self, interrupts: BluetoothInterruptSetup) -> RadioHardware {
        self.release_bluetooth_low_power_timer();
        self.release_coexistence_clock();
        self.release_modem_syscon_bluetooth_apb_clocks();
        self.release_modem_syscon_bluetooth_controller_clocks();
        self.release_platform_pll_source();
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
            coexistence_clock: _,
            low_power_timer_clock: _,
            platform_pll_source: _,
            modem_syscon_clocks: _,
            modem_syscon_controller_clocks_retained: _,
            modem_syscon_apb_clocks_retained: _,
            controller_time_latch: _,
        } = self;
        RadioHardware {
            wifi_mac,
            wifi_interrupts,
            radio_phy,
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
    use super::{MacInterruptMask, RadioHardware, RadioPhyReleaseError};

    fn occupy_wifi_restore(registers: &mut super::WifiColdRegisters) {
        registers
            .registers
            .peripherals
            .radio_phy
            .occupy_txdc_pwdet_restore_for_test();
    }

    fn occupy_ieee802154_restore(registers: &mut super::Ieee802154ColdRegisters) {
        registers
            .task
            .peripherals
            .radio_phy
            .occupy_txdc_pwdet_restore_for_test();
    }

    fn occupy_bluetooth_restore(registers: &mut super::BluetoothColdRegisters) {
        registers
            .task
            .radio_phy
            .occupy_txdc_pwdet_restore_for_test();
    }

    fn occupy_wifi_txiq_restore(registers: &mut super::WifiColdRegisters) {
        registers
            .registers
            .peripherals
            .radio_phy
            .occupy_txiq_tone_control_restore_for_test();
    }

    fn occupy_ieee802154_txiq_restore(registers: &mut super::Ieee802154ColdRegisters) {
        registers
            .task
            .peripherals
            .radio_phy
            .occupy_txiq_tone_control_restore_for_test();
    }

    fn occupy_bluetooth_txiq_restore(registers: &mut super::BluetoothColdRegisters) {
        registers
            .task
            .radio_phy
            .occupy_txiq_tone_control_restore_for_test();
    }

    fn occupy_wifi_rx_dco_restore(registers: &mut super::WifiColdRegisters) {
        registers
            .registers
            .peripherals
            .radio_phy
            .occupy_rx_dco_control_restore_for_test();
    }

    fn occupy_ieee802154_rx_dco_restore(registers: &mut super::Ieee802154ColdRegisters) {
        registers
            .task
            .peripherals
            .radio_phy
            .occupy_rx_dco_control_restore_for_test();
    }

    fn occupy_bluetooth_rx_dco_restore(registers: &mut super::BluetoothColdRegisters) {
        registers
            .task
            .radio_phy
            .occupy_rx_dco_control_restore_for_test();
    }

    fn occupy_wifi_bluetooth_tx_power_restore(registers: &mut super::WifiColdRegisters) {
        registers
            .registers
            .peripherals
            .radio_phy
            .occupy_bluetooth_tx_power_control_restore_for_test();
    }

    fn occupy_ieee802154_bluetooth_tx_power_restore(
        registers: &mut super::Ieee802154ColdRegisters,
    ) {
        registers
            .task
            .peripherals
            .radio_phy
            .occupy_bluetooth_tx_power_control_restore_for_test();
    }

    fn occupy_bluetooth_tx_power_restore(registers: &mut super::BluetoothColdRegisters) {
        registers
            .task
            .radio_phy
            .occupy_bluetooth_tx_power_control_restore_for_test();
    }

    #[test]
    fn pending_txdc_restore_survives_same_route_transitions_and_blocks_release() {
        let mut wifi = RadioHardware::for_validation().into_wifi();
        occupy_wifi_restore(&mut wifi);
        let (task, interrupts) = wifi.into_running();
        let wifi = task.into_cold(interrupts);
        let Err(failure) = wifi.release() else {
            panic!("Wi-Fi released a pending restore");
        };
        assert_eq!(
            failure.error(),
            RadioPhyReleaseError::TxDcPwdetRestorePending
        );

        let mut ieee802154 = RadioHardware::for_validation().into_ieee802154();
        occupy_ieee802154_restore(&mut ieee802154);
        let (task, interrupts) = ieee802154.separate_interrupt_owner();
        let ieee802154 = task.into_cold(interrupts);
        let Err(failure) = ieee802154.release() else {
            panic!("IEEE 802.15.4 released a pending restore");
        };
        assert_eq!(
            failure.error(),
            RadioPhyReleaseError::TxDcPwdetRestorePending
        );

        let mut bluetooth = RadioHardware::for_validation().into_bluetooth();
        occupy_bluetooth_restore(&mut bluetooth);
        let (task, interrupts) = bluetooth.separate_interrupt_owner();
        let bluetooth = task
            .into_cold(interrupts)
            .expect("an idle Bluetooth task owner can be reunited");
        let Err(failure) = bluetooth.release() else {
            panic!("Bluetooth released a pending restore");
        };
        assert_eq!(
            failure.error(),
            RadioPhyReleaseError::TxDcPwdetRestorePending
        );
    }

    #[test]
    fn pending_txiq_restore_survives_same_route_transitions_and_blocks_release() {
        let mut wifi = RadioHardware::for_validation().into_wifi();
        occupy_wifi_txiq_restore(&mut wifi);
        let (task, interrupts) = wifi.into_running();
        let wifi = task.into_cold(interrupts);
        let Err(failure) = wifi.release() else {
            panic!("Wi-Fi released a pending TX-IQ restore");
        };
        assert_eq!(
            failure.error(),
            RadioPhyReleaseError::TxIqToneControlRestorePending
        );

        let mut ieee802154 = RadioHardware::for_validation().into_ieee802154();
        occupy_ieee802154_txiq_restore(&mut ieee802154);
        let (task, interrupts) = ieee802154.separate_interrupt_owner();
        let ieee802154 = task.into_cold(interrupts);
        let Err(failure) = ieee802154.release() else {
            panic!("IEEE 802.15.4 released a pending TX-IQ restore");
        };
        assert_eq!(
            failure.error(),
            RadioPhyReleaseError::TxIqToneControlRestorePending
        );

        let mut bluetooth = RadioHardware::for_validation().into_bluetooth();
        occupy_bluetooth_txiq_restore(&mut bluetooth);
        let (task, interrupts) = bluetooth.separate_interrupt_owner();
        let bluetooth = task
            .into_cold(interrupts)
            .expect("an idle Bluetooth task owner can be reunited");
        let Err(failure) = bluetooth.release() else {
            panic!("Bluetooth released a pending TX-IQ restore");
        };
        assert_eq!(
            failure.error(),
            RadioPhyReleaseError::TxIqToneControlRestorePending
        );
    }

    #[test]
    fn pending_rx_dco_restore_survives_same_route_transitions_and_blocks_release() {
        let mut wifi = RadioHardware::for_validation().into_wifi();
        occupy_wifi_rx_dco_restore(&mut wifi);
        let (task, interrupts) = wifi.into_running();
        let wifi = task.into_cold(interrupts);
        let Err(failure) = wifi.release() else {
            panic!("Wi-Fi released a pending RX-DCO restore");
        };
        assert_eq!(
            failure.error(),
            RadioPhyReleaseError::RxDcoControlRestorePending
        );

        let mut ieee802154 = RadioHardware::for_validation().into_ieee802154();
        occupy_ieee802154_rx_dco_restore(&mut ieee802154);
        let (task, interrupts) = ieee802154.separate_interrupt_owner();
        let ieee802154 = task.into_cold(interrupts);
        let Err(failure) = ieee802154.release() else {
            panic!("IEEE 802.15.4 released a pending RX-DCO restore");
        };
        assert_eq!(
            failure.error(),
            RadioPhyReleaseError::RxDcoControlRestorePending
        );

        let mut bluetooth = RadioHardware::for_validation().into_bluetooth();
        occupy_bluetooth_rx_dco_restore(&mut bluetooth);
        let (task, interrupts) = bluetooth.separate_interrupt_owner();
        let bluetooth = task
            .into_cold(interrupts)
            .expect("an idle Bluetooth task owner can be reunited");
        let Err(failure) = bluetooth.release() else {
            panic!("Bluetooth released a pending RX-DCO restore");
        };
        assert_eq!(
            failure.error(),
            RadioPhyReleaseError::RxDcoControlRestorePending
        );
    }

    #[test]
    fn pending_bluetooth_tx_power_restore_survives_route_transitions_and_blocks_release() {
        let mut wifi = RadioHardware::for_validation().into_wifi();
        occupy_wifi_bluetooth_tx_power_restore(&mut wifi);
        let (task, interrupts) = wifi.into_running();
        let wifi = task.into_cold(interrupts);
        let Err(failure) = wifi.release() else {
            panic!("Wi-Fi released pending Bluetooth TX-power control state");
        };
        assert_eq!(
            failure.error(),
            RadioPhyReleaseError::BluetoothTxPowerControlRestorePending
        );

        let mut ieee802154 = RadioHardware::for_validation().into_ieee802154();
        occupy_ieee802154_bluetooth_tx_power_restore(&mut ieee802154);
        let (task, interrupts) = ieee802154.separate_interrupt_owner();
        let ieee802154 = task.into_cold(interrupts);
        let Err(failure) = ieee802154.release() else {
            panic!("IEEE 802.15.4 released pending Bluetooth TX-power control state");
        };
        assert_eq!(
            failure.error(),
            RadioPhyReleaseError::BluetoothTxPowerControlRestorePending
        );

        let mut bluetooth = RadioHardware::for_validation().into_bluetooth();
        occupy_bluetooth_tx_power_restore(&mut bluetooth);
        let (task, interrupts) = bluetooth.separate_interrupt_owner();
        let bluetooth = task
            .into_cold(interrupts)
            .expect("an idle Bluetooth task owner can be reunited");
        let Err(failure) = bluetooth.release() else {
            panic!("Bluetooth released pending TX-power control state");
        };
        assert_eq!(
            failure.error(),
            RadioPhyReleaseError::BluetoothTxPowerControlRestorePending
        );
    }

    #[test]
    fn cold_owner_is_consumed_by_interrupt_setup_split() {
        let registers = RadioHardware::for_validation().into_wifi();
        let (_running, _setup) = registers.into_running();
    }

    #[test]
    fn wifi_route_roundtrip_returns_the_complete_root() {
        let wifi = RadioHardware::for_validation().into_wifi();
        let (task, setup) = wifi.into_running();
        let hardware = task
            .into_cold(setup)
            .release()
            .expect("an untouched cold route can be released");

        let bluetooth = hardware.into_bluetooth();
        let _hardware = bluetooth
            .release()
            .expect("an untouched Bluetooth route can be released");
    }

    #[test]
    fn bluetooth_task_and_interrupt_owners_roundtrip_without_mmio() {
        let bluetooth = RadioHardware::for_validation().into_bluetooth();
        let (task, setup) = bluetooth.separate_interrupt_owner();
        let hardware = task
            .into_cold(setup)
            .expect("an idle Bluetooth task owner can be reunited")
            .release()
            .expect("an untouched Bluetooth route can be released");

        let wifi = hardware.into_wifi();
        let _hardware = wifi
            .release()
            .expect("an untouched Wi-Fi route can be released");
    }

    #[test]
    fn ieee802154_route_roundtrip_returns_every_other_protocol_owner() {
        let ieee802154 = RadioHardware::for_validation().into_ieee802154();
        let hardware = ieee802154
            .release()
            .expect("a fresh IEEE 802.15.4 route has no pending PHY restore");

        // The IEEE 802.15.4 epoch retains the complete generated Bluetooth
        // controller partition behind its BTBB role and never consumes either
        // protocol's interrupt owner.
        let bluetooth = hardware.into_bluetooth();
        let hardware = bluetooth
            .release()
            .expect("an untouched Bluetooth route can be released");
        let wifi = hardware.into_wifi();
        let _hardware = wifi
            .release()
            .expect("an untouched Wi-Fi route can be released");
    }

    #[test]
    fn ieee802154_task_and_interrupt_owners_reunite_without_mmio() {
        let ieee802154 = RadioHardware::for_validation().into_ieee802154();
        let (task, setup) = ieee802154.separate_interrupt_owner();
        let hardware = task
            .into_cold(setup)
            .release()
            .expect("an untouched IEEE 802.15.4 route can be released");

        let bluetooth = hardware.into_bluetooth();
        let _hardware = bluetooth
            .release()
            .expect("an untouched Bluetooth route can be released");
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
