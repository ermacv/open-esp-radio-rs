#![no_std]
#![deny(unsafe_code)]

#[cfg(test)]
extern crate std;

use core::future::Future;

use open_esp_radio_esp32s31_pac::{
    Ieee802154TaskRegisters, MacInterruptRegisters as PacMacInterruptRegisters,
    MacInterruptSetup as PacMacInterruptSetup,
    MacPowerInterruptRegisters as PacMacPowerInterruptRegisters, RadioHardware, RadioPhyRegisters,
    RadioPhyReleaseError, WifiColdRegisters, WifiRadioRegisters,
};
pub use phy::analog_i2c;
pub mod bluetooth;
pub use wifi::channel;
pub mod coex;

pub use ieee802154::lifecycle as ieee802154_lifecycle;
pub use phy::agc as phy_agc;
pub use phy::baseband as phy_baseband;
pub use phy::clock as phy_clock;
pub use phy::frequency as phy_frequency;
pub use phy::i2c as phy_i2c;
pub use phy::iq_estimator as phy_iq_estimator;
pub use phy::memory as phy_memory;
pub use phy::pbus;
pub use phy::power_detector as phy_power_detector;
pub use phy::prelude as phy_prelude;
pub use phy::rx_dco as phy_rx_dco;
pub use phy::temperature as phy_temperature;
pub mod power;
pub use phy::power_detector::platform as power_detector_platform;
pub use wifi::arena as radio_arena;
pub mod types;
#[cfg(feature = "validation-probes")]
#[doc(hidden)]
pub mod validation;
pub use bluetooth::{
    BluetoothColdOwner, BluetoothColdOwnerReleaseFailure, BluetoothControllerHal,
    BluetoothControllerHalBorrow, BluetoothControllerHalInitConfig, BluetoothControllerLatchedTime,
    BluetoothControllerPublicAddress, BluetoothControllerRandomAddress,
    BluetoothControllerTimeLatchBeginError, BluetoothControllerTimeLatchStep,
    BluetoothControllerTimeLatchStepError, BluetoothDirectionFindingDisabledBaselineOwner,
    BluetoothInterruptOutputAfterRoutesOwner, BluetoothInterruptOutputPreparedOwner,
    BluetoothInterruptRegistersOwner, BluetoothInterruptSetupOwner,
    BluetoothLowPowerRuntimeControlObservation, BluetoothModemLpTimerCompareDisposition,
    BluetoothModemLpTimerCounterObservation, BluetoothModemLpTimerCounterStartedOwner,
    BluetoothModemLpTimerEpoch, BluetoothModemLpTimerHandlerPendingOwner,
    BluetoothModemLpTimerHandlerRegisterObservation, BluetoothModemLpTimerHandlerRegisterStep,
    BluetoothModemLpTimerInstant, BluetoothModemLpTimerInterruptObservation,
    BluetoothModemLpTimerInterruptReadyOwner, BluetoothModemLpTimerInterruptStep,
    BluetoothModemLpTimerLowPowerHardwareInitializedOwner, BluetoothModemLpTimerOwnerError,
    BluetoothModemLpTimerRegistersPreparedOwner, BluetoothModemLpTimerSoftwarePendingOwner,
    BluetoothNrtInterruptAcknowledged, BluetoothRxMemoryListPublished, BluetoothScanStartPublished,
    BluetoothSchedulerExecutionLockDisposition, BluetoothSchedulerExecutionLockPublished,
    BluetoothSchedulerExecutionLockRequest, BluetoothSchedulerExecutionModifyDisposition,
    BluetoothSchedulerExecutionModifyPublished, BluetoothSchedulerFinishedHardwareListObserved,
    BluetoothSchedulerFinishedListObservation, BluetoothSchedulerFinishedListPop,
    BluetoothSchedulerHardwareListHead, BluetoothSchedulerHardwareListHeadEmptyObserved,
    BluetoothSchedulerHardwareListHeadError, BluetoothSchedulerHardwareListHeadPublished,
    BluetoothSchedulerHardwareListHeadRetirementObservation, BluetoothSchedulerHardwareListIndex,
    BluetoothSchedulerHardwareListsCleared, BluetoothSchedulerHardwareRunCommandPublished,
    BluetoothSchedulerInsertionCommand, BluetoothSchedulerInsertionCommandStartCleared,
    BluetoothSchedulerLockModifyInterruptObservation, BluetoothSchedulerLockModifyObservation,
    BluetoothSchedulerLockModifyPublished, BluetoothSchedulerLockModifyRequest,
    BluetoothSchedulerLockModifyTaskObservation, BluetoothSchedulerReferenceGateObservation,
    BluetoothSchedulerRunEventPublished, BluetoothSchedulerRunInterruptsPrepared,
    BluetoothSchedulerSoftwareListRemovalIdle, BluetoothSchedulerSoftwareListRemovalInterruptStep,
    BluetoothSchedulerSoftwareListRemovalJoin, BluetoothSchedulerSoftwareListRemovalReady,
    BluetoothSchedulerWorkObservation, BluetoothTaskOwner, BluetoothTaskOwnerReuniteError,
    BluetoothTaskOwnerReuniteFailure,
};
pub use ieee802154::operation::{
    Ieee802154OperationEventMaskState, Ieee802154OperationEventObservation,
    Ieee802154OperationPollBudget, Ieee802154OperationRxAbortMaskState, Ieee802154OperationStage,
    Ieee802154PolledOperation, Ieee802154PolledOperationAbortEvidence,
    Ieee802154PolledOperationEvidence, Ieee802154PolledOperationFailure,
    Ieee802154PolledOperationResult,
};
pub use ieee802154::policy::{
    IEEE802154_ACK_TIMEOUT_QUANTUM_MICROSECONDS, IEEE802154_MAX_ACK_TIMEOUT_MICROSECONDS,
    Ieee802154AckTimeout, Ieee802154AckTimeoutError, Ieee802154CcaMode, Ieee802154MacControl,
    Ieee802154MacPolicy, Ieee802154MacPolicyCheckpoint, Ieee802154PanIdentity,
};
pub use ieee802154::role::{
    Ieee802154ClockTransitionFailure, Ieee802154Clocked, Ieee802154FoundationConfigured,
    Ieee802154FoundationTransitionFailure, Ieee802154MacPolicyConfigured,
    Ieee802154MacPolicyRecovery, Ieee802154MacPolicyTransitionFailure,
    Ieee802154OperationCompleted, Ieee802154OperationFailed, Ieee802154Owned,
    Ieee802154PowerTransitionFailure, Ieee802154Powered, Ieee802154Reset,
    Ieee802154ResetTransitionFailure,
};
#[cfg(feature = "validation-probes")]
#[doc(hidden)]
pub use ieee802154::role::{Ieee802154EdEventProbeFinished, Ieee802154EventStatusProbeFinished};
pub use ieee802154::tx_power::{
    Ieee802154ResolvedTxPower, Ieee802154TxPowerLevels, Ieee802154TxPowerLevelsError,
};
#[cfg(feature = "validation-probes")]
pub use ieee802154::validation::ed_event::{
    Ieee802154EdEventProbeConfig, Ieee802154EdEventProbeEvidence, Ieee802154EdEventProbeIsolation,
    Ieee802154EdEventProbeStop,
};
#[cfg(feature = "validation-probes")]
pub use ieee802154::validation::event_status::{
    Ieee802154EventStatusProbeConfig, Ieee802154EventStatusProbeEvidence,
    Ieee802154EventStatusProbeIsolation, Ieee802154EventStatusProbeStop,
};
pub use ieee802154_lifecycle::{
    IEEE802154_MAX_CHANNEL, IEEE802154_MIN_CHANNEL, Ieee802154Channel, Ieee802154ChannelError,
    Ieee802154ClockCheckpoint, Ieee802154ClockReadback, Ieee802154FoundationCheckpoint,
    Ieee802154ReadbackError, Ieee802154ResetCheckpoint, Ieee802154ResetReadback,
};
pub use open_esp_radio_esp32s31_pac::{
    BluetoothControllerSramAddress, BluetoothControllerSramAddressError,
    BluetoothMemoryListSelector, BluetoothPhyEnvironmentAddress,
    BluetoothPhyEnvironmentAddressError, BluetoothPhyRegisterInitInputs,
    Ieee802154ObservedEventState, Ieee802154OperationRxAbortEnableObservation,
    Ieee802154RxAbortReason, Ieee802154RxAbortReasonObservation, Ieee802154TimingPrerequisite,
    Ieee802154TimingReady, Ieee802154ValidationEdDurationState,
    Ieee802154ValidationEventEnableState, MacPowerWakeCause, MacTsfTimerIndex, PhyAdcRate,
    StaBeaconMissLimit, StaBeaconMissTimeoutRaw, StaModemSleepLimit, StaModemWakeConfig,
    StaModemWakePrepareError, StaModemWakeRestore, StaModemWakeRestoreError,
    StaModemWakeRestoreFailure, StaTbttAutoPeriod, StaTbttWakePrepareError, StaTbttWakeRestore,
    StaTbttWakeRestoreError, StaTbttWakeRestoreFailure, StaWakeProtectEarlyTimeRaw,
};
pub use power::{PowerCheckpoint, PowerClockReadback, PowerError};
pub use types::{
    CfrValue, ForcedRxGain, MacInterruptEnableState, MacInterruptEvents, MacInterruptMask,
    MacInterruptObservation, MacInterruptSnapshot, MacOrdinaryTxQueueSnapshot,
    MacPowerInterruptObservation, MacPowerInterruptSnapshot,
};
pub use wifi::baseband as wifi_bb;
pub use wifi::mac as wifi_mac;

pub mod owner;
pub use owner::AsyncDelay;
pub use owner::AsyncEvent;
pub use owner::BluetoothSharedPhyBorrow;
pub use owner::ConnectedStaInterruptPrepared;
pub use owner::Ieee802154SharedPhyBorrow;
pub use owner::MacInterruptRegisters;
pub use owner::MacInterruptSetup;
pub use owner::MacPowerInterruptRegisters;
pub use owner::PhyHal;
pub use owner::PhyInitializationAccess;
pub use owner::PowerUpFailure;
pub use owner::Radio;
pub use owner::RadioReleaseFailure;
pub use owner::RadioRuntimeOwner;
pub use owner::SharedPhyAccess;
pub use owner::SharedPhyContext;
pub use owner::SharedPhyHal;
pub use owner::WifiBasebandEnableObservation;
pub(crate) use owner::phy_pac;
pub(crate) use owner::phy_pac_mut;
pub(crate) use owner::sealed;
pub use owner::state;
#[cfg(feature = "validation-probes")]
pub use owner::validation_mac_interrupt_registers;
#[cfg(feature = "validation-probes")]
pub use owner::validation_mac_interrupt_setup;
#[cfg(feature = "validation-probes")]
pub use owner::validation_mac_power_interrupt_registers;
pub mod ieee802154;
pub mod phy;
pub mod wifi;
