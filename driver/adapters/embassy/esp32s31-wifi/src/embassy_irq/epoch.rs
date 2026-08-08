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
    route: R,
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
            route,
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
        match self.route.activate(platform, setup, event_mask) {
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
            .quiesce(platform)
            .map_err(Esp32s31MacInterruptEpochQuiesceError::Route)?;
        self.setup = Some(setup);
        Ok(Esp32s31MacInterruptEpochDrain {
            mac: self.mac_runtime.drain_pending(),
            power_events: self.power_runtime.drain_pending(),
        })
    }
}

impl<R, M> Drop for Esp32s31MacInterruptEpoch<'_, R, M>
where
    R: MacInterruptRoute,
    M: RawMutex,
{
    fn drop(&mut self) {
        if self.is_active() {
            panic!("active ESP32-S31 MAC interrupt epoch destroyed before quiescence");
        }
    }
}
