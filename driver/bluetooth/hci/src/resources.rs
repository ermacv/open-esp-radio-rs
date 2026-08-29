//! Affine storage for one source-owned HCI Controller epoch.

use embassy_sync::blocking_mutex::raw::RawMutex;

use crate::{
    BOOTSTRAP_COMMAND_COMPLETE_EVENT_CAPACITY, HciCommandWorker, InProcessHciChannel,
    InProcessHciHostTransport, LeControllerBootstrap, LeControllerBootstrapConfig,
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

/// Complete borrowed HCI worker produced by one runtime-resource split.
pub type LeControllerHciRuntimeWorker<
    'resources,
    M,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> = HciCommandWorker<
    'resources,
    M,
    &'resources mut LeControllerBootstrap,
    HOST_TO_CONTROLLER_DEPTH,
    CONTROLLER_TO_HOST_DEPTH,
    PACKET_CAPACITY,
>;

/// Allocation-free transport and bootstrap state for exactly one HCI epoch.
///
/// The aggregate replaces a vendor packet mempool, global HCI environment and
/// callback-broker node with bounded packet queues and one typed dispatcher.
/// It is neither `Copy` nor `Clone`; splitting requires a unique mutable borrow
/// and yields the only Host transport and Controller worker for that epoch.
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
        let required = acl_packet_capacity.max(BOOTSTRAP_COMMAND_COMPLETE_EVENT_CAPACITY);
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
        })
    }

    /// Immutable bootstrap profile retained by this exact epoch.
    pub const fn config(&self) -> LeControllerBootstrapConfig {
        self.bootstrap.config()
    }

    /// Whether no packet or successful HCI command has entered this epoch.
    pub fn is_pristine(&self) -> bool {
        self.channel.is_empty() && self.bootstrap.is_pristine()
    }

    /// Borrow the only Host transport and Controller bootstrap worker.
    pub fn split(
        &mut self,
    ) -> (
        InProcessHciHostTransport<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
        LeControllerHciRuntimeWorker<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) {
        let (host, controller) = self.channel.split();
        let worker = HciCommandWorker::new(controller, &mut self.bootstrap);
        (host, worker)
    }
}

#[cfg(test)]
mod tests {
    use bt_hci::{cmd::controller_baseband::Reset, transport::Transport};
    use embassy_futures::block_on;
    use embassy_sync::blocking_mutex::raw::NoopRawMutex;

    use super::{LeControllerHciResources, LeControllerHciResourcesError};
    use crate::{BluetoothPublicDeviceAddress, LeControllerBootstrapConfig};

    fn config(payload: u16, credits: u8) -> LeControllerBootstrapConfig {
        LeControllerBootstrapConfig::new(
            BluetoothPublicDeviceAddress::from_canonical_bytes([2, 3, 5, 7, 11, 13]),
            payload,
            credits,
        )
        .expect("nonzero test profile")
    }

    #[test]
    fn advertised_acl_profile_must_fit_owned_storage_and_credits() {
        assert!(matches!(
            LeControllerHciResources::<NoopRawMutex, 2, 1, 30>::new(config(27, 1)),
            Err(LeControllerHciResourcesError::PacketCapacityTooSmall {
                required: 31,
                available: 30,
            })
        ));
        assert!(matches!(
            LeControllerHciResources::<NoopRawMutex, 1, 1, 31>::new(config(27, 2)),
            Err(LeControllerHciResourcesError::AclCreditsExceedHostQueue {
                credits: 2,
                slots: 1,
            })
        ));
    }

    #[test]
    fn one_split_joins_the_host_to_the_same_pristine_bootstrap_epoch() {
        let mut resources = LeControllerHciResources::<NoopRawMutex, 1, 1, 31>::new(config(27, 1))
            .expect("profile fits its source-owned storage");
        assert!(resources.is_pristine());

        {
            let (host, mut worker) = resources.split();
            block_on(async {
                let reset = Reset::new();
                let (sent, processed) =
                    embassy_futures::join::join(host.write(&reset), worker.process_one()).await;
                sent.expect("Reset enters the bounded queue");
                processed.expect("the matching worker publishes its response");
            });
        }

        assert!(!resources.is_pristine());
    }
}
