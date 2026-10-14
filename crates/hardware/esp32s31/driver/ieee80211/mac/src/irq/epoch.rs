//! Finite platform route ownership around the executor-independent IRQ consumer.

use super::consumer::{CanModerate, IrqConsumer, IrqDrain, PowerIrqConsumer, RxDelivery};

use oer_esp32s31_hal::types::{MacInterruptMask, MacPowerInterruptObservation};

use super::MacInterruptRoute;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacInterruptEpochStateError {
    Active,
    Quiesced,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacInterruptEpochActivateError<E> {
    AlreadyActive,
    Route(E),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacInterruptEpochQuiesceError<E> {
    AlreadyQuiesced,
    Route(E),
}

/// Complete stale executor publication removed after a hardware interrupt
/// epoch is closed.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MacInterruptEpochDrain {
    pub mac: IrqDrain,
    pub power_events: MacPowerInterruptObservation,
}

/// Persistent setup owner for repeated connected MAC interrupt epochs.
///
/// The inactive state contains the task-side setup token. The active state
/// lends that token to a platform route. Quiescence first recovers the exact
/// token and only then drains coalesced IRQ publications, preventing one
/// epoch's wake from becoming work in the next epoch.
// CAPABILITY: wifi-interrupt-epoch-owners
pub struct InterruptEpoch<'runtime, R, M: RxDelivery>
where
    R: MacInterruptRoute,
{
    // `Option` is not lifecycle state: `setup` remains the active/inactive
    // discriminator. It only lets `Drop` retain an accidentally-live route
    // without running `R::drop` while hardware can still observe it.
    pub(super) route: Option<R>,
    pub(super) setup: Option<R::Setup>,
    pub(super) mac_consumer: &'runtime IrqConsumer<M>,
    pub(super) power_consumer: &'runtime PowerIrqConsumer,
}

impl<'runtime, R, M> InterruptEpoch<'runtime, R, M>
where
    R: MacInterruptRoute,
    M: RxDelivery,
{
    pub const fn new(
        route: R,
        setup: R::Setup,
        mac_consumer: &'runtime IrqConsumer<M>,
        power_consumer: &'runtime PowerIrqConsumer,
    ) -> Self {
        Self {
            route: Some(route),
            setup: Some(setup),
            mac_consumer,
            power_consumer,
        }
    }

    pub const fn is_active(&self) -> bool {
        self.setup.is_none()
    }

    /// Executor wake runtime bound to this interrupt epoch.
    ///
    /// The reference cannot activate or quiesce the platform route; it only
    /// lets the finite service owning this epoch await and publish handoff
    /// probes through the matching coalesced wake state.
    pub const fn consumer(&self) -> &'runtime IrqConsumer<M> {
        self.mac_consumer
    }

    /// Whether this active route owns the RX source-moderation contract.
    ///
    /// The bit is lifecycle state retained by the IRQ epoch, not a caller
    /// convention. A successful [`Self::quiesce`] always clears it after the
    /// hardware route has returned its setup token.
    pub fn is_rx_moderated(&self) -> bool {
        self.is_active() && self.mac_consumer.is_rx_moderation_active()
    }

    /// Borrow the task-side capability for polling-only scan/auth phases.
    pub fn setup(&self) -> Result<&R::Setup, MacInterruptEpochStateError> {
        self.setup
            .as_ref()
            .ok_or(MacInterruptEpochStateError::Active)
    }

    /// Mutably borrow the task-side setup capability while the route is idle.
    pub fn setup_mut(&mut self) -> Result<&mut R::Setup, MacInterruptEpochStateError> {
        self.setup
            .as_mut()
            .ok_or(MacInterruptEpochStateError::Active)
    }

    pub fn activate(
        &mut self,
        platform: &R::Platform,
        event_mask: MacInterruptMask,
    ) -> Result<(), MacInterruptEpochActivateError<R::Error>> {
        let setup = self
            .setup
            .take()
            .ok_or(MacInterruptEpochActivateError::AlreadyActive)?;
        let route = self
            .route
            .as_mut()
            .expect("interrupt epoch always retains its route");
        match route.activate(platform, setup, event_mask) {
            Ok(()) => Ok(()),
            Err((error, setup)) => {
                self.setup = Some(setup);
                Err(MacInterruptEpochActivateError::Route(error))
            }
        }
    }

    /// Activate one RX source-moderated interrupt epoch.
    ///
    /// Moderation is enabled before the route becomes visible to hardware, so
    /// the first RX success cannot race ahead of the mask-on-ack policy. An
    /// activation failure restores both the setup token and the inactive
    /// moderation state. Quiescence owns the sole inverse transition.
    pub fn activate_rx_moderated(
        &mut self,
        platform: &R::Platform,
        event_mask: MacInterruptMask,
    ) -> Result<(), MacInterruptEpochActivateError<R::Error>>
    where
        M: CanModerate,
    {
        if self.is_active() {
            return Err(MacInterruptEpochActivateError::AlreadyActive);
        }
        debug_assert!(!self.mac_consumer.is_rx_moderation_active());
        self.mac_consumer.begin_rx_moderation();
        match self.activate(platform, event_mask) {
            Ok(()) => Ok(()),
            Err(error) => {
                self.mac_consumer.end_rx_moderation();
                Err(error)
            }
        }
    }

    /// Enter or resume a logical RX role on the already-installed route.
    ///
    /// The first role consumes the setup token and installs the physical CPU
    /// route. Later STA/AP/monitor cutovers keep that route live and reuse the
    /// same moderation domain; they must not manufacture a deactivate/activate
    /// cycle around a MAC which is still powered and clocked.
    pub fn activate_or_resume_rx_moderated(
        &mut self,
        platform: &R::Platform,
        event_mask: MacInterruptMask,
    ) -> Result<(), MacInterruptEpochActivateError<R::Error>>
    where
        M: CanModerate,
    {
        if self.is_active() {
            assert!(
                self.mac_consumer.is_rx_moderation_active(),
                "an active production MAC route retains RX moderation"
            );
            return Ok(());
        }
        self.activate_rx_moderated(platform, event_mask)
    }

    /// Close only the current logical consumer epoch.
    ///
    /// Hardware publication remains active. Draining coalesced executor state
    /// gives the next role a clean ownership boundary while a concurrently
    /// arriving edge remains durable in the same runtime and is therefore
    /// observed by the next consumer.
    pub fn park(
        &mut self,
    ) -> Result<MacInterruptEpochDrain, MacInterruptEpochQuiesceError<R::Error>> {
        if !self.is_active() {
            return Err(MacInterruptEpochQuiesceError::AlreadyQuiesced);
        }
        Ok(MacInterruptEpochDrain {
            mac: self.mac_consumer.drain_pending(),
            power_events: self.power_consumer.drain_pending(),
        })
    }

    pub fn quiesce(
        &mut self,
        platform: &R::Platform,
    ) -> Result<MacInterruptEpochDrain, MacInterruptEpochQuiesceError<R::Error>> {
        if self.setup.is_some() {
            return Err(MacInterruptEpochQuiesceError::AlreadyQuiesced);
        }
        let setup = self
            .route
            .as_mut()
            .expect("interrupt epoch always retains its route")
            .quiesce(platform)
            .map_err(MacInterruptEpochQuiesceError::Route)?;
        self.setup = Some(setup);
        self.mac_consumer.end_rx_moderation();
        Ok(MacInterruptEpochDrain {
            mac: self.mac_consumer.drain_pending(),
            power_events: self.power_consumer.drain_pending(),
        })
    }

    /// Recover every interrupt resource after the route returned its setup
    /// token and the executor publications were drained.
    ///
    /// An active epoch is returned unchanged. Consequently no caller can
    /// extract or drop an installed route through this API.
    pub fn try_into_inactive_parts(mut self) -> Result<InactiveIrqParts<'runtime, R, M>, Self> {
        if self.is_active() {
            return Err(self);
        }
        let route = self.route.take().expect("inactive epoch retains its route");
        let setup = self
            .setup
            .take()
            .expect("inactive epoch retains its setup token");
        Ok(InactiveIrqParts {
            route,
            setup,
            mac: self.mac_consumer,
            power: self.power_consumer,
        })
    }
}

impl<R, M> Drop for InterruptEpoch<'_, R, M>
where
    R: MacInterruptRoute,
    M: RxDelivery,
{
    fn drop(&mut self) {
        if self.is_active() {
            // `Drop` cannot perform the async owner protocol used by the
            // surrounding radio service. Most importantly, it must not drop
            // a route which is still installed in the interrupt controller.
            // Retain that finite capability until board reset. Normal code
            // closes the epoch explicitly and therefore drops `R` normally.
            core::mem::forget(self.route.take());
        }
    }
}

/// Every recovered resource after the route is detached and its facts drained.
pub struct InactiveIrqParts<'irq, R: MacInterruptRoute, M: RxDelivery> {
    pub route: R,
    pub setup: R::Setup,
    pub mac: &'irq IrqConsumer<M>,
    pub power: &'irq PowerIrqConsumer,
}
