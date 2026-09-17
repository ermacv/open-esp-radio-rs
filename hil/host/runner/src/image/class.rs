//! HIL image identity and reproducible build feature recipes.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ImageClass {
    BluetoothDtm,
    BluetoothPhyMaintenance,
    BootSmoke,
    Performance,
    Correctness,
    DiagnosticMacIrq,
    DiagnosticTxWait,
    DiagnosticTaskResidence,
    DiagnosticTxArchitecture,
    DiagnosticTaskPoll,
    DiagnosticCore0RxCoarse,
    DiagnosticCore0RxCycles,
    DiagnosticRxDelivery,
    DiagnosticRxDeliveryPhyHotSram,
    /// Historical dirty-worktree experiment retained only to read sealed runs.
    #[serde(rename = "diagnostic-rx-delivery-phy-steps")]
    LegacyDiagnosticRxDeliveryPhySteps,
    /// Historical dirty-worktree experiment retained only to read sealed runs.
    #[serde(rename = "diagnostic-rx-delivery-phy-settle")]
    LegacyDiagnosticRxDeliveryPhySettle,
    DiagnosticIeee802154EventStatus,
    DiagnosticIeee802154EdEvent,
    DiagnosticMemoryBenchmark,
}

impl ImageClass {
    pub const ALL: [Self; 17] = [
        Self::BluetoothDtm,
        Self::BluetoothPhyMaintenance,
        Self::BootSmoke,
        Self::Performance,
        Self::Correctness,
        Self::DiagnosticMacIrq,
        Self::DiagnosticTxWait,
        Self::DiagnosticTaskResidence,
        Self::DiagnosticTxArchitecture,
        Self::DiagnosticTaskPoll,
        Self::DiagnosticCore0RxCoarse,
        Self::DiagnosticCore0RxCycles,
        Self::DiagnosticRxDelivery,
        Self::DiagnosticRxDeliveryPhyHotSram,
        Self::DiagnosticIeee802154EventStatus,
        Self::DiagnosticIeee802154EdEvent,
        Self::DiagnosticMemoryBenchmark,
    ];

    pub const fn id(self) -> &'static str {
        match self {
            Self::BluetoothDtm => "bluetooth-dtm",
            Self::BluetoothPhyMaintenance => "bluetooth-phy-maintenance",
            Self::BootSmoke => "boot-smoke",
            Self::Performance => "performance",
            Self::Correctness => "correctness",
            Self::DiagnosticMacIrq => "diagnostic-mac-irq",
            Self::DiagnosticTxWait => "diagnostic-tx-wait",
            Self::DiagnosticTaskResidence => "diagnostic-task-residence",
            Self::DiagnosticTxArchitecture => "diagnostic-tx-architecture",
            Self::DiagnosticTaskPoll => "diagnostic-task-poll",
            Self::DiagnosticCore0RxCoarse => "diagnostic-core0-rx-coarse",
            Self::DiagnosticCore0RxCycles => "diagnostic-core0-rx-cycles",
            Self::DiagnosticRxDelivery => "diagnostic-rx-delivery",
            Self::DiagnosticRxDeliveryPhyHotSram => "diagnostic-rx-delivery-phy-hot-sram",
            Self::LegacyDiagnosticRxDeliveryPhySteps => "diagnostic-rx-delivery-phy-steps",
            Self::LegacyDiagnosticRxDeliveryPhySettle => "diagnostic-rx-delivery-phy-settle",
            Self::DiagnosticIeee802154EventStatus => "diagnostic-ieee802154-event-status",
            Self::DiagnosticIeee802154EdEvent => "diagnostic-ieee802154-ed-event",
            Self::DiagnosticMemoryBenchmark => "diagnostic-memory-benchmark",
        }
    }

    pub const fn runtime_features(self) -> &'static str {
        match self {
            Self::BluetoothDtm => {
                "bluetooth-hil,phy-rx-hot-sram,psram-task-stack,code-psram,profile-psram-data"
            }
            Self::BluetoothPhyMaintenance => {
                "bluetooth-phy-maintenance,psram-task-stack,code-psram,profile-psram-data"
            }
            Self::BootSmoke => "boot-smoke,psram-task-stack,code-psram,profile-psram-data",
            Self::Performance => "open-radio-hil,psram-task-stack,code-psram,profile-psram-data",
            Self::Correctness => {
                "open-radio-hil,driver-observation,psram-task-stack,code-psram,profile-psram-data"
            }
            Self::DiagnosticMacIrq => {
                "open-radio-hil,psram-task-stack,mac-irq-telemetry,code-psram,profile-psram-data"
            }
            Self::DiagnosticTxWait => {
                "open-radio-hil,psram-task-stack,tx-wait-probe,task-poll-telemetry,code-psram,profile-psram-data"
            }
            Self::DiagnosticTaskResidence => {
                "open-radio-hil,psram-task-stack,task-residence-telemetry,code-psram,profile-psram-data"
            }
            Self::DiagnosticTxArchitecture => {
                "open-radio-hil,psram-task-stack,tx-architecture-probes,code-psram,profile-psram-data"
            }
            Self::DiagnosticTaskPoll => {
                "open-radio-hil,psram-task-stack,task-poll-telemetry,code-psram,profile-psram-data"
            }
            Self::DiagnosticCore0RxCoarse => {
                "open-radio-hil,psram-task-stack,core0-rx-coarse-telemetry,code-psram,profile-psram-data"
            }
            Self::DiagnosticCore0RxCycles => {
                "open-radio-hil,psram-task-stack,core0-rx-cycle-telemetry,code-psram,profile-psram-data"
            }
            Self::DiagnosticRxDeliveryPhyHotSram => {
                "open-radio-hil,psram-task-stack,rx-delivery-telemetry,phy-rx-hot-sram,code-psram,profile-psram-data"
            }
            Self::LegacyDiagnosticRxDeliveryPhySteps => {
                "open-radio-hil,psram-task-stack,rx-delivery-telemetry,phy-rx-step-timing,code-psram,profile-psram-data"
            }
            Self::LegacyDiagnosticRxDeliveryPhySettle => {
                "open-radio-hil,psram-task-stack,rx-delivery-telemetry,phy-short-settle,code-psram,profile-psram-data"
            }
            Self::DiagnosticRxDelivery => {
                "open-radio-hil,psram-task-stack,rx-delivery-telemetry,code-psram,profile-psram-data"
            }
            Self::DiagnosticIeee802154EventStatus => {
                "open-radio-hil,ieee802154-event-status-probe,psram-task-stack,code-psram,profile-psram-data"
            }
            Self::DiagnosticMemoryBenchmark => {
                "open-radio-hil,memory-benchmark,psram-task-stack,code-psram,profile-psram-data"
            }
            Self::DiagnosticIeee802154EdEvent => {
                "open-radio-hil,ieee802154-ed-event-probe,psram-task-stack,code-psram,profile-psram-data"
            }
        }
    }

    pub const fn runtime_profile(self) -> &'static str {
        match self {
            Self::BluetoothDtm
            | Self::BluetoothPhyMaintenance
            | Self::BootSmoke
            | Self::Performance
            | Self::Correctness
            | Self::DiagnosticMacIrq
            | Self::DiagnosticTxWait
            | Self::DiagnosticTaskResidence
            | Self::DiagnosticTxArchitecture
            | Self::DiagnosticTaskPoll
            | Self::DiagnosticCore0RxCoarse
            | Self::DiagnosticCore0RxCycles
            | Self::DiagnosticRxDelivery
            | Self::DiagnosticRxDeliveryPhyHotSram
            | Self::LegacyDiagnosticRxDeliveryPhySteps
            | Self::LegacyDiagnosticRxDeliveryPhySettle
            | Self::DiagnosticIeee802154EventStatus
            | Self::DiagnosticMemoryBenchmark
            | Self::DiagnosticIeee802154EdEvent => "psram-code-psram-data-psram-stack",
        }
    }

    pub const fn uses_psram_task_stack(self) -> bool {
        true
    }

    /// Whether this identity is accepted only while reading immutable old runs.
    pub const fn historical_only(self) -> bool {
        matches!(
            self,
            Self::LegacyDiagnosticRxDeliveryPhySteps | Self::LegacyDiagnosticRxDeliveryPhySettle
        )
    }

    /// Whether the image promises typed driver-internal evidence.
    ///
    /// The task-residence image is deliberately production-like: its only
    /// diagnostic boundary is executor residence, so it must use the same
    /// transport/external-link acceptance path as the performance image.
    pub const fn requires_driver_observation(self) -> bool {
        !matches!(
            self,
            Self::BluetoothDtm
                | Self::BluetoothPhyMaintenance
                | Self::BootSmoke
                | Self::Performance
                | Self::DiagnosticTaskResidence
                | Self::DiagnosticTxArchitecture
                | Self::DiagnosticCore0RxCoarse
                | Self::DiagnosticMemoryBenchmark
        )
    }
}

impl std::str::FromStr for ImageClass {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|class| class.id() == value)
            .ok_or_else(|| format!("unknown image class `{value}`"))
    }
}
