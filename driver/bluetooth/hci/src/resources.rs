//! Affine storage for one source-owned HCI Controller epoch.

use embassy_sync::blocking_mutex::raw::RawMutex;

use crate::legacy_advertising::LeLegacyAdvertisingConfiguration;
use crate::legacy_scanning::{
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

const HCI_ACL_HEADER_BYTES: usize = 4;

/// Result of attempting one unsolicited legacy advertising report publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeLegacyAdvertisingReportPublication {
    /// The complete event entered the bounded Controller-to-Host queue.
    Published,
    /// The Host currently masks either LE Meta or LE Advertising Report events.
    Masked,
}

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

/// Result of claiming the sole initial next-command authority for an HCI epoch.
#[must_use = "retain either the command-ready authority or the unchanged owner"]
pub enum LeControllerCommandReadyClaim<'epoch, Owner> {
    /// This owner now carries the epoch's sole affine next-command authority.
    Ready(LeControllerCommandReady<'epoch, Owner>),
    /// The authority was already claimed; the supplied owner is unchanged.
    AlreadyClaimed(Owner),
}

/// Sole command-side authority for one HCI resource epoch.
///
/// Transport and mutable bootstrap state are private and cannot be separated.
/// A session runner can only observe readiness by borrowing its affine command
/// or response token, and can only consume a command through the combined
/// endpoint. Successful intake keeps classification and next-command authority
/// inseparable until one session router consumes both. Shared observation does
/// not grant raw queue access or bypass session policy.
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
    legacy_advertising: &'resources mut LeLegacyAdvertisingConfiguration,
    legacy_scanning: &'resources mut LeLegacyScanningConfiguration,
    initial_ready_available: &'resources mut bool,
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
    /// Crate-internal access to the raw transport behind this combined endpoint.
    ///
    /// Public command intake and response publication deliberately stay on the
    /// combined endpoint so transport access cannot bypass affine command order.
    pub(crate) const fn transport(
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

    /// Claim the sole initial next-command authority for this Controller epoch.
    ///
    /// Dropping the returned token does not recreate authority. Every later
    /// token is returned only by durable publication of an ordered response.
    pub fn claim_initial_command_ready<Owner>(
        &mut self,
        owner: Owner,
    ) -> LeControllerCommandReadyClaim<'resources, Owner> {
        if !*self.initial_ready_available {
            return LeControllerCommandReadyClaim::AlreadyClaimed(owner);
        }
        *self.initial_ready_available = false;
        LeControllerCommandReadyClaim::Ready(LeControllerCommandReady::initial(
            owner,
            self.transport.epoch_identity(),
        ))
    }

    /// Dispatch after the combined classified router has validated epoch and Reset policy.
    pub(crate) fn dispatch_bootstrap_command(
        &mut self,
        command: OwnedBootstrapCommand,
    ) -> BootstrapCommandCompleteEvent {
        let reset = command.is_reset();
        let response = self.bootstrap.dispatch_owned(command);
        if reset {
            self.legacy_advertising.reset();
            self.legacy_scanning.reset();
        }
        response
    }

    pub(crate) fn dispatch_bootstrap_command_while_radio_active(
        &mut self,
        command: OwnedBootstrapCommand,
    ) -> BootstrapCommandCompleteEvent {
        self.bootstrap.dispatch_owned_while_radio_active(command)
    }

    pub(crate) fn dispatch_legacy_advertising_configuration(
        &mut self,
        command: LeLegacyAdvertisingConfigurationCommand,
    ) -> LeLegacyAdvertisingCommandCompleteEvent {
        self.legacy_advertising
            .dispatch(self.bootstrap.phase(), command)
    }

    pub(crate) fn dispatch_idle_legacy_advertising_enable(
        &self,
        command: LeLegacyAdvertisingEnableCommand,
    ) -> LeLegacyAdvertisingIdleEnableDisposition {
        self.legacy_advertising.dispatch_idle_enable(
            self.bootstrap.phase(),
            command,
            self.bootstrap.config().public_address(),
            self.bootstrap.requested_random_address(),
        )
    }

    pub(crate) fn complete_legacy_advertising_enable_while_radio_unavailable(
        &self,
        command: LeLegacyAdvertisingEnableCommand,
    ) -> LeLegacyAdvertisingCommandCompleteEvent {
        LeLegacyAdvertisingConfiguration::complete_enable_while_radio_unavailable(
            self.bootstrap.phase(),
            command,
        )
    }

    pub(crate) fn dispatch_legacy_scanning_configuration(
        &mut self,
        command: LeLegacyScanningConfigurationCommand,
    ) -> LeLegacyScanningCommandCompleteEvent {
        self.legacy_scanning
            .dispatch(self.bootstrap.phase(), command)
    }

    pub(crate) fn dispatch_idle_legacy_scanning_enable(
        &self,
        command: LeLegacyScanningEnableCommand,
    ) -> LeLegacyScanningIdleEnableDisposition {
        self.legacy_scanning
            .dispatch_idle_enable(self.bootstrap.phase(), command)
    }

    pub(crate) fn complete_legacy_scanning_enable_while_radio_unavailable(
        &self,
        command: LeLegacyScanningEnableCommand,
    ) -> LeLegacyScanningCommandCompleteEvent {
        LeLegacyScanningConfiguration::complete_enable_while_radio_unavailable(
            self.bootstrap.phase(),
            command,
        )
    }

    /// Wait until Controller-to-Host capacity may accept a scan report.
    ///
    /// This readiness hint borrows no report and reserves no slot. The caller
    /// must retain the exact event and retry publication after cancellation or
    /// a competing Controller response.
    pub async fn wait_legacy_advertising_report_capacity(&self) {
        self.transport().wait_publish_ready().await;
    }

    /// Publish one standard LE Advertising Report if enabled by the Host masks.
    ///
    /// The event does not consume or mint command-order authority. A full queue
    /// returns the unchanged borrowed event to caller policy through the error.
    pub fn try_publish_legacy_advertising_report(
        &self,
        event: &LeLegacyAdvertisingReportEvent,
    ) -> Result<LeLegacyAdvertisingReportPublication, crate::HciChannelError> {
        if !self.bootstrap.event_mask().is_le_meta_enabled()
            || !self.bootstrap.le_event_mask().is_le_adv_report_enabled()
        {
            return Ok(LeLegacyAdvertisingReportPublication::Masked);
        }
        self.transport()
            .try_publish(event.kind(), event.as_bytes())?;
        Ok(LeLegacyAdvertisingReportPublication::Published)
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
mod tests {
    use bt_hci::{
        ControllerToHostPacket, FromHciBytes,
        cmd::{
            Cmd,
            controller_baseband::{Reset, SetEventMask},
        },
        event::{CommandComplete, CommandCompleteWithStatus, EventKind},
        param::{AddrKind, BdAddr, EventMask, LeAdvEventKind, LeEventMask, Status},
        transport::Transport,
    };
    use embassy_futures::block_on;
    use embassy_sync::blocking_mutex::raw::NoopRawMutex;

    use super::{
        LeControllerCommandReadyClaim, LeControllerHciResources, LeControllerHciResourcesError,
        LeLegacyAdvertisingReportPublication,
    };
    use crate::{
        BluetoothPublicDeviceAddress, BootstrapPhase, HciChannelError, LeControllerBootstrapConfig,
        LeControllerClassifiedCommandRoute, LeControllerCommandIntake,
        LeControllerIdleClassifiedCommandRoute, LeControllerResetCompletion,
        LeControllerResponsePublication, LeLegacyAdvertisingReportEvent, OwnedBootstrapCommand,
    };

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
                required: 45,
                available: 30,
            })
        ));
        assert!(matches!(
            LeControllerHciResources::<NoopRawMutex, 1, 1, 45>::new(config(27, 2)),
            Err(LeControllerHciResourcesError::AclCreditsExceedHostQueue {
                credits: 2,
                slots: 1,
            })
        ));
    }

    #[test]
    fn advertising_reports_honor_masks_and_retain_backpressure() {
        let mut resources = LeControllerHciResources::<NoopRawMutex, 1, 1, 45>::new(config(27, 1))
            .expect("the report event fits this transport profile");
        assert_eq!(
            resources
                .bootstrap
                .dispatch_owned(OwnedBootstrapCommand::Reset)
                .status(),
            Status::SUCCESS
        );
        let event = LeLegacyAdvertisingReportEvent::new(
            LeAdvEventKind::AdvNonconnInd,
            AddrKind::PUBLIC,
            BdAddr::new([1, 2, 3, 4, 5, 6]),
            &[2, 1, 6],
            -60,
        )
        .expect("the report is representable");

        let endpoints = resources.split();
        assert_eq!(
            endpoints
                .controller
                .try_publish_legacy_advertising_report(&event),
            Ok(LeLegacyAdvertisingReportPublication::Masked)
        );
        assert_eq!(
            endpoints
                .controller
                .bootstrap
                .dispatch_owned(OwnedBootstrapCommand::SetEventMask(
                    EventMask::new().enable_le_meta(true),
                ))
                .status(),
            Status::SUCCESS
        );
        assert_eq!(
            endpoints
                .controller
                .bootstrap
                .dispatch_owned(OwnedBootstrapCommand::LeSetEventMask(
                    LeEventMask::new().enable_le_adv_report(true),
                ))
                .status(),
            Status::SUCCESS
        );
        assert_eq!(
            endpoints
                .controller
                .try_publish_legacy_advertising_report(&event),
            Ok(LeLegacyAdvertisingReportPublication::Published)
        );
        assert_eq!(
            endpoints
                .controller
                .try_publish_legacy_advertising_report(&event),
            Err(HciChannelError::Full)
        );

        let mut packet = [0; 45];
        let received = block_on(endpoints.host.read(&mut packet))
            .expect("the Host drains the retained first event");
        let ControllerToHostPacket::Event(received) = received else {
            panic!("the report changed packet kind");
        };
        assert_eq!(received.kind, EventKind::Le);
        assert_eq!(
            endpoints
                .controller
                .try_publish_legacy_advertising_report(&event),
            Ok(LeLegacyAdvertisingReportPublication::Published)
        );
    }

    #[test]
    fn one_split_exposes_host_and_the_matching_combined_command_endpoint() {
        let mut resources = LeControllerHciResources::<NoopRawMutex, 1, 1, 45>::new(config(27, 1))
            .expect("profile fits its source-owned storage");
        assert!(resources.is_pristine());

        {
            let mut endpoints = resources.split();
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
                let mut command_buffer = [0; 45];
                let LeControllerCommandReadyClaim::Ready(ready) =
                    endpoints.controller.claim_initial_command_ready(())
                else {
                    panic!("the fresh endpoint grants command authority once");
                };
                endpoints
                    .controller
                    .wait_command_available(&ready)
                    .await
                    .expect("matching authority can observe command readiness");
                let LeControllerCommandIntake::Command { command, .. } = endpoints
                    .controller
                    .try_receive_classified_command_with_buffer(ready, &mut command_buffer)
                else {
                    panic!("the combined endpoint consumes and classifies Reset");
                };
                let LeControllerIdleClassifiedCommandRoute::ResetBarrier(barrier) =
                    endpoints.controller.route_idle_classified_command(command)
                else {
                    panic!("idle Reset becomes a lifecycle barrier");
                };
                let LeControllerResetCompletion::ResponsePending(pending) = endpoints
                    .controller
                    .complete_reset_after_quiescence(barrier)
                else {
                    panic!("the matching endpoint completes Reset after quiescence");
                };
                assert_eq!(
                    endpoints.controller.bootstrap_phase(),
                    BootstrapPhase::Configuring
                );
                let LeControllerResponsePublication::Published(_) =
                    pending.try_publish(&endpoints.controller)
                else {
                    panic!("the combined endpoint publishes the ordered completion");
                };

                let mut event_buffer = [0; 45];
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
    fn initial_command_ready_can_be_claimed_only_once_across_resplits() {
        let mut resources = LeControllerHciResources::<NoopRawMutex, 1, 1, 45>::new(config(27, 1))
            .expect("profile fits its source-owned storage");

        {
            let mut endpoints = resources.split();
            let LeControllerCommandReadyClaim::Ready(ready) =
                endpoints.controller.claim_initial_command_ready(41_u8)
            else {
                panic!("the pristine epoch exposes its sole initial authority");
            };
            assert_eq!(ready.owner(), &41);
            let LeControllerCommandReadyClaim::AlreadyClaimed(owner) =
                endpoints.controller.claim_initial_command_ready(42_u8)
            else {
                panic!("a second claim cannot mint another authority");
            };
            assert_eq!(owner, 42);
            drop(ready);
        }

        assert!(!resources.is_pristine());
        let mut endpoints = resources.split();
        let LeControllerCommandReadyClaim::AlreadyClaimed(owner) =
            endpoints.controller.claim_initial_command_ready(43_u8)
        else {
            panic!("dropping and resplitting cannot recreate authority");
        };
        assert_eq!(owner, 43);
    }

    #[test]
    fn draining_a_command_cannot_reclassify_the_epoch_as_pristine() {
        let mut resources = LeControllerHciResources::<NoopRawMutex, 1, 1, 45>::new(config(27, 1))
            .expect("profile fits its source-owned storage");

        {
            let mut endpoints = resources.split();
            block_on(endpoints.host.write(&Reset::new())).expect("Reset enters the input queue");
            let mut command_buffer = [0; 45];
            let LeControllerCommandReadyClaim::Ready(ready) =
                endpoints.controller.claim_initial_command_ready(())
            else {
                panic!("the fresh endpoint grants command authority once");
            };
            let LeControllerCommandIntake::Command { .. } = endpoints
                .controller
                .try_receive_classified_command_with_buffer(ready, &mut command_buffer)
            else {
                panic!("the combined endpoint drains Reset only with command authority");
            };
        }

        assert!(!resources.is_pristine());
    }

    #[test]
    fn combined_router_dispatches_non_reset_once_before_ordered_backpressure() {
        let mut resources = LeControllerHciResources::<NoopRawMutex, 1, 1, 45>::new(config(27, 1))
            .expect("profile fits its source-owned storage");
        let mut endpoints = resources.split();

        block_on(endpoints.host.write(&Reset::new()))
            .expect("Reset enters the real Host transport");
        let mut reset_buffer = [0; 45];
        let LeControllerCommandReadyClaim::Ready(initial) =
            endpoints.controller.claim_initial_command_ready(())
        else {
            panic!("the fresh epoch exposes its sole initial authority");
        };
        let LeControllerCommandIntake::Command { command: reset, .. } = endpoints
            .controller
            .try_receive_classified_command_with_buffer(initial, &mut reset_buffer)
        else {
            panic!("the real endpoint classifies Reset under affine authority");
        };
        let LeControllerIdleClassifiedCommandRoute::ResetBarrier(barrier) =
            endpoints.controller.route_idle_classified_command(reset)
        else {
            panic!("idle Reset becomes a barrier before software dispatch");
        };
        let LeControllerResetCompletion::ResponsePending(prior) = endpoints
            .controller
            .complete_reset_after_quiescence(barrier)
        else {
            panic!("the matching endpoint completes the proven-idle Reset");
        };
        let LeControllerResponsePublication::Published(published) =
            prior.try_publish(&endpoints.controller)
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
        let mut command_buffer = [0; 45];
        let LeControllerCommandIntake::Command {
            command: classified,
            ..
        } = endpoints
            .controller
            .try_receive_classified_command_with_buffer(published, &mut command_buffer)
        else {
            panic!("the real endpoint must classify Set Event Mask under authority");
        };
        let LeControllerClassifiedCommandRoute::ResponsePending(pending) =
            endpoints.controller.route_classified_command(classified)
        else {
            panic!("non-Reset bootstrap must dispatch into the ordered response axis");
        };
        assert_eq!(endpoints.controller.bootstrap.event_mask(), requested_mask);

        let LeControllerResponsePublication::Pending(pending) =
            pending.try_publish(&endpoints.controller)
        else {
            panic!("the queued Reset completion must backpressure Set Event Mask");
        };
        assert_eq!(endpoints.controller.bootstrap.event_mask(), requested_mask);

        let mut event_buffer = [0; 45];
        assert_command_complete(
            block_on(endpoints.host.read(&mut event_buffer))
                .expect("Host drains the older Reset completion"),
            Reset::OPCODE,
            Status::SUCCESS,
        );

        let LeControllerResponsePublication::Published(published) =
            pending.try_publish(&endpoints.controller)
        else {
            panic!("the retained completion must publish after capacity returns");
        };
        assert_eq!(published.owner(), &());
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
