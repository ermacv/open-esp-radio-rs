//! The `[bluetooth]` scenario table: Bluetooth LE workloads and their images.
//!
//! A workload implies its firmware image. Where a workload runs with or
//! without automatic PHY maintenance, that choice is a typed field which
//! selects the image; contradictory combinations are unrepresentable.

use std::path::Path;

use hil_core::{
    context::Context,
    image::ImageClass,
    lab::requirements::Requirements,
    scenario::{Plan, bounded},
};
use oer_hil_protocol::{BluetoothPeripheralTermination, BluetoothSecurityFailure, ResetReason};
use serde::{Deserialize, Serialize};

use crate::{
    Result,
    fixture::bluetooth::{self as fixture, model::Adapter},
    workload::bluetooth::{self as workload, secure_gatt::IrqSampling},
};

// Unit variants of an internally tagged enum would ignore unknown keys;
// empty struct variants keep `deny_unknown_fields` effective.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum BluetoothScenario {
    /// Three plaintext ATT connection cycles on one Trouble/Controller epoch.
    Gatt {},
    /// Automated Numeric Comparison, bonded reconnect and an explicit
    /// terminal fault.
    SecureGatt {
        #[serde(default)]
        shutdown: SecureGattShutdown,
        /// Boundary-only sampling keeps IRQ watermark scans out of traffic;
        /// it is a timing comparison, not memory-stress evidence.
        #[serde(default)]
        irq_sampling: IrqSampling,
    },
    PhyWatchdog {},
    WatchdogReset {},
    MaintenanceDeadline {},
    AclBackpressure {
        /// Run on the image with automatic PHY maintenance.
        #[serde(default)]
        active_maintenance: bool,
    },
    AclCalibration {
        duration_millis: u16,
        minimum_calibrations: u16,
    },
    EncryptedAcl {
        #[serde(default)]
        exercise: EncryptedAclExercise,
    },
    SecurityFailure {
        failure: BluetoothSecurityFailure,
        /// Diagnose plaintext control progress after initial key rejection.
        #[serde(default)]
        read_version_before_disconnect: bool,
    },
    Dtm {
        boots: u8,
        minimum_packets: u16,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        quiet_cycles: Option<u16>,
    },
    Peripheral {
        boots: u8,
        connections: u8,
        hold_millis: u16,
        #[serde(default)]
        termination: BluetoothPeripheralTermination,
        /// Terminal software/ownership retirement after all connection cycles.
        #[serde(default)]
        retire_after: bool,
        /// Powered cold restart between connections in one boot.
        #[serde(default)]
        restart_between_connections: bool,
        #[serde(default)]
        phy_maintenance: PeripheralPhyMaintenance,
    },
}

/// Mutually exclusive terminal proofs; neither substitutes for the other.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SecureGattShutdown {
    #[default]
    BondLoadFailure,
    HciReadFailure,
}

/// What an encrypted ACL connection exercises beyond ordered traffic.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum EncryptedAclExercise {
    #[default]
    None,
    KeyRefresh,
    /// Ordered encrypted traffic before and after live PHY maintenance.
    ActiveMaintenance,
}

/// PHY maintenance around peripheral connections.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum PeripheralPhyMaintenance {
    None {},
    /// The firmware's automatic maintenance policy.
    Automatic {},
    /// Due maintenance requested between connections, preserving HCI and
    /// the powered epoch.
    BetweenConnections {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        calibration_threshold: Option<u8>,
    },
}

impl Default for PeripheralPhyMaintenance {
    fn default() -> Self {
        Self::None {}
    }
}

impl BluetoothScenario {
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::SecurityFailure {
                failure,
                read_version_before_disconnect: true,
            } if *failure != BluetoothSecurityFailure::MissingKey => {
                Err("plaintext verification requires initial missing-key rejection".into())
            }
            Self::AclCalibration {
                duration_millis,
                minimum_calibrations,
            } => {
                bounded(*duration_millis, 10_000, 20_000, "duration_millis")?;
                bounded(*minimum_calibrations, 2, 20, "minimum_calibrations")
            }
            Self::Dtm {
                boots,
                minimum_packets,
                quiet_cycles,
            } => {
                bounded(*boots, 1, 10, "boots")?;
                bounded(*minimum_packets, 1, 160, "minimum_packets")?;
                if let Some(cycles) = quiet_cycles {
                    bounded(*cycles, 1, 1000, "quiet_cycles")?;
                }
                Ok(())
            }
            Self::Peripheral {
                boots,
                connections,
                hold_millis,
                restart_between_connections,
                phy_maintenance,
                ..
            } => {
                bounded(*boots, 1, 10, "boots")?;
                bounded(*connections, 1, 100, "connections")?;
                bounded(*hold_millis, 0, 5_000, "hold_millis")?;
                let between = matches!(
                    phy_maintenance,
                    PeripheralPhyMaintenance::BetweenConnections { .. }
                );
                if (*restart_between_connections || between) && *connections < 2 {
                    return Err(
                        "inter-connection lifecycle requires at least two connections in one boot"
                            .into(),
                    );
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    pub fn image(&self) -> ImageClass {
        match self {
            Self::Gatt {} => ImageClass::BluetoothGatt,
            Self::SecureGatt { .. } => ImageClass::BluetoothSecureGatt,
            Self::PhyWatchdog {} | Self::WatchdogReset {} => ImageClass::BluetoothWatchdogReset,
            Self::MaintenanceDeadline {} | Self::AclCalibration { .. } => {
                ImageClass::BluetoothPhyMaintenance
            }
            Self::AclBackpressure {
                active_maintenance: true,
            }
            | Self::EncryptedAcl {
                exercise: EncryptedAclExercise::ActiveMaintenance,
            }
            | Self::Peripheral {
                phy_maintenance: PeripheralPhyMaintenance::Automatic {},
                ..
            } => ImageClass::BluetoothPhyMaintenance,
            Self::AclBackpressure { .. }
            | Self::EncryptedAcl { .. }
            | Self::SecurityFailure { .. }
            | Self::Dtm { .. }
            | Self::Peripheral { .. } => ImageClass::BluetoothDtm,
        }
    }

    pub fn plan(&self) -> Plan {
        Plan {
            requirements: Requirements {
                bluetooth_adapter: true,
                ..Requirements::default()
            },
            ..Plan::target_only(self.image())
        }
    }

    /// The Linux adapter capabilities this workload needs.
    pub fn adapter_preflight(&self) -> fn(Adapter) -> Result<()> {
        match self {
            Self::Gatt {} | Self::SecureGatt { .. } | Self::AclBackpressure { .. } => {
                fixture::att::preflight
            }
            Self::AclCalibration { .. } => fixture::att_parameters::preflight,
            Self::Dtm { .. } | Self::WatchdogReset {} | Self::MaintenanceDeadline {} => {
                fixture::preflight
            }
            Self::Peripheral { .. } | Self::EncryptedAcl { .. } | Self::PhyWatchdog {} => {
                fixture::preflight_connect_reset
            }
            Self::SecurityFailure { .. } => fixture::preflight_security_failure,
        }
    }

    pub fn run(&self, output: &Path, context: &Context<'_>) -> Result<()> {
        match self {
            Self::Gatt {} => workload::gatt::run(output, context),
            Self::SecureGatt {
                shutdown,
                irq_sampling,
            } => workload::secure_gatt::run(output, context, *shutdown, *irq_sampling),
            Self::PhyWatchdog {} => workload::phy_watchdog::run(output, context),
            Self::WatchdogReset {} => {
                workload::deadline::run(output, context, ResetReason::MainWatchdog1)
            }
            Self::MaintenanceDeadline {} => {
                workload::deadline::run(output, context, ResetReason::Software)
            }
            Self::AclBackpressure { active_maintenance } => {
                workload::backpressure::run(output, context, *active_maintenance)
            }
            Self::AclCalibration {
                duration_millis,
                minimum_calibrations,
            } => {
                workload::calibration::run(*duration_millis, *minimum_calibrations, output, context)
            }
            Self::EncryptedAcl { exercise } => {
                let active_maintenance = *exercise == EncryptedAclExercise::ActiveMaintenance;
                workload::run_peripheral(
                    workload::PeripheralConfig {
                        boots: 1,
                        connections: 2,
                        hold_millis: if active_maintenance { 1000 } else { 0 },
                        termination: BluetoothPeripheralTermination::PeerReset,
                        retire_after: true,
                        restart_between_connections: false,
                        maintain_between_connections: false,
                        calibration_threshold: None,
                        encrypted: true,
                        key_refresh: *exercise == EncryptedAclExercise::KeyRefresh,
                        encrypted_maintenance: active_maintenance,
                    },
                    output,
                    context,
                )
            }
            Self::SecurityFailure {
                failure,
                read_version_before_disconnect,
            } => workload::security_failure::run(
                *failure,
                *read_version_before_disconnect,
                output,
                context,
            ),
            Self::Dtm {
                boots,
                minimum_packets,
                quiet_cycles,
            } => workload::run(*boots, *minimum_packets, *quiet_cycles, output, context),
            Self::Peripheral {
                boots,
                connections,
                hold_millis,
                termination,
                retire_after,
                restart_between_connections,
                phy_maintenance,
            } => {
                let (maintain_between_connections, calibration_threshold) = match phy_maintenance {
                    PeripheralPhyMaintenance::BetweenConnections {
                        calibration_threshold,
                    } => (true, *calibration_threshold),
                    PeripheralPhyMaintenance::None {} | PeripheralPhyMaintenance::Automatic {} => {
                        (false, None)
                    }
                };
                workload::run_peripheral(
                    workload::PeripheralConfig {
                        encrypted: false,
                        key_refresh: false,
                        encrypted_maintenance: false,
                        boots: *boots,
                        connections: *connections,
                        hold_millis: *hold_millis,
                        termination: *termination,
                        retire_after: *retire_after,
                        restart_between_connections: *restart_between_connections,
                        maintain_between_connections,
                        calibration_threshold,
                    },
                    output,
                    context,
                )
            }
        }
    }
}

#[cfg(test)]
mod tests;
