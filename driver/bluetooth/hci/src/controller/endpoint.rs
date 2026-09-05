//! Combined command endpoint: transport, bootstrap and session configuration authority.

use super::*;

/// Result of attempting one unsolicited legacy advertising report publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeLegacyAdvertisingReportPublication {
    /// The complete event entered the bounded Controller-to-Host queue.
    Published,
    /// The Host currently masks either LE Meta or LE Advertising Report events.
    Masked,
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
    pub(super) transport: InProcessHciControllerEndpoint<
        'resources,
        M,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >,
    pub(super) bootstrap: &'resources mut LeControllerBootstrap,
    pub(super) legacy_advertising: &'resources mut LeLegacyAdvertisingConfiguration,
    pub(super) legacy_scanning: &'resources mut LeLegacyScanningConfiguration,
    pub(super) initial_ready_available: &'resources mut bool,
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
