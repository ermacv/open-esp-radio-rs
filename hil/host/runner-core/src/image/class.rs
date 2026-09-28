//! HIL image identity and reproducible build feature recipes.

use oer_hil_protocol::FeatureCapabilities;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ImageClass {
    SystemWatchdog,
    BluetoothHci,
    /// The HCI passthrough image that also restarts and retires its
    /// Controller epoch on request.
    BluetoothHciDiagnostics,
    BluetoothGatt,
    BluetoothSecureGatt,
    BootSmoke,
    Performance,
    Correctness,
    /// The performance Wi-Fi image with the Bluetooth LE GATT application
    /// running beside it on the shared radio.
    WifiBleCoex,
    DiagnosticMacIrq,
    DiagnosticTxWait,
    DiagnosticTaskResidence,
    DiagnosticTxArchitecture,
    DiagnosticTaskPoll,
    DiagnosticCore0RxCoarse,
    DiagnosticCore0RxCycles,
    DiagnosticRxDelivery,
    DiagnosticRxOwnership,
    /// The performance image with only the Wi-Fi system's diagnostics: the
    /// station's exit evidence without the executor and PHY observers.
    DiagnosticStationExit,
    DiagnosticIeee802154EventStatus,
    DiagnosticIeee802154EdEvent,
    DiagnosticIeee802154Radio,
    /// The IEEE 802.15.4 radio image with OpenThread over its client.
    DiagnosticIeee802154Thread,
    /// The IEEE 802.15.4 same-bit and level-retrigger route probe.
    DiagnosticIeee802154Route,
    DiagnosticMemoryBenchmark,
}

impl ImageClass {
    /// Effective Cargo feature selection used by both the builder and provenance.
    pub fn build_features(self, network: super::Integration) -> String {
        if matches!(
            self,
            Self::SystemWatchdog
                | Self::BluetoothGatt
                | Self::BluetoothSecureGatt
                | Self::BluetoothHci
                | Self::BluetoothHciDiagnostics
        ) {
            self.runtime_features().to_owned()
        } else {
            format!("{},{}", self.runtime_features(), network.feature())
        }
    }

    pub const ALL: [Self; 25] = [
        Self::BluetoothSecureGatt,
        Self::BluetoothGatt,
        Self::SystemWatchdog,
        Self::BluetoothHci,
        Self::BluetoothHciDiagnostics,
        Self::BootSmoke,
        Self::Performance,
        Self::Correctness,
        Self::WifiBleCoex,
        Self::DiagnosticMacIrq,
        Self::DiagnosticTxWait,
        Self::DiagnosticTaskResidence,
        Self::DiagnosticTxArchitecture,
        Self::DiagnosticTaskPoll,
        Self::DiagnosticCore0RxCoarse,
        Self::DiagnosticCore0RxCycles,
        Self::DiagnosticRxDelivery,
        Self::DiagnosticRxOwnership,
        Self::DiagnosticStationExit,
        Self::DiagnosticIeee802154EventStatus,
        Self::DiagnosticIeee802154EdEvent,
        Self::DiagnosticIeee802154Radio,
        Self::DiagnosticIeee802154Thread,
        Self::DiagnosticIeee802154Route,
        Self::DiagnosticMemoryBenchmark,
    ];

    pub const fn id(self) -> &'static str {
        match self {
            Self::SystemWatchdog => "system-watchdog",
            Self::BluetoothHci => "bluetooth-hci",
            Self::BluetoothHciDiagnostics => "bluetooth-hci-diagnostics",
            Self::BluetoothGatt => "bluetooth-gatt",
            Self::BluetoothSecureGatt => "bluetooth-secure-gatt",
            Self::BootSmoke => "boot-smoke",
            Self::Performance => "performance",
            Self::Correctness => "correctness",
            Self::WifiBleCoex => "wifi-ble-coex",
            Self::DiagnosticMacIrq => "diagnostic-mac-irq",
            Self::DiagnosticTxWait => "diagnostic-tx-wait",
            Self::DiagnosticTaskResidence => "diagnostic-task-residence",
            Self::DiagnosticTxArchitecture => "diagnostic-tx-architecture",
            Self::DiagnosticTaskPoll => "diagnostic-task-poll",
            Self::DiagnosticCore0RxCoarse => "diagnostic-core0-rx-coarse",
            Self::DiagnosticCore0RxCycles => "diagnostic-core0-rx-cycles",
            Self::DiagnosticRxDelivery => "diagnostic-rx-delivery",
            Self::DiagnosticRxOwnership => "diagnostic-rx-ownership",
            Self::DiagnosticStationExit => "diagnostic-station-exit",
            Self::DiagnosticIeee802154EventStatus => "diagnostic-ieee802154-event-status",
            Self::DiagnosticIeee802154EdEvent => "diagnostic-ieee802154-ed-event",
            Self::DiagnosticIeee802154Radio => "diagnostic-ieee802154-radio",
            Self::DiagnosticIeee802154Thread => "diagnostic-ieee802154-thread",
            Self::DiagnosticIeee802154Route => "diagnostic-ieee802154-route",
            Self::DiagnosticMemoryBenchmark => "diagnostic-memory-benchmark",
        }
    }

    /// The capabilities a dedicated console image of this class declares,
    /// or `None` for an image that reports the Wi-Fi HIL profile.
    pub fn console_capabilities(self) -> Option<FeatureCapabilities> {
        let console = FeatureCapabilities {
            structured_evidence: true,
            psram_task_stack: true,
            ..FeatureCapabilities::default()
        };
        let dtm = FeatureCapabilities {
            bluetooth_dtm: true,
            ..console
        };
        Some(match self {
            Self::SystemWatchdog => FeatureCapabilities {
                system_watchdog: true,
                ..console
            },
            Self::BluetoothGatt => FeatureCapabilities {
                bluetooth_gatt: true,
                ..console
            },
            Self::BluetoothSecureGatt => FeatureCapabilities {
                bluetooth_secure_gatt: true,
                ..console
            },
            // The HCI passthrough image serves Direct Test Mode and raw HCI
            // commands, events and ACL data.
            Self::BluetoothHci => FeatureCapabilities {
                phy_rx_hot_sram: true,
                bluetooth_hci: true,
                ..dtm
            },
            Self::BluetoothHciDiagnostics => FeatureCapabilities {
                phy_rx_hot_sram: true,
                bluetooth_hci: true,
                bluetooth_hci_lifecycle: true,
                ..dtm
            },
            _ => return None,
        })
    }

    /// Whether the image compiles the program-counter sampler
    /// (`pc-profile`), so a scenario can request a profile of it.
    pub const fn samples_program_counter(self) -> bool {
        matches!(
            self,
            Self::DiagnosticTaskResidence | Self::DiagnosticTxArchitecture
        )
    }

    pub const fn runtime_features(self) -> &'static str {
        match self {
            Self::BluetoothGatt => "bluetooth-gatt",
            Self::BluetoothSecureGatt => "bluetooth-secure-gatt",
            Self::SystemWatchdog => "system-watchdog",
            Self::BluetoothHci => "bluetooth-hil,phy-rx-hot-sram",
            Self::BluetoothHciDiagnostics => {
                "bluetooth-hil,phy-rx-hot-sram,bluetooth-hci-lifecycle"
            }
            Self::BootSmoke => "boot-smoke",
            Self::Performance => "open-radio-hil",
            Self::WifiBleCoex => "wifi-ble-coex",
            Self::DiagnosticRxOwnership => "open-radio-hil,rx-ownership-telemetry",
            Self::DiagnosticStationExit => "open-radio-hil,station-exit-evidence",
            Self::Correctness => "open-radio-hil,driver-observation",
            Self::DiagnosticMacIrq => "open-radio-hil,mac-irq-telemetry",
            Self::DiagnosticTxWait => "open-radio-hil,tx-wait-probe,task-poll-telemetry",
            Self::DiagnosticTaskResidence => "open-radio-hil,task-residence-telemetry",
            Self::DiagnosticTxArchitecture => "open-radio-hil,tx-architecture-probes",
            Self::DiagnosticTaskPoll => "open-radio-hil,task-poll-telemetry",
            Self::DiagnosticCore0RxCoarse => "open-radio-hil,core0-rx-coarse-telemetry",
            Self::DiagnosticCore0RxCycles => "open-radio-hil,core0-rx-cycle-telemetry",
            Self::DiagnosticRxDelivery => "open-radio-hil,rx-delivery-telemetry",
            Self::DiagnosticIeee802154EventStatus => "open-radio-hil,ieee802154-event-status-probe",
            Self::DiagnosticMemoryBenchmark => "open-radio-hil,memory-benchmark",
            Self::DiagnosticIeee802154EdEvent => "open-radio-hil,ieee802154-ed-event-probe",
            Self::DiagnosticIeee802154Radio => "open-radio-hil,ieee802154-radio",
            Self::DiagnosticIeee802154Thread => "open-radio-hil,ieee802154-thread",
            Self::DiagnosticIeee802154Route => "open-radio-hil,ieee802154-route-probe",
        }
    }

    /// The one stage-two placement every image uses: code, data and task
    /// stacks in PSRAM, interrupt stacks in SRAM.
    pub const fn runtime_profile(self) -> &'static str {
        "psram-code-psram-data-psram-stack"
    }

    /// Whether the image promises typed driver-internal evidence.
    ///
    /// The task-residence image is deliberately production-like: its only
    /// diagnostic boundary is executor residence, so it must use the same
    /// transport/external-link acceptance path as the performance image.
    pub const fn requires_driver_observation(self) -> bool {
        !matches!(
            self,
            Self::SystemWatchdog
                | Self::BluetoothGatt
                | Self::BluetoothSecureGatt
                | Self::BluetoothHci
                | Self::BluetoothHciDiagnostics
                | Self::BootSmoke
                | Self::Performance
                | Self::WifiBleCoex
                | Self::DiagnosticRxOwnership
                | Self::DiagnosticStationExit
                | Self::DiagnosticTaskResidence
                | Self::DiagnosticTxArchitecture
                | Self::DiagnosticCore0RxCoarse
                | Self::DiagnosticMemoryBenchmark
        )
    }

    /// Images that run the Wi-Fi station and AP stack.
    pub const fn is_wifi(self) -> bool {
        matches!(
            self,
            Self::Correctness
                | Self::Performance
                | Self::WifiBleCoex
                | Self::DiagnosticMacIrq
                | Self::DiagnosticTxWait
                | Self::DiagnosticTaskResidence
                | Self::DiagnosticTxArchitecture
                | Self::DiagnosticTaskPoll
                | Self::DiagnosticCore0RxCoarse
                | Self::DiagnosticCore0RxCycles
                | Self::DiagnosticRxDelivery
                | Self::DiagnosticRxOwnership
                | Self::DiagnosticStationExit
        )
    }

    /// Images whose Core0 RX phase totals use u32 cycle accumulators.
    pub const fn is_core0_rx_cycle_diagnostic(self) -> bool {
        matches!(
            self,
            Self::DiagnosticCore0RxCoarse | Self::DiagnosticCore0RxCycles
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
