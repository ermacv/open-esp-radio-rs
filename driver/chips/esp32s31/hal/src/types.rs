//! Value-only hardware contracts exposed above the PAC boundary.
//!
//! The defining register domains remain in the restricted PAC, while this
//! module is the only public path used by HAL consumers.  No peripheral owner,
//! register block, raw accessor, or generic MMIO capability is re-exported.

pub use open_esp_radio_esp32s31_pac::{
    CfrValue, CoexTimerClientValue, CoexTimerPtiValue, CoexTimerRegister,
    ExtraSoftApRxBlockAckEntrySnapshot, ForcedRxGain, MAC_TX_POWER_RATE_COUNT,
    MacApReceivePolicySnapshot, MacExtraSoftApRxBlockAckEntryIndex, MacHe20PeerConfig,
    MacHe20PeerError, MacHeBeamformingReportProfile, MacHeBeamformingReportProfileError,
    MacHeErSuAckRateProfile, MacHeTbLinkReservation, MacHeTbProgramError, MacHeTbTidLimit,
    MacHeTid, MacHeTriggerRxDiagnostics, MacHeTriggerTxQueueSnapshot, MacHeTxProgram,
    MacHeTxVectorSnapshot, MacHtAmpduCompletionObservation, MacHtTxProgram, MacInterface,
    MacInterruptEvents, MacInterruptMask, MacInterruptObservation, MacInterruptSnapshot,
    MacItwtClearIndex, MacKeyEntryIndex, MacKeyInstallOutcome, MacLegacyTxProgram,
    MacPartialRuPowerSelector, MacPowerInterruptSnapshot, MacPti, MacRoleReceivePolicy,
    MacRxBlockAckEntryIndex, MacRxBlockAckStartingSequence, MacRxBlockAckTid, MacRxBlockAckWindow,
    MacRxDecodeErrorStatistics, MacRxDecodeErrorStatisticsDelta, MacRxDmaSnapshot,
    MacRxHangStatistics, MacRxHangStatisticsDelta, MacRxPrimaryStatistics, MacRxStatisticsSnapshot,
    MacStaApReceivePlan, MacStaPolicyMode, MacStaReceivePolicySnapshot, MacTxCompletionObservation,
    MacTxDetachOutcome, MacTxDetachReason, MacTxPowerPair, MacTxPowerTable, MacTxPtiCount,
    MacTxPtiProgram, MacTxQueueDetached, MacTxQueueIndex, PbusMemoryGroupBoundary,
    PhyGainMemoryEntry, PhyMemoryError, RxBlockAckEntrySnapshot, RxDcoControlPrepareError,
    RxDcoControlRestoreError, TxBlockAckPayload, TxIqToneControlPrepareError,
    TxIqToneControlRestoreError,
};
