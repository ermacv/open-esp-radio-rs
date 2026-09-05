#![no_std]
#![deny(unsafe_code)]

#[cfg(test)]
extern crate std;

pub(crate) use modem::coex;
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

pub use phy::pbus;
pub mod phy;
pub use phy::i2c as phy_i2c;
pub(crate) use phy::table_memory;
#[cfg(feature = "validation-probes")]
pub mod validation;
pub use baseband::{
    BluetoothTxPowerControlPrepareError, BluetoothTxPowerControlRestoreError,
    RxDcoControlPrepareError, RxDcoControlRestoreError, TxDcPwdetLifecycleError,
    TxDcPwdetPrepareError, TxDcPwdetRestoreError, TxIqToneControlPrepareError,
    TxIqToneControlRestoreError,
};
pub use bluetooth::controller::init::{
    BluetoothControllerHalInitConfig, BluetoothControllerTimeScale, BluetoothHalInitPeriod,
    BluetoothHalInitScale, BluetoothRawTickDeltaProjection,
};
pub use bluetooth::controller::time::{
    BluetoothControllerLatchedTime, BluetoothControllerTimeLatchBeginError,
    BluetoothControllerTimeLatchRequest, BluetoothControllerTimeLatchStep,
    BluetoothControllerTimeLatchStepError,
};
pub use bluetooth::direction_finding::BluetoothDirectionFindingDisabledBaselinePrepared;
pub use bluetooth::interrupt::{
    BluetoothInterruptOutputPrepared, BluetoothNrtInterruptAcknowledged,
    BluetoothPrimaryFaultSources, BluetoothPrimaryInterruptEpoch,
    BluetoothSchedulerRunInterruptsPrepared,
};
pub use bluetooth::memory_lists::{
    BluetoothControllerSramAddress, BluetoothControllerSramAddressError,
    BluetoothMemoryListPointerImage, BluetoothMemoryListSelector, BluetoothMemoryListSlot,
};
pub use bluetooth::modem_timer::{
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
pub use bluetooth::phy::{
    BluetoothPhyEnvironmentAddress, BluetoothPhyEnvironmentAddressError,
    BluetoothPhyRegisterInitInputs,
};
pub use bluetooth::scan::BluetoothScanStartPublished;
pub use bluetooth::scheduler::insertion::{
    BluetoothSchedulerExecutionLockDisposition, BluetoothSchedulerExecutionLockPublished,
    BluetoothSchedulerExecutionLockRequest, BluetoothSchedulerExecutionModifyDisposition,
    BluetoothSchedulerExecutionModifyPublished,
};
pub use bluetooth::scheduler::lock_modify::{
    BluetoothSchedulerLockModifyInterruptObservation, BluetoothSchedulerLockModifyObservation,
    BluetoothSchedulerLockModifyPublished, BluetoothSchedulerLockModifyRequest,
    BluetoothSchedulerLockModifyTaskObservation,
};
pub use bluetooth::scheduler::runtime::{
    BluetoothSchedulerFinishedHardwareListObserved, BluetoothSchedulerFinishedListObservation,
    BluetoothSchedulerFinishedListPop, BluetoothSchedulerHardwareListIndex,
    BluetoothSchedulerReferenceCleared, BluetoothSchedulerReferenceGateObservation,
    BluetoothSchedulerSoftwareListRemovalIdle, BluetoothSchedulerSoftwareListRemovalInterruptStep,
    BluetoothSchedulerSoftwareListRemovalJoin, BluetoothSchedulerSoftwareListRemovalReady,
    BluetoothSchedulerWorkObservation,
};
pub use bluetooth::scheduler::{
    BluetoothSchedulerHardwareListHead, BluetoothSchedulerHardwareListHeadEmptyObserved,
    BluetoothSchedulerHardwareListHeadError, BluetoothSchedulerHardwareListHeadPublished,
    BluetoothSchedulerHardwareListHeadRetirementObservation,
    BluetoothSchedulerHardwareListsCleared, BluetoothSchedulerHardwareRunCommandPublished,
    BluetoothSchedulerInsertionCommand, BluetoothSchedulerInsertionCommandStartCleared,
    BluetoothSchedulerRunEventPublished,
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
pub use ieee802154::timing::{Ieee802154TimingPrerequisite, Ieee802154TimingReady};
pub use modem::platform::PlatformClockPowerObservation;
pub use modem::shared_clock::{
    BluetoothLowPowerClockObservation, CoexistenceLowPowerClockObservation,
    CoexistenceLowPowerClockSource, ModemLowPowerClockSource, SharedModemClockObservation,
};
use modem::shared_clock::{BluetoothLowPowerTimerLease, SharedModemClock, SharedModemClockLease};
use modem::syscon::BluetoothModemSysconClockState;
pub use modem::syscon::{
    ModemSysconBluetoothObservation, ModemSysconIeee802154ClockObservation,
    ModemSysconIeee802154ResetObservation, ModemSysconPowerObservation, WifiBasebandAgcUpdate,
};
use open_esp_radio_esp32s31_pac_raw as svd;
pub use phy::agc::runtime::ForcedRxGain;
pub use phy_i2c::{
    BluetoothTxPowerControlAction, BluetoothTxPowerControlCompletion, BluetoothTxPowerControlError,
    BluetoothTxPowerControlObservation, BluetoothTxPowerControlOperation,
    BluetoothTxPowerControlTransaction, PhyAdcRate, PhyFilterDcapInputs, PhyI2cAccessError,
    PhyI2cAddress, PhyI2cBlock, PhyI2cCommandMemoryInputs, PhyI2cConfigurationAction,
    PhyI2cConfigurationError, PhyI2cConfigurationObservation, PhyI2cConfigurationOperation,
    PhyI2cConfigurationTransaction, PhyI2cField, PhyI2cHost, PhyI2cInitializationStageOneInputs,
    analog_registers,
};
pub use table_memory::{PbusMemoryGroupBoundary, PhyGainMemoryEntry, PhyMemoryError};
pub use wifi::mac::block_ack::{
    ExtraSoftApRxBlockAckEntrySnapshot, InternalTxBlockAckSnapshot, RxBlockAckEntrySnapshot,
    TxBlockAckDiagnosticSnapshot, TxBlockAckPayload,
};
pub use wifi::mac::coex::init::MacCoexPrioritySnapshot;
pub use wifi::mac::crypto::MacCcmpKeyIdentity;
pub use wifi::mac::crypto::MacKeyInstallOutcome;
pub use wifi::mac::he::beamforming::{
    MacHeBeamformingReportProfile, MacHeBeamformingReportProfileError, MacHeErSuAckRateProfile,
};
pub use wifi::mac::he::init_suffix::MacHeTxMpduLengthLink;
pub use wifi::mac::he::ofdma::{
    MacBeamformingAverageSnr, MacHeBeamformingConfigurationSnapshot, MacHeBeamformingDiagnostics,
    MacHeBufferStatusSnapshot, MacHeCustomReceiveType, MacHeEdcaQueueConfiguration,
    MacHeMuEdcaTimerSnapshot, MacHeQueueSchedulingSnapshot, MacHeReceiveConfigurationSnapshot,
    MacHeRxPowerSaveSnapshot, MacHeTbLinkReservation, MacHeTbProgramError, MacHeTbTidLimit,
    MacHeTid, MacHeTriggerQueueConfiguration, MacHeTriggerRxDiagnostics,
    MacHeTriggerTxQueueSnapshot,
};
pub use wifi::mac::he::peer::{MacHe20PeerConfig, MacHe20PeerError};
pub use wifi::mac::he::tb::{MacHeTbStatistics, MacHeTbTxDiagnostics};
pub use wifi::mac::interrupt::{
    ConnectedStaWithoutPowerSavePrepared, MacInterruptEnableState, MacInterruptRegisters,
    MacInterruptSetup, MacPowerInterruptRegisters, MacPowerWakeCause, MacTsfTimerIndex,
};
pub use wifi::mac::modem_wakeup::{
    StaBeaconMissLimit, StaBeaconMissTimeoutRaw, StaModemSleepLimit, StaModemWakeConfig,
    StaModemWakePrepareError, StaModemWakeRestore, StaModemWakeRestoreError,
    StaModemWakeRestoreFailure, StaTbttAutoPeriod, StaWakeProtectEarlyTimeRaw,
};
pub use wifi::mac::rx::dma::{MacRxDmaSnapshot, MacRxNextDescriptorObservation};
pub use wifi::mac::rx::policy::{
    MacApReceivePolicySnapshot, MacRoleReceivePolicy, MacStaApReceivePlan, MacStaPolicyMode,
    MacStaReceivePolicySnapshot,
};
pub use wifi::mac::rx::statistics::{
    MacHeColorCollisionSnapshot, MacRxDecodeErrorStatistics, MacRxDecodeErrorStatisticsDelta,
    MacRxHangStatistics, MacRxHangStatisticsDelta, MacRxPrimaryStatistics,
    MacRxPrimaryStatisticsDelta, MacRxStatisticsSnapshot,
};
pub use wifi::mac::tsf::{
    StaTbttWakePrepareError, StaTbttWakeRestore, StaTbttWakeRestoreError, StaTbttWakeRestoreFailure,
};
pub use wifi::mac::tx::power_init::{
    MAC_TX_POWER_RATE_COUNT, MacPartialRuPowerSelector, MacTxPowerIndex, MacTxPowerPair,
    MacTxPowerTable,
};
pub use wifi::mac::tx::statistics::MacTxStatisticsSnapshot;
pub use wifi::mac::tx::{
    MacHeFecCoding, MacHeGuardIntervalAndLtf, MacHeMcs, MacHeRate, MacHeTxFormat,
    MacHeTxParameters, MacHeTxProgram, MacHtAmpduCompletionObservation, MacHtChannelWidth,
    MacHtGuardInterval, MacHtMcs, MacHtProtectionSpacing, MacHtRate, MacHtTxFormat,
    MacHtTxParameters, MacHtTxProgram, MacLegacyRate, MacLegacyTxParameters, MacLegacyTxProgram,
    MacOrdinaryTxQueueSnapshot, MacTxCompletionObservation, MacTxDetachOutcome, MacTxDetachReason,
    MacTxPtiProgram, MacTxQueueDetached,
};
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
