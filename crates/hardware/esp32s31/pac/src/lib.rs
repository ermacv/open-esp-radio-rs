#![no_std]
#![deny(unsafe_code)]

#[cfg(test)]
extern crate std;

pub(crate) use modem::coex;

pub(crate) use phy::{baseband, cfr};

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

pub(crate) use phy::table_memory;
#[cfg(feature = "validation-probes")]
pub mod validation;

/// Reviewed writable MAC interrupt mask.
///
/// The generated domain deliberately has no public integer constructor:
///
/// ```compile_fail
/// use oer_esp32s31_pac::MacInterruptMask;
///
/// let invented = MacInterruptMask(0xdead_beef);
/// ```
pub use generated::MacInterruptMask;

pub use baseband::{
    BluetoothTxPowerControlPrepareError, BluetoothTxPowerControlRestoreError,
    RxDcoControlPrepareError, RxDcoControlRestoreError, TxDcPwdetLifecycleError,
    TxDcPwdetPrepareError, TxDcPwdetRestoreError, TxIqToneControlPrepareError,
    TxIqToneControlRestoreError,
};

pub use bluetooth::{
    controller::{
        init::{
            BluetoothControllerHalInitConfig, BluetoothControllerTimeScale, BluetoothHalInitPeriod,
            BluetoothHalInitScale, BluetoothMicrosecondDeltaProjection,
            BluetoothRawTickDeltaProjection,
        },
        time::{
            BluetoothControllerLatchedTime, BluetoothControllerTimeLatchBeginError,
            BluetoothControllerTimeLatchRequest, BluetoothControllerTimeLatchStep,
            BluetoothControllerTimeLatchStepError,
        },
    },
    direction_finding::BluetoothDirectionFindingDisabledBaselinePrepared,
    interrupt::{
        BluetoothInterruptOutputPrepared, BluetoothNrtInterruptAcknowledged,
        BluetoothPrimaryFaultSources, BluetoothPrimaryInterruptEpoch,
        BluetoothSchedulerRunInterruptsPrepared,
    },
    memory_lists::{
        BluetoothControllerSramAddress, BluetoothControllerSramAddressError,
        BluetoothMemoryListPointerImage, BluetoothMemoryListSelector, BluetoothMemoryListSlot,
    },
    modem_timer::{
        BluetoothLowPowerRuntimeControlObservation, BluetoothModemLpTimerCompareDisposition,
        BluetoothModemLpTimerCounterObservation, BluetoothModemLpTimerCounterStarted,
        BluetoothModemLpTimerEpoch, BluetoothModemLpTimerHandlerPending,
        BluetoothModemLpTimerHandlerRegisterObservation, BluetoothModemLpTimerInstant,
        BluetoothModemLpTimerInterruptObservation, BluetoothModemLpTimerInterruptReady,
        BluetoothModemLpTimerLowPowerHardwareInitialized, BluetoothModemLpTimerOwnerError,
        BluetoothModemLpTimerRegisters, BluetoothModemLpTimerRegistersPrepared,
        BluetoothModemLpTimerSoftwarePending, ModemLpTimerHandlerRegisterStep,
        ModemLpTimerInterruptStep,
    },
    phy::{
        BluetoothPhyEnvironmentAddress, BluetoothPhyEnvironmentAddressError,
        BluetoothPhyRegisterInitInputs,
    },
    scan::BluetoothScanStartPublished,
    scheduler::{
        BluetoothSchedulerHardwareListHead, BluetoothSchedulerHardwareListHeadEmptyObserved,
        BluetoothSchedulerHardwareListHeadError, BluetoothSchedulerHardwareListHeadPublished,
        BluetoothSchedulerHardwareListHeadRetirementObservation,
        BluetoothSchedulerHardwareListsCleared, BluetoothSchedulerHardwareRunCommandPublished,
        BluetoothSchedulerInsertionCommand, BluetoothSchedulerInsertionCommandStartCleared,
        BluetoothSchedulerRunEventPublished,
        insertion::{
            BluetoothSchedulerExecutionLockDisposition, BluetoothSchedulerExecutionLockPublished,
            BluetoothSchedulerExecutionLockRequest, BluetoothSchedulerExecutionModifyDisposition,
            BluetoothSchedulerExecutionModifyPublished,
        },
        lock_modify::{
            BluetoothSchedulerLockModifyInterruptObservation,
            BluetoothSchedulerLockModifyObservation, BluetoothSchedulerLockModifyPublished,
            BluetoothSchedulerLockModifyRequest, BluetoothSchedulerLockModifyTaskObservation,
        },
        runtime::{
            BluetoothSchedulerFinishedHardwareListObserved,
            BluetoothSchedulerFinishedListObservation, BluetoothSchedulerFinishedListPop,
            BluetoothSchedulerHardwareListIndex, BluetoothSchedulerReferenceCleared,
            BluetoothSchedulerReferenceGateObservation, BluetoothSchedulerSoftwareListRemovalIdle,
            BluetoothSchedulerSoftwareListRemovalInterruptStep,
            BluetoothSchedulerSoftwareListRemovalJoin, BluetoothSchedulerSoftwareListRemovalReady,
            BluetoothSchedulerWorkObservation,
        },
        stop::{
            BluetoothSchedulerStop, BluetoothSchedulerStopStep, BluetoothSchedulerStopped,
            BluetoothSchedulerStoppedHeadRetirement, BluetoothSchedulerStoppedItem,
        },
    },
};

pub use cfr::CfrValue;

pub use coex::{COEX_TIMER_COUNT, CoexTimerRegister};

pub use frequency::PhyFrequencyI2cNumberAddresses;

#[doc(hidden)]
pub use ieee802154::mac::{Ieee802154PolledRegisterLease, Ieee802154RegisterLease};

pub use generated::{
    CoexTimerClientValue, CoexTimerPtiValue, CoexTimerTickInput, MacAssociationId,
    MacExtraSoftApRxBlockAckEntryIndex, MacHeBssColor, MacHeDefaultPacketExtensionDuration,
    MacHePacketPaddingDuration, MacInterface, MacItwtClearIndex, MacKeyEntryIndex,
    MacMinimumMpduStartSpacing, MacPti, MacRxBlockAckEntryIndex, MacRxBlockAckStartingSequence,
    MacRxBlockAckTid, MacRxBlockAckWindow, MacTxPtiCount, MacTxQueueIndex,
    ModemLowPowerClockDivider, PhyForcedPowerIndex, PhyFtmEnableVendorArgument,
};

pub use ieee802154::{
    mac::{
        Ieee802154AckTimeoutUnits, Ieee802154CcaMode, Ieee802154EdCcaSnapshot, Ieee802154EdCommand,
        Ieee802154EdDurationUnits, Ieee802154EdSampleRate, Ieee802154Event,
        Ieee802154EventEnableState, Ieee802154EventMask, Ieee802154EventObservation,
        Ieee802154EventObservationError, Ieee802154FoundationSnapshot, Ieee802154FrequencyCode,
        Ieee802154InterruptSnapshot, Ieee802154MacCommand, Ieee802154MacConfigurationReadback,
        Ieee802154MacControl, Ieee802154MacPolicySnapshot, Ieee802154MultipanEnableState,
        Ieee802154MultipanIndex, Ieee802154ObservedEventState,
        Ieee802154OperationEventEnableObservation, Ieee802154OperationRxAbortEnableObservation,
        Ieee802154PanIdentity, Ieee802154Pti, Ieee802154RouteState, Ieee802154RxAbortEnableState,
        Ieee802154RxAbortReason, Ieee802154RxAbortReasonObservation, Ieee802154RxStateCode,
        Ieee802154SecurityPayloadOffset, Ieee802154StateSnapshot, Ieee802154Timer0ThresholdWord,
        Ieee802154Timer0ValueWord, Ieee802154Timer1ThresholdWord, Ieee802154Timer1ValueWord,
        Ieee802154TimerLease, Ieee802154TransmitSecurityControl, Ieee802154TxAbortReason,
        Ieee802154TxAbortReasonObservation, Ieee802154TxPowerCode, Ieee802154TxStateCode,
        Ieee802154ValidationEdDurationState, Ieee802154ValidationEventEnableState,
    },
    timing::{Ieee802154TimingPrerequisite, Ieee802154TimingReady},
};

pub use modem::{
    platform::PlatformClockPowerObservation,
    shared_clock::{
        BluetoothLowPowerClockObservation, CoexistenceLowPowerClockObservation,
        CoexistenceLowPowerClockSource, ModemLowPowerClockSource, SharedModemClockObservation,
    },
};

use modem::{
    shared_clock::{BluetoothLowPowerTimerLease, SharedModemClock, SharedModemClockLease},
    syscon::BluetoothModemSysconClockState,
};

pub use modem::syscon::{
    ModemSysconBluetoothObservation, ModemSysconIeee802154ClockObservation,
    ModemSysconIeee802154ResetObservation, ModemSysconPowerObservation, WifiBasebandAgcUpdate,
};

use oer_esp32s31_pac_raw as svd;

pub use phy::{
    agc::runtime::ForcedRxGain,
    i2c::{
        BluetoothTxPowerControlAction, BluetoothTxPowerControlCompletion,
        BluetoothTxPowerControlError, BluetoothTxPowerControlObservation,
        BluetoothTxPowerControlOperation, BluetoothTxPowerControlTransaction, PhyAdcRate,
        PhyFilterDcapInputs, PhyI2cAccessError, PhyI2cAddress, PhyI2cBlock,
        PhyI2cCommandMemoryInputs, PhyI2cConfigurationAction, PhyI2cConfigurationError,
        PhyI2cConfigurationObservation, PhyI2cConfigurationOperation,
        PhyI2cConfigurationTransaction, PhyI2cField, PhyI2cHost,
        PhyI2cInitializationStageOneInputs, analog_registers,
    },
};

pub use table_memory::{PbusMemoryGroupBoundary, PhyGainMemoryEntry, PhyMemoryError};

pub use wifi::mac::{
    block_ack::{
        ExtraSoftApRxBlockAckEntrySnapshot, InternalTxBlockAckSnapshot, RxBlockAckEntrySnapshot,
        TxBlockAckDiagnosticSnapshot, TxBlockAckPayload,
    },
    coex::init::MacCoexPrioritySnapshot,
    crypto::{MacCcmpKeyIdentity, MacKeyInstallOutcome},
    he::{
        beamforming::{
            MacHeBeamformingReportProfile, MacHeBeamformingReportProfileError,
            MacHeErSuAckRateProfile,
        },
        init_suffix::MacHeTxMpduLengthLink,
        ofdma::{
            MacBeamformingAverageSnr, MacHeBeamformingConfigurationSnapshot,
            MacHeBeamformingDiagnostics, MacHeBufferStatusSnapshot, MacHeCustomReceiveType,
            MacHeEdcaQueueConfiguration, MacHeMuEdcaTimerSnapshot, MacHeQueueSchedulingSnapshot,
            MacHeReceiveConfigurationSnapshot, MacHeRxPowerSaveSnapshot, MacHeTbLinkReservation,
            MacHeTbProgramError, MacHeTbTidLimit, MacHeTid, MacHeTriggerQueueConfiguration,
            MacHeTriggerRxDiagnostics, MacHeTriggerTxQueueSnapshot,
        },
        peer::{MacHe20PeerConfig, MacHe20PeerError},
        tb::{MacHeTbStatistics, MacHeTbTxDiagnostics},
    },
    interrupt::{
        ConnectedStaWithoutPowerSavePrepared, MacInterruptEnableState, MacInterruptRegisters,
        MacInterruptSetup, MacPowerInterruptRegisters, MacPowerWakeCause, MacTsfTimerIndex,
    },
    modem_wakeup::{
        StaBeaconMissLimit, StaBeaconMissTimeoutRaw, StaModemSleepLimit, StaModemWakeConfig,
        StaModemWakePrepareError, StaModemWakeRestore, StaModemWakeRestoreError,
        StaModemWakeRestoreFailure, StaTbttAutoPeriod, StaWakeProtectEarlyTimeRaw,
    },
    rx::{
        dma::{MacRxDmaSnapshot, MacRxNextDescriptorObservation},
        policy::{
            MacApReceivePolicySnapshot, MacRoleReceivePolicy, MacStaApReceivePlan,
            MacStaPolicyMode, MacStaReceivePolicySnapshot,
        },
        statistics::{
            MacHeColorCollisionSnapshot, MacRxDecodeErrorStatistics,
            MacRxDecodeErrorStatisticsDelta, MacRxHangStatistics, MacRxHangStatisticsDelta,
            MacRxPrimaryStatistics, MacRxPrimaryStatisticsDelta, MacRxStatisticsSnapshot,
        },
    },
    tsf::{
        StaTbttWakePrepareError, StaTbttWakeRestore, StaTbttWakeRestoreError,
        StaTbttWakeRestoreFailure,
    },
    tx::{
        MacHeFecCoding, MacHeGuardIntervalAndLtf, MacHeMcs, MacHeRate, MacHeTxFormat,
        MacHeTxParameters, MacHeTxProgram, MacHtAmpduCompletionObservation, MacHtChannelWidth,
        MacHtGuardInterval, MacHtMcs, MacHtProtectionSpacing, MacHtRate, MacHtTxFormat,
        MacHtTxParameters, MacHtTxProgram, MacLegacyRate, MacLegacyTxParameters,
        MacLegacyTxProgram, MacOrdinaryTxQueueSnapshot, MacTxCompletionObservation,
        MacTxDetachOutcome, MacTxDetachReason, MacTxPtiProgram, MacTxQueueDetached,
        power_init::{
            MAC_TX_POWER_RATE_COUNT, MacPartialRuPowerSelector, MacTxPowerIndex, MacTxPowerPair,
            MacTxPowerTable,
        },
        statistics::MacTxStatisticsSnapshot,
    },
};
pub mod ownership;

pub(crate) use ownership::BLUETOOTH_MAIN_XTAL_LOW_POWER_DIVIDER;

pub use ownership::{
    BluetoothColdRegisters, BluetoothInterruptRegisters, BluetoothInterruptSetup,
    BluetoothTaskRegisters, BluetoothTaskReuniteError, BluetoothTaskReuniteFailure,
    Ieee802154ColdRegisters, Ieee802154InterruptRegisters, Ieee802154InterruptSetup,
    Ieee802154TaskRegisters, MacInterruptEvents, MacInterruptObservation, MacInterruptSnapshot,
    MacPowerInterruptObservation, MacPowerInterruptSnapshot, RadioHardware, RadioPhyRegisters,
    RadioPhyReleaseError, RadioPhyReleaseFailure, WifiColdRegisters, WifiRadioRegisters,
};

pub(crate) use ownership::device_fence;
pub(crate) mod bluetooth;
pub(crate) mod ieee802154;
pub(crate) mod modem;
pub(crate) mod wifi;
