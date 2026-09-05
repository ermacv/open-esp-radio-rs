//! Statically bounded resources and the sole combined Controller epoch.

use embassy_sync::blocking_mutex::raw::RawMutex;

use crate::command::advertising::LeLegacyAdvertisingConfiguration;

use crate::command::scanning::{
    LeLegacyScanningConfiguration, LeLegacyScanningIdleEnableDisposition,
};

use crate::{
    BOOTSTRAP_COMMAND_COMPLETE_EVENT_CAPACITY, BootstrapCommandCompleteEvent, BootstrapPhase,
    HciControllerResponse, InProcessHciChannel, InProcessHciControllerEndpoint,
    InProcessHciHostTransport, LE_DTM_COMMAND_COMPLETE_EVENT_CAPACITY,
    LE_LEGACY_ADVERTISING_REPORT_EVENT_CAPACITY, LeControllerBootstrap,
    LeControllerBootstrapConfig, LeControllerCommandReady, LeLegacyAdvertisingCommandCompleteEvent,
    LeLegacyAdvertisingConfigurationCommand, LeLegacyAdvertisingEnableCommand,
    LeLegacyAdvertisingIdleEnableDisposition, LeLegacyAdvertisingReportEvent,
    LeLegacyScanningCommandCompleteEvent, LeLegacyScanningConfigurationCommand,
    LeLegacyScanningEnableCommand, OwnedBootstrapCommand,
};

mod endpoint;
pub use endpoint::{
    LeControllerCommandEndpoint, LeControllerCommandReadyClaim,
    LeLegacyAdvertisingReportPublication,
};

const HCI_ACL_HEADER_BYTES: usize = 4;

/// Why an HCI runtime profile cannot represent its advertised LE resources.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeControllerHciResourcesError {
    /// A complete command, event or advertised LE ACL packet does not fit one
    /// transport slot.
    PacketCapacityTooSmall {
        /// Minimum packet-body storage required by the profile.
        required: usize,
        /// Compile-time storage selected by the caller.
        available: usize,
    },
    /// The Controller advertised more simultaneous Host ACL credits than its
    /// source-owned inbound queue can retain.
    AclCreditsExceedHostQueue {
        /// Credits reported by LE Read Buffer Size.
        credits: usize,
        /// Complete Host packet slots owned by this epoch.
        slots: usize,
    },
}

/// Complete disjoint endpoints borrowed from one HCI resource epoch.
///
/// The Host transport is independent, while all Controller command authority
/// remains in one [`LeControllerCommandEndpoint`]. The shared lifetime prevents
/// a second split until both endpoints are returned.
#[must_use = "all HCI endpoints belong to one resource epoch"]
pub struct LeControllerHciEndpoints<
    'resources,
    M,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> where
    M: RawMutex,
{
    /// Host-facing typed HCI transport.
    pub host: InProcessHciHostTransport<
        'resources,
        M,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >,
    /// Sole combined transport and bootstrap command endpoint.
    pub controller: LeControllerCommandEndpoint<
        'resources,
        M,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >,
}

/// Allocation-free transport, bootstrap, and Link Layer configuration for one HCI epoch.
///
/// The aggregate replaces a vendor packet mempool, global HCI environment and
/// callback-broker node with bounded packet queues and typed bootstrap state.
/// Its reset-scoped advertising owner retains parameters, advertising data,
/// and scan-response data until Enable captures one role-specific snapshot.
/// It is neither `Copy` nor `Clone`; splitting requires a unique mutable borrow
/// and yields the only Host and combined command endpoints for that epoch.
#[must_use = "HCI runtime resources must remain owned by their Controller epoch"]
pub struct LeControllerHciResources<
    M,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> where
    M: RawMutex,
{
    channel:
        InProcessHciChannel<M, HOST_TO_CONTROLLER_DEPTH, CONTROLLER_TO_HOST_DEPTH, PACKET_CAPACITY>,
    bootstrap: LeControllerBootstrap,
    legacy_advertising: LeLegacyAdvertisingConfiguration,
    legacy_scanning: LeLegacyScanningConfiguration,
    initial_ready_available: bool,
}

impl<
    M,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> LeControllerHciResources<M, HOST_TO_CONTROLLER_DEPTH, CONTROLLER_TO_HOST_DEPTH, PACKET_CAPACITY>
where
    M: RawMutex,
{
    /// Construct an empty HCI epoch whose storage covers every advertised
    /// initial LE packet and credit.
    pub fn new(config: LeControllerBootstrapConfig) -> Result<Self, LeControllerHciResourcesError> {
        let acl_packet_capacity =
            usize::from(config.le_acl_data_packet_length()).saturating_add(HCI_ACL_HEADER_BYTES);
        let required = acl_packet_capacity
            .max(BOOTSTRAP_COMMAND_COMPLETE_EVENT_CAPACITY)
            .max(LE_DTM_COMMAND_COMPLETE_EVENT_CAPACITY)
            .max(LE_LEGACY_ADVERTISING_REPORT_EVENT_CAPACITY);
        if PACKET_CAPACITY < required {
            return Err(LeControllerHciResourcesError::PacketCapacityTooSmall {
                required,
                available: PACKET_CAPACITY,
            });
        }

        let credits = usize::from(config.total_num_le_acl_data_packets());
        if credits > HOST_TO_CONTROLLER_DEPTH {
            return Err(LeControllerHciResourcesError::AclCreditsExceedHostQueue {
                credits,
                slots: HOST_TO_CONTROLLER_DEPTH,
            });
        }

        Ok(Self {
            channel: InProcessHciChannel::new(),
            bootstrap: LeControllerBootstrap::new(config),
            legacy_advertising: LeLegacyAdvertisingConfiguration::new(),
            legacy_scanning: LeLegacyScanningConfiguration::new(),
            initial_ready_available: true,
        })
    }

    /// Immutable bootstrap profile retained by this exact epoch.
    pub const fn config(&self) -> LeControllerBootstrapConfig {
        self.bootstrap.config()
    }

    /// Whether initial command authority remains unclaimed and no packet or
    /// successful bootstrap command has entered this epoch.
    pub fn is_pristine(&self) -> bool {
        self.initial_ready_available && self.channel.is_pristine() && self.bootstrap.is_pristine()
    }

    /// Borrow the only Host transport and combined Controller command endpoint.
    pub fn split(
        &mut self,
    ) -> LeControllerHciEndpoints<
        '_,
        M,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    > {
        let (host, transport) = self.channel.split();
        LeControllerHciEndpoints {
            host,
            controller: LeControllerCommandEndpoint {
                transport,
                bootstrap: &mut self.bootstrap,
                legacy_advertising: &mut self.legacy_advertising,
                legacy_scanning: &mut self.legacy_scanning,
                initial_ready_available: &mut self.initial_ready_available,
            },
        }
    }
}

#[cfg(test)]
mod tests;
