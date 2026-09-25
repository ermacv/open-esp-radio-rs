//! Stable full-service Bluetooth interrupt composition for ESP32-S31.

use core::future::poll_fn;

use crate::{
    interrupt_fault::DurableFirstFault,
    interrupt_publication::{InterruptPublication, InterruptPublicationSlot},
};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;

use oer_esp32s31_bluetooth::controller::ControllerPublishedInterruptService;

use oer_esp32s31_bluetooth_runtime::notification::RuntimeNotifications;

use oer_esp32s31_radio_platform_esp_hal::{
    BoundEspHalBluetoothInterruptEpoch, EspHalBluetoothInterruptDisposition,
    EspHalBluetoothInterruptRouteError, EspHalBluetoothInterruptSource,
    EspHalBluetoothModemLpTimerStorageError, EspHalBluetoothSharedInterruptDispatchError,
    PublishedEspHalBluetoothInterruptOwners,
};

type PublishedInterruptService =
    ControllerPublishedInterruptService<'static, PublishedEspHalBluetoothInterruptOwners>;
type RuntimeWakers = RuntimeNotifications<CriticalSectionRawMutex>;

/// First fatal stable-storage error observed by a live Bluetooth ISR epoch.
///
/// The source is retained explicitly because primary and NRT share one lower
/// storage error type but have different hardware meaning.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothInterruptFault {
    /// Source 124 could not service the published shared-register owner.
    Primary(EspHalBluetoothSharedInterruptDispatchError),
    /// Source 127 could not service the published timer owner.
    ModemLpTimer(EspHalBluetoothModemLpTimerStorageError),
    /// Source 133 could not service the published shared-register owner.
    NrtDefault(EspHalBluetoothSharedInterruptDispatchError),
}

struct BluetoothInterruptDispatch {
    service: PublishedInterruptService,
    wakers: &'static RuntimeWakers,
    fault: DurableFirstFault<CriticalSectionRawMutex, BluetoothInterruptFault>,
}

impl BluetoothInterruptDispatch {
    fn service(
        &self,
        source: EspHalBluetoothInterruptSource,
    ) -> EspHalBluetoothInterruptDisposition {
        match source {
            EspHalBluetoothInterruptSource::Primary => {
                match self.service.service_primary_interrupt() {
                    Ok(step) => {
                        let _ = self.wakers.notify_primary_service(&step);
                        EspHalBluetoothInterruptDisposition::Serviced
                    }
                    Err(error) => {
                        self.fault.publish(BluetoothInterruptFault::Primary(error));
                        EspHalBluetoothInterruptDisposition::Quarantine
                    }
                }
            }
            EspHalBluetoothInterruptSource::ModemLpTimer => {
                match self.service.service_modem_lp_timer_interrupt() {
                    Ok(step) => {
                        let _ = self.wakers.modem_timer().notify_modem_timer_service(step);
                        EspHalBluetoothInterruptDisposition::Serviced
                    }
                    Err(error) => {
                        self.fault
                            .publish(BluetoothInterruptFault::ModemLpTimer(error));
                        EspHalBluetoothInterruptDisposition::Quarantine
                    }
                }
            }
            EspHalBluetoothInterruptSource::NrtDefault => {
                match self.service.service_nrt_default_interrupt() {
                    Ok(_) => EspHalBluetoothInterruptDisposition::Serviced,
                    Err(error) => {
                        self.fault
                            .publish(BluetoothInterruptFault::NrtDefault(error));
                        EspHalBluetoothInterruptDisposition::Quarantine
                    }
                }
            }
        }
    }
}

static INTERRUPT_PUBLICATION: InterruptPublicationSlot<
    CriticalSectionRawMutex,
    BluetoothInterruptDispatch,
> = InterruptPublicationSlot::new();

type LivePublication = InterruptPublication<
    'static,
    CriticalSectionRawMutex,
    BluetoothInterruptDispatch,
    BoundEspHalBluetoothInterruptEpoch<'static>,
>;

fn dispatch_bluetooth_interrupt(
    source: EspHalBluetoothInterruptSource,
) -> EspHalBluetoothInterruptDisposition {
    INTERRUPT_PUBLICATION.with(|dispatch| {
        dispatch.map_or(
            EspHalBluetoothInterruptDisposition::Quarantine,
            |dispatch| dispatch.service(source),
        )
    })
}

/// Why final stable ISR composition could not activate all three CPU routes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothInterruptBindError {
    /// The production interrupt publication is still occupied.
    InUse,
    /// ESP-HAL rejected the complete three-route activation.
    Route(EspHalBluetoothInterruptRouteError),
}

/// Affine owner of one fully serviced ESP32-S31 Bluetooth interrupt epoch.
///
/// The embedded route epoch keeps all three handlers live. The stable dispatch
/// performs the complete chip service before publishing the matching Embassy
/// notification, while the first storage error remains durably observable by
/// the sole outer Controller runner.
#[must_use = "the live Bluetooth interrupt epoch must remain owned by its Controller runner"]
pub struct BluetoothInterruptRuntime {
    publication: LivePublication,
}

/// Rejected activation returns the unpublished service and its notifications.
#[must_use = "a rejected bind retains the exact interrupt service"]
pub struct BluetoothInterruptBindFailure {
    error: BluetoothInterruptBindError,
    disabled: BluetoothInterruptDisabled,
}

impl BluetoothInterruptBindFailure {
    /// Exact rejection before any new live route ownership was returned.
    pub const fn error(&self) -> BluetoothInterruptBindError {
        self.error
    }

    /// Recover the inactive service for retention or an explicit retry.
    pub fn into_parts(self) -> (BluetoothInterruptBindError, BluetoothInterruptDisabled) {
        (self.error, self.disabled)
    }
}

/// Complete service after its three CPU routes are inactive and unpublished.
///
/// This is an IRQ boundary only. BTBB, DMA, modem-timer work and HCI may still
/// be live; this owner does not authorize PHY release or cold-owner recovery.
/// A sticky ISR fault travels with the service through an explicit rebind.
#[must_use = "retain the disabled service until complete Controller teardown"]
pub struct BluetoothInterruptDisabled {
    dispatch: BluetoothInterruptDispatch,
}

impl BluetoothInterruptDisabled {
    /// Controller retirement supplies the drained task/HCI/timer boundary.
    pub(crate) fn retire_registers(
        &self,
    ) -> Result<
        oer_esp32s31_radio_platform_esp_hal::RetiredEspHalBluetoothInterruptRegisters,
        oer_esp32s31_radio_platform_esp_hal::EspHalBluetoothInterruptRetirementError,
    > {
        self.dispatch
            .service
            .storage()
            .retire_interrupt_registers_after_routes_disabled()
    }

    /// First fault retained from the preceding live route epoch.
    pub fn fault(&self) -> Option<BluetoothInterruptFault> {
        self.dispatch.fault.get()
    }

    /// Reactivate the same service without clearing its first-fault evidence.
    /// Rejection returns this exact inactive owner for retention or retry.
    pub fn bind(self) -> Result<BluetoothInterruptRuntime, BluetoothInterruptBindFailure> {
        let storage = self.dispatch.service.storage();
        match INTERRUPT_PUBLICATION.bind(self.dispatch, || {
            storage.bind_routes(dispatch_bluetooth_interrupt)
        }) {
            Ok(publication) => Ok(BluetoothInterruptRuntime { publication }),
            Err((error, dispatch)) => Err(BluetoothInterruptBindFailure {
                error: error.map_or(
                    BluetoothInterruptBindError::InUse,
                    BluetoothInterruptBindError::Route,
                ),
                disabled: Self { dispatch },
            }),
        }
    }
}

/// Failed full-route shutdown retaining the unchanged live interrupt runtime.
#[must_use = "a rejected shutdown still owns all three live Bluetooth routes"]
pub struct BluetoothInterruptDisableFailure {
    error: EspHalBluetoothInterruptRouteError,
    runtime: BluetoothInterruptRuntime,
}

impl BluetoothInterruptDisableFailure {
    /// Exact all-route shutdown rejection.
    pub const fn error(&self) -> EspHalBluetoothInterruptRouteError {
        self.error
    }

    /// Recover the rejection and unchanged live runtime for a later retry.
    pub fn into_parts(
        self,
    ) -> (
        EspHalBluetoothInterruptRouteError,
        BluetoothInterruptRuntime,
    ) {
        (self.error, self.runtime)
    }
}

impl BluetoothInterruptRuntime {
    /// Observe the first fatal ISR storage error without consuming it.
    pub fn fault(&self) -> Option<BluetoothInterruptFault> {
        self.publication.with(|dispatch| dispatch.fault.get())
    }

    /// Wait cancellation-safely for the first fatal ISR storage error.
    ///
    /// The fault remains stored after completion, so cancelling this future or
    /// polling it from a replacement outer-runner wait cannot lose the cause.
    pub async fn wait_fault(&self) -> BluetoothInterruptFault {
        poll_fn(|cx| {
            self.publication
                .with(|dispatch| dispatch.fault.poll_wait(cx))
        })
        .await
    }

    /// Disable the complete source-124/source-127/source-133 route set.
    ///
    /// A rejected ESP-HAL transition reconstructs this exact runtime, so a
    /// terminal quarantine can retain or retry shutdown without reminting any
    /// handler owner. Success removes the publication only after same-core
    /// route shutdown; no reference into the replaceable slot escapes the ISR.
    pub fn disable(self) -> Result<BluetoothInterruptDisabled, BluetoothInterruptDisableFailure> {
        match self
            .publication
            .disable(|routes| routes.disable().map_err(|failure| failure.into_parts()))
        {
            Ok(dispatch) => Ok(BluetoothInterruptDisabled { dispatch }),
            Err((error, publication)) => Err(BluetoothInterruptDisableFailure {
                error,
                runtime: Self { publication },
            }),
        }
    }
}

/// Materialize the sole production ISR service and then activate its routes.
///
/// The service and wakers must already represent the same final Controller
/// runtime split. Stable dispatch publication precedes route binding; no IRQ
/// can observe a route without the complete chip-service/Embassy bridge.
pub fn bind_production_bluetooth_interrupt_runtime(
    service: PublishedInterruptService,
    wakers: &'static RuntimeWakers,
) -> Result<BluetoothInterruptRuntime, BluetoothInterruptBindFailure> {
    BluetoothInterruptDisabled {
        dispatch: BluetoothInterruptDispatch {
            service,
            wakers,
            fault: DurableFirstFault::new(),
        },
    }
    .bind()
}
