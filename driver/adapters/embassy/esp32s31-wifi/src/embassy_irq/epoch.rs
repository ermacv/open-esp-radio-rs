use open_esp_radio_embassy_net::RawMutex;
use open_esp_radio_esp32s31_wifi_mac::irq::MacInterruptRoute;

use super::{EmbassyMacIrqDrain, EmbassyMacIrqRuntime, EmbassyPowerIrqRuntime};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31MacInterruptEpochStateError {
    Active,
    Quiesced,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31MacInterruptEpochActivateError<E> {
    AlreadyActive,
    Route(E),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31MacInterruptEpochQuiesceError<E> {
    AlreadyQuiesced,
    Route(E),
}

/// Complete stale executor publication removed after a hardware interrupt
/// epoch is closed.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Esp32s31MacInterruptEpochDrain {
    pub mac: EmbassyMacIrqDrain,
    pub power_events: u32,
}

/// Persistent setup owner for repeated connected MAC interrupt epochs.
///
/// The inactive state contains the task-side setup token. The active state
/// lends that token to a platform route. Quiescence first recovers the exact
/// token and only then drains coalesced Embassy publications, preventing one
/// epoch's wake from becoming work in the next epoch.
pub struct Esp32s31MacInterruptEpoch<'runtime, R, M: RawMutex>
where
    R: MacInterruptRoute,
{
    // `Option` is not lifecycle state: `setup` remains the active/inactive
    // discriminator. It only lets `Drop` retain an accidentally-live route
    // without running `R::drop` while hardware can still observe it.
    route: Option<R>,
    setup: Option<R::Setup>,
    mac_runtime: &'runtime EmbassyMacIrqRuntime<M>,
    power_runtime: &'runtime EmbassyPowerIrqRuntime<M>,
}

impl<'runtime, R, M> Esp32s31MacInterruptEpoch<'runtime, R, M>
where
    R: MacInterruptRoute,
    M: RawMutex,
{
    pub const fn new(
        route: R,
        setup: R::Setup,
        mac_runtime: &'runtime EmbassyMacIrqRuntime<M>,
        power_runtime: &'runtime EmbassyPowerIrqRuntime<M>,
    ) -> Self {
        Self {
            route: Some(route),
            setup: Some(setup),
            mac_runtime,
            power_runtime,
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
    pub const fn mac_runtime(&self) -> &'runtime EmbassyMacIrqRuntime<M> {
        self.mac_runtime
    }

    /// Borrow the task-side capability for polling-only scan/auth phases.
    pub fn setup(&self) -> Result<&R::Setup, Esp32s31MacInterruptEpochStateError> {
        self.setup
            .as_ref()
            .ok_or(Esp32s31MacInterruptEpochStateError::Active)
    }

    pub fn activate(
        &mut self,
        platform: &R::Platform,
        event_mask: u32,
    ) -> Result<(), Esp32s31MacInterruptEpochActivateError<R::Error>> {
        let setup = self
            .setup
            .take()
            .ok_or(Esp32s31MacInterruptEpochActivateError::AlreadyActive)?;
        let route = self
            .route
            .as_mut()
            .expect("interrupt epoch always retains its route");
        match route.activate(platform, setup, event_mask) {
            Ok(()) => Ok(()),
            Err((error, setup)) => {
                self.setup = Some(setup);
                Err(Esp32s31MacInterruptEpochActivateError::Route(error))
            }
        }
    }

    pub fn quiesce(
        &mut self,
        platform: &R::Platform,
    ) -> Result<Esp32s31MacInterruptEpochDrain, Esp32s31MacInterruptEpochQuiesceError<R::Error>>
    {
        if self.setup.is_some() {
            return Err(Esp32s31MacInterruptEpochQuiesceError::AlreadyQuiesced);
        }
        let setup = self
            .route
            .as_mut()
            .expect("interrupt epoch always retains its route")
            .quiesce(platform)
            .map_err(Esp32s31MacInterruptEpochQuiesceError::Route)?;
        self.setup = Some(setup);
        Ok(Esp32s31MacInterruptEpochDrain {
            mac: self.mac_runtime.drain_pending(),
            power_events: self.power_runtime.drain_pending(),
        })
    }

    /// Recover every interrupt resource after the route returned its setup
    /// token and the executor publications were drained.
    ///
    /// An active epoch is returned unchanged. Consequently no caller can
    /// extract or drop an installed route through this API.
    pub fn try_into_inactive_parts(
        mut self,
    ) -> Result<
        (
            R,
            R::Setup,
            &'runtime EmbassyMacIrqRuntime<M>,
            &'runtime EmbassyPowerIrqRuntime<M>,
        ),
        Self,
    > {
        if self.is_active() {
            return Err(self);
        }
        let route = self.route.take().expect("inactive epoch retains its route");
        let setup = self
            .setup
            .take()
            .expect("inactive epoch retains its setup token");
        Ok((route, setup, self.mac_runtime, self.power_runtime))
    }
}

impl<R, M> Drop for Esp32s31MacInterruptEpoch<'_, R, M>
where
    R: MacInterruptRoute,
    M: RawMutex,
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
