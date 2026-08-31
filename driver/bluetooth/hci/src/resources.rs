//! Affine storage for one source-owned HCI Controller epoch.

use embassy_sync::blocking_mutex::raw::RawMutex;

use crate::{
    BOOTSTRAP_COMMAND_COMPLETE_EVENT_CAPACITY, BootstrapCommandCompleteEvent, BootstrapPhase,
    InProcessHciChannel, InProcessHciControllerEndpoint, InProcessHciHostTransport,
    LE_DTM_COMMAND_COMPLETE_EVENT_CAPACITY, LeControllerBootstrap, LeControllerBootstrapConfig,
    OwnedBootstrapCommand,
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

/// Sole command-side authority for one HCI resource epoch.
///
/// Transport and mutable bootstrap state are private and cannot be separated.
/// A session runner may borrow the transport for lossless intake/publication
/// and retain a classified command across radio transitions. Shared observation
/// does not grant mutation of either underlying owner or bypass session policy.
#[must_use = "the command endpoint is the sole Controller-side epoch authority"]
pub struct LeControllerCommandEndpoint<
    'resources,
    M,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> where
    M: RawMutex,
{
    transport: InProcessHciControllerEndpoint<
        'resources,
        M,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >,
    bootstrap: &'resources mut LeControllerBootstrap,
}

impl<
    'resources,
    M,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
>
    LeControllerCommandEndpoint<
        'resources,
        M,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >
where
    M: RawMutex,
{
    /// Shared access to the sole transport endpoint for this Controller epoch.
    pub const fn transport(
        &self,
    ) -> &InProcessHciControllerEndpoint<
        'resources,
        M,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    > {
        &self.transport
    }

    /// Immutable bootstrap profile retained by this exact Controller epoch.
    pub const fn bootstrap_config(&self) -> LeControllerBootstrapConfig {
        self.bootstrap.config()
    }

    /// Current accepted bootstrap progression for this Controller epoch.
    pub const fn bootstrap_phase(&self) -> BootstrapPhase {
        self.bootstrap.phase()
    }

    /// Dispatch after the combined classified router has validated epoch and Reset policy.
    pub(crate) fn dispatch_bootstrap_command(
        &mut self,
        command: OwnedBootstrapCommand,
    ) -> BootstrapCommandCompleteEvent {
        self.bootstrap.dispatch_owned(command)
    }
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

/// Allocation-free transport and bootstrap state for exactly one HCI epoch.
///
/// The aggregate replaces a vendor packet mempool, global HCI environment and
/// callback-broker node with bounded packet queues and typed bootstrap state.
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
            .max(LE_DTM_COMMAND_COMPLETE_EVENT_CAPACITY);
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
        self.channel.is_pristine() && self.bootstrap.is_pristine()
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
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use bt_hci::{
        ControllerToHostPacket, FromHciBytes, PacketKind,
        cmd::{
            Cmd,
            controller_baseband::{Reset, SetEventMask},
        },
        event::{CommandComplete, CommandCompleteWithStatus, EventKind},
        param::{EventMask, Status},
        transport::Transport,
    };
    use embassy_futures::block_on;
    use embassy_sync::blocking_mutex::raw::NoopRawMutex;

    use super::{LeControllerHciResources, LeControllerHciResourcesError};
    use crate::{
        BluetoothPublicDeviceAddress, BootstrapPhase, HostToControllerFrame,
        LeControllerBootstrapConfig, LeControllerClassifiedCommandRoute,
        LeControllerCommandClassification, LeControllerResponsePending,
        LeControllerResponsePublication, OwnedBootstrapCommand, classify_le_controller_command,
    };

    const HARDWARE_ERROR: [u8; 3] = [0x10, 0x01, 0x42];

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
    fn one_split_exposes_host_and_the_matching_combined_command_endpoint() {
        let mut resources = LeControllerHciResources::<NoopRawMutex, 1, 1, 31>::new(config(27, 1))
            .expect("profile fits its source-owned storage");
        assert!(resources.is_pristine());

        {
            let endpoints = resources.split();
            assert_eq!(endpoints.controller.bootstrap_config(), config(27, 1));
            assert_eq!(
                endpoints.controller.bootstrap_phase(),
                BootstrapPhase::AwaitingReset
            );
            block_on(async {
                endpoints
                    .host
                    .write(&Reset::new())
                    .await
                    .expect("Reset enters the bounded queue");
                let mut command_buffer = [0; 31];
                let HostToControllerFrame::Command(command) = endpoints
                    .controller
                    .transport()
                    .receive(&mut command_buffer)
                    .await
                    .expect("the combined command endpoint receives Reset")
                else {
                    panic!("Reset changed HCI packet class");
                };
                let LeControllerCommandClassification::Bootstrap(command) =
                    classify_le_controller_command(command)
                else {
                    panic!("Reset did not become an owned bootstrap command");
                };
                let response = endpoints.controller.bootstrap.dispatch_owned(command);
                assert_eq!(
                    endpoints.controller.bootstrap_phase(),
                    BootstrapPhase::Configuring
                );
                endpoints
                    .controller
                    .transport()
                    .publish(PacketKind::Event, response.as_bytes())
                    .await
                    .expect("the combined command endpoint publishes completion");

                let mut event_buffer = [0; 31];
                let packet = endpoints
                    .host
                    .read(&mut event_buffer)
                    .await
                    .expect("Host receives matching completion");
                assert_command_complete(packet, Reset::OPCODE, Status::SUCCESS);
            });
        }

        assert!(!resources.is_pristine());
    }

    #[test]
    fn draining_a_command_cannot_reclassify_the_epoch_as_pristine() {
        let mut resources = LeControllerHciResources::<NoopRawMutex, 1, 1, 31>::new(config(27, 1))
            .expect("profile fits its source-owned storage");

        {
            let endpoints = resources.split();
            block_on(endpoints.host.write(&Reset::new())).expect("Reset enters the input queue");
            let mut command_buffer = [0; 31];
            let HostToControllerFrame::Command(command) = endpoints
                .controller
                .transport()
                .try_receive(&mut command_buffer)
                .expect("combined command endpoint drains Reset without dispatching it")
            else {
                panic!("Reset changed HCI packet class");
            };
            assert_eq!(command.opcode(), Reset::OPCODE);
        }

        assert!(!resources.is_pristine());
    }

    #[test]
    fn draining_a_controller_event_cannot_reclassify_the_epoch_as_pristine() {
        let mut resources = LeControllerHciResources::<NoopRawMutex, 1, 1, 31>::new(config(27, 1))
            .expect("profile fits its source-owned storage");

        {
            let endpoints = resources.split();
            endpoints
                .controller
                .transport()
                .try_publish(PacketKind::Event, &HARDWARE_ERROR)
                .expect("Controller event enters the output queue");
            let mut event_buffer = [0; 31];
            let ControllerToHostPacket::Event(hardware_error) =
                block_on(endpoints.host.read(&mut event_buffer))
                    .expect("Host drains the Controller event")
            else {
                panic!("Hardware Error changed HCI packet class");
            };
            assert_eq!(hardware_error.data, &[0x42]);
        }

        assert!(!resources.is_pristine());
    }

    #[test]
    fn combined_router_dispatches_non_reset_once_before_ordered_backpressure() {
        let mut resources = LeControllerHciResources::<NoopRawMutex, 1, 1, 31>::new(config(27, 1))
            .expect("profile fits its source-owned storage");
        let mut endpoints = resources.split();

        let reset_response = endpoints
            .controller
            .bootstrap
            .dispatch_owned(OwnedBootstrapCommand::Reset);
        let prior = LeControllerResponsePending::new(
            (),
            reset_response,
            endpoints.controller.transport().epoch_identity(),
        );
        let LeControllerResponsePublication::Published(published) =
            prior.try_publish(endpoints.controller.transport())
        else {
            panic!("the empty response queue must accept the fixture Reset completion");
        };
        assert_eq!(
            endpoints.controller.bootstrap_phase(),
            BootstrapPhase::Configuring
        );

        let requested_mask = EventMask::new().enable_hardware_error(true);
        block_on(endpoints.host.write(&SetEventMask::new(requested_mask)))
            .expect("Set Event Mask enters the real Host transport");
        let mut command_buffer = [0; 31];
        let classified = match endpoints
            .controller
            .transport()
            .try_receive_classified_command(&mut command_buffer)
        {
            Ok(classified) => classified,
            Err(_) => panic!("the real endpoint must classify Set Event Mask"),
        };
        let LeControllerClassifiedCommandRoute::ResponsePending(pending) = endpoints
            .controller
            .route_classified_command(published, classified)
        else {
            panic!("non-Reset bootstrap must dispatch into the ordered response axis");
        };
        assert_eq!(endpoints.controller.bootstrap.event_mask(), requested_mask);

        let LeControllerResponsePublication::Pending(pending) =
            pending.try_publish(endpoints.controller.transport())
        else {
            panic!("the queued Reset completion must backpressure Set Event Mask");
        };
        assert_eq!(endpoints.controller.bootstrap.event_mask(), requested_mask);

        let mut event_buffer = [0; 31];
        assert_command_complete(
            block_on(endpoints.host.read(&mut event_buffer))
                .expect("Host drains the older Reset completion"),
            Reset::OPCODE,
            Status::SUCCESS,
        );

        let LeControllerResponsePublication::Published(published) =
            pending.try_publish(endpoints.controller.transport())
        else {
            panic!("the retained completion must publish after capacity returns");
        };
        assert_eq!(published.into_owner(), ());
        assert_eq!(endpoints.controller.bootstrap.event_mask(), requested_mask);
        assert_command_complete(
            block_on(endpoints.host.read(&mut event_buffer))
                .expect("Host receives the retried response"),
            SetEventMask::OPCODE,
            Status::SUCCESS,
        );
    }

    fn assert_command_complete(
        packet: ControllerToHostPacket<'_>,
        opcode: bt_hci::cmd::Opcode,
        status: Status,
    ) {
        let ControllerToHostPacket::Event(event) = packet else {
            panic!("Command Complete changed HCI packet class");
        };
        assert_eq!(event.kind, EventKind::CommandComplete);
        let complete = CommandComplete::from_hci_bytes_complete(event.data)
            .expect("event retains a complete Command Complete body");
        let complete: CommandCompleteWithStatus<'_> = complete
            .try_into()
            .expect("Command Complete retains status");
        assert_eq!(complete.cmd_opcode, opcode);
        assert_eq!(complete.status, status);
    }
}
