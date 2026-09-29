//! The `[bluetooth]` scenario table: Bluetooth LE workloads and their images.
//!
//! A workload implies its firmware image.

use std::path::Path;

use hil_core::session::DeviceImageKeys;
use hil_core::{
    context::Context,
    image::ImageClass,
    lab::requirements::Requirements,
    scenario::{Plan, bounded},
};
use oer_hil_protocol::bluetooth;
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
    /// Scannable non-connectable advertising found by an active scanner
    /// through its scan response.
    ScannableAdvertising {},
    /// High duty cycle directed advertising times out; low duty cycle
    /// directed advertising connects its target.
    DirectedAdvertising {},
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
    Dtm {
        boots: u8,
        minimum_packets: u16,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        quiet_cycles: Option<u16>,
    },
    /// Direct Test Mode with the ESP32-C5 reference peer: the ESP32-S31
    /// receives LE 1M, 2M and Coded (S=8 and S=2) and transmits LE 1M and
    /// 2M, with a silence control on each receiver.
    DtmPeer { minimum_packets: u16 },
}

/// Mutually exclusive terminal proofs; neither substitutes for the other.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SecureGattShutdown {
    #[default]
    BondLoadFailure,
    HciReadFailure,
}

impl BluetoothScenario {
    pub fn validate(&self) -> Result<()> {
        match self {
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
            Self::DtmPeer { minimum_packets } => {
                bounded(*minimum_packets, 1, 1000, "minimum_packets")
            }
            _ => Ok(()),
        }
    }

    pub fn image(&self) -> ImageClass {
        match self {
            Self::Gatt {} => ImageClass::BluetoothGatt,
            Self::SecureGatt { .. } => ImageClass::BluetoothSecureGatt,
            Self::Dtm { .. }
            | Self::DtmPeer { .. }
            | Self::ScannableAdvertising {}
            | Self::DirectedAdvertising {} => ImageClass::BluetoothDtm,
        }
    }

    /// The catalog image the reference peer board must carry, when the
    /// scenario uses the peer; [`crate::fixture::dtm_peer`] drives the
    /// Direct Test Mode peer.
    pub fn peer_image(&self) -> Option<hil_core::fixture::peer_line::PeerImage> {
        match self {
            Self::DtmPeer { .. } => Some(crate::fixture::dtm_peer::DTM_PEER_IMAGE),
            _ => None,
        }
    }

    pub fn plan(&self) -> Plan {
        Plan {
            requirements: Requirements {
                bluetooth_adapter: self.adapter_preflight().is_some(),
                peer: self.peer_image().is_some(),
                ..Requirements::default()
            },
            ..Plan::target_only(self.image())
        }
    }

    /// Whether an image of [`Self::image`] reporting `capabilities` runs this
    /// workload: the advertising workloads drive the image's Controller over
    /// raw HCI.
    pub fn served_by(&self, capabilities: &DeviceImageKeys) -> bool {
        match self {
            Self::Gatt {} | Self::SecureGatt { .. } => true,
            Self::ScannableAdvertising {} | Self::DirectedAdvertising {} | Self::DtmPeer { .. } => {
                capabilities.has::<bluetooth::Hci>()
            }
            Self::Dtm { .. } => capabilities.has::<bluetooth::Dtm>(),
        }
    }

    /// The Linux adapter capabilities this workload needs, or `None` when it
    /// uses no Linux adapter.
    pub fn adapter_preflight(&self) -> Option<fn(Adapter) -> Result<()>> {
        Some(match self {
            Self::Gatt {}
            | Self::SecureGatt { .. }
            | Self::ScannableAdvertising {}
            | Self::DirectedAdvertising {} => fixture::att::preflight,
            Self::Dtm { .. } => fixture::preflight,
            Self::DtmPeer { .. } => return None,
        })
    }

    pub fn run(&self, output: &Path, context: &Context<'_>) -> Result<()> {
        match self {
            Self::Gatt {} => workload::gatt::run(output, context),
            Self::ScannableAdvertising {} => workload::scannable::run(output, context),
            Self::DirectedAdvertising {} => workload::directed::run(output, context),
            Self::SecureGatt {
                shutdown,
                irq_sampling,
            } => workload::secure_gatt::run(output, context, *shutdown, *irq_sampling),
            Self::Dtm {
                boots,
                minimum_packets,
                quiet_cycles,
            } => workload::run(*boots, *minimum_packets, *quiet_cycles, output, context),
            Self::DtmPeer { minimum_packets } => {
                workload::dtm_peer::run(*minimum_packets, output, context)
            }
        }
    }
}

#[cfg(test)]
mod tests;
