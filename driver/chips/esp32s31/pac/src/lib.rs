#![no_std]
#![deny(unsafe_code)]

#[cfg(test)]
extern crate std;

pub(crate) use bluetooth::controller::init as bluetooth_controller_hal_init;
pub(crate) use bluetooth::controller::time as bluetooth_controller_time;
pub(crate) use bluetooth::direction_finding as bluetooth_direction_finding;
pub(crate) use bluetooth::interrupt as bluetooth_interrupt;
pub(crate) use bluetooth::memory_lists as bluetooth_memory_lists;
pub(crate) use bluetooth::modem_timer as bluetooth_modem_lp_timer;
pub(crate) use bluetooth::phy as bluetooth_phy_init;
pub(crate) use bluetooth::scan as bluetooth_scan;
pub(crate) use bluetooth::scheduler as bluetooth_scheduler;
pub(crate) use bluetooth::scheduler::insertion as bluetooth_scheduler_insertion;
pub(crate) use bluetooth::scheduler::lock_modify as bluetooth_scheduler_lock_modify;
pub(crate) use bluetooth::scheduler::runtime as bluetooth_scheduler_runtime;
pub(crate) use modem::coex;
pub(crate) use phy::agc::runtime as agc_runtime;
pub(crate) use phy::baseband;
pub(crate) use phy::cfr;
pub use phy::clock;
pub(crate) use phy::frequency;
// The generated capability catalog is intentionally broader than this crate's
// restricted ownership facade. Some reviewed leaves stay unreachable until an
// owner transition exposes them; do not reopen full-block access to make them
// appear used.
#[allow(
    dead_code,
    reason = "generated capability catalog is wider than the restricted ownership facade"
)]
mod generated;

pub(crate) use ieee802154::timing as ieee802154_timing;
pub(crate) use modem::shared_clock as modem_shared_clock;
pub(crate) use modem::syscon as modem_syscon;
pub use phy::pbus;
pub(crate) use wifi::mac::block_ack as mac_block_ack;
pub(crate) use wifi::mac::coex::init as mac_coex_init;
pub(crate) use wifi::mac::crypto as mac_crypto;
pub(crate) use wifi::mac::he::beamforming as mac_he_beamforming;
pub(crate) use wifi::mac::he::init_suffix as mac_he_init_suffix;
pub(crate) use wifi::mac::he::ofdma as mac_he_ofdma;
pub(crate) use wifi::mac::he::peer as mac_he_peer;
pub(crate) use wifi::mac::he::tb as mac_he_tb;
pub(crate) use wifi::mac::interrupt as mac_interrupt;
pub(crate) use wifi::mac::modem_wakeup as mac_modem_wakeup;
pub(crate) use wifi::mac::rx::dma as mac_rx_dma;
pub(crate) use wifi::mac::rx::policy as mac_rx_policy;
pub(crate) use wifi::mac::rx::statistics as mac_rx_statistics;
pub(crate) use wifi::mac::tsf as mac_tsf;
pub(crate) use wifi::mac::tx as mac_tx;
pub(crate) use wifi::mac::tx::power_init as mac_tx_power_init;
pub(crate) use wifi::mac::tx::queue as mac_tx_queue;
pub(crate) use wifi::mac::tx::statistics as mac_tx_statistics;
pub mod phy;
pub(crate) use modem::platform as platform_clock_power;
pub use phy::i2c as phy_i2c;
pub(crate) use phy::table_memory;
#[cfg(feature = "validation-probes")]
pub mod validation;
pub use agc_runtime::ForcedRxGain;
pub use baseband::{
    BluetoothTxPowerControlPrepareError, BluetoothTxPowerControlRestoreError,
    RxDcoControlPrepareError, RxDcoControlRestoreError, TxDcPwdetLifecycleError,
    TxDcPwdetPrepareError, TxDcPwdetRestoreError, TxIqToneControlPrepareError,
    TxIqToneControlRestoreError,
};
pub use bluetooth_controller_hal_init::{
    BluetoothControllerHalInitConfig, BluetoothControllerTimeScale, BluetoothHalInitPeriod,
    BluetoothHalInitScale, BluetoothRawTickDeltaProjection,
};
pub use bluetooth_controller_time::{
    BluetoothControllerLatchedTime, BluetoothControllerTimeLatchBeginError,
    BluetoothControllerTimeLatchRequest, BluetoothControllerTimeLatchStep,
    BluetoothControllerTimeLatchStepError,
};
pub use bluetooth_direction_finding::BluetoothDirectionFindingDisabledBaselinePrepared;
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
    BluetoothLowPowerRuntimeControlObservation, BluetoothModemLpTimerCompareDisposition,
    BluetoothModemLpTimerCounterObservation, BluetoothModemLpTimerCounterStarted,
    BluetoothModemLpTimerEpoch, BluetoothModemLpTimerHandlerPending,
    BluetoothModemLpTimerHandlerRegisterObservation, BluetoothModemLpTimerHandlerRegisterStep,
    BluetoothModemLpTimerInstant, BluetoothModemLpTimerInterruptObservation,
    BluetoothModemLpTimerInterruptReady, BluetoothModemLpTimerInterruptStep,
    BluetoothModemLpTimerLowPowerHardwareInitialized, BluetoothModemLpTimerOwnerError,
    BluetoothModemLpTimerRegisters, BluetoothModemLpTimerRegistersPrepared,
    BluetoothModemLpTimerSoftwarePending,
};
pub use bluetooth_phy_init::{
    BluetoothPhyEnvironmentAddress, BluetoothPhyEnvironmentAddressError,
    BluetoothPhyRegisterInitInputs,
};
pub use bluetooth_scan::BluetoothScanStartPublished;
pub use bluetooth_scheduler::{
    BluetoothSchedulerHardwareListHead, BluetoothSchedulerHardwareListHeadEmptyObserved,
    BluetoothSchedulerHardwareListHeadError, BluetoothSchedulerHardwareListHeadPublished,
    BluetoothSchedulerHardwareListHeadRetirementObservation,
    BluetoothSchedulerHardwareListsCleared, BluetoothSchedulerHardwareRunCommandPublished,
    BluetoothSchedulerInsertionCommand, BluetoothSchedulerInsertionCommandStartCleared,
    BluetoothSchedulerRunEventPublished,
};
pub use bluetooth_scheduler_insertion::{
    BluetoothSchedulerExecutionLockDisposition, BluetoothSchedulerExecutionLockPublished,
    BluetoothSchedulerExecutionLockRequest, BluetoothSchedulerExecutionModifyDisposition,
    BluetoothSchedulerExecutionModifyPublished,
};
pub use bluetooth_scheduler_lock_modify::{
    BluetoothSchedulerLockModifyInterruptObservation, BluetoothSchedulerLockModifyObservation,
    BluetoothSchedulerLockModifyPublished, BluetoothSchedulerLockModifyRequest,
    BluetoothSchedulerLockModifyTaskObservation,
};
pub use bluetooth_scheduler_runtime::{
    BluetoothSchedulerFinishedHardwareListObserved, BluetoothSchedulerFinishedListObservation,
    BluetoothSchedulerFinishedListPop, BluetoothSchedulerHardwareListIndex,
    BluetoothSchedulerReferenceCleared, BluetoothSchedulerReferenceGateObservation,
    BluetoothSchedulerSoftwareListRemovalIdle, BluetoothSchedulerSoftwareListRemovalInterruptStep,
    BluetoothSchedulerSoftwareListRemovalJoin, BluetoothSchedulerSoftwareListRemovalReady,
    BluetoothSchedulerWorkObservation,
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
    CoexTimerClientValue, CoexTimerPtiValue, CoexTimerTickInput, MacAssociationId,
    MacExtraSoftApRxBlockAckEntryIndex, MacHeBssColor, MacHeDefaultPacketExtensionDuration,
    MacHePacketPaddingDuration, MacInterface, MacItwtClearIndex, MacKeyEntryIndex,
    MacMinimumMpduStartSpacing, MacPti, MacRxBlockAckEntryIndex, MacRxBlockAckStartingSequence,
    MacRxBlockAckTid, MacRxBlockAckWindow, MacTxPtiCount, MacTxQueueIndex,
    ModemLowPowerClockDivider, PhyForcedPowerIndex, PhyFtmEnableVendorArgument,
};
pub use ieee802154::mac::{
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
pub use ieee802154::mac::{Ieee802154PolledRegisterLease, Ieee802154RegisterLease};
pub use ieee802154_timing::{Ieee802154TimingPrerequisite, Ieee802154TimingReady};
pub use mac_block_ack::{
    ExtraSoftApRxBlockAckEntrySnapshot, InternalTxBlockAckSnapshot, RxBlockAckEntrySnapshot,
    TxBlockAckDiagnosticSnapshot, TxBlockAckPayload,
};
pub use mac_coex_init::MacCoexPrioritySnapshot;
pub use mac_crypto::MacCcmpKeyIdentity;
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
    MacOrdinaryTxQueueSnapshot, MacTxCompletionObservation, MacTxDetachOutcome, MacTxDetachReason,
    MacTxPtiProgram, MacTxQueueDetached,
};
pub use mac_tx_power_init::{
    MAC_TX_POWER_RATE_COUNT, MacPartialRuPowerSelector, MacTxPowerIndex, MacTxPowerPair,
    MacTxPowerTable,
};
pub use mac_tx_statistics::MacTxStatisticsSnapshot;
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
pub mod ownership;
pub(crate) use ownership::BLUETOOTH_MAIN_XTAL_LOW_POWER_DIVIDER;
pub use ownership::BluetoothColdRegisters;
pub use ownership::BluetoothInterruptRegisters;
pub use ownership::BluetoothInterruptSetup;
pub use ownership::BluetoothTaskRegisters;
pub use ownership::BluetoothTaskReuniteError;
pub use ownership::BluetoothTaskReuniteFailure;
pub use ownership::Ieee802154ColdRegisters;
pub use ownership::Ieee802154InterruptRegisters;
pub use ownership::Ieee802154InterruptSetup;
pub use ownership::Ieee802154TaskRegisters;
pub use ownership::MacInterruptEvents;
pub use ownership::MacInterruptObservation;
pub use ownership::MacInterruptSnapshot;
pub use ownership::MacPowerInterruptObservation;
pub use ownership::MacPowerInterruptSnapshot;
pub use ownership::RadioHardware;
pub use ownership::RadioPhyRegisters;
pub use ownership::RadioPhyReleaseError;
pub use ownership::RadioPhyReleaseFailure;
pub use ownership::WifiColdRegisters;
pub use ownership::WifiRadioRegisters;
pub(crate) use ownership::device_fence;
pub(crate) mod bluetooth;
pub(crate) mod ieee802154;
pub(crate) mod modem;
pub(crate) mod wifi;
