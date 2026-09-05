//! Stable full-service Bluetooth interrupt composition for ESP32-S31.

use core::{
    cell::RefCell,
    sync::atomic::{AtomicBool, Ordering},
};

use embassy_sync::blocking_mutex::{Mutex, raw::CriticalSectionRawMutex};
use open_esp_radio_esp32s31_bluetooth::BluetoothControllerPublishedInterruptService;
use open_esp_radio_esp32s31_bluetooth_embassy::EmbassyBluetoothRuntimeWakers;
use open_esp_radio_esp32s31_radio_platform_esp_hal::{
    BoundEspHalBluetoothInterruptEpoch, EspHalBluetoothInterruptDisposition,
    EspHalBluetoothInterruptRouteError, EspHalBluetoothInterruptSource,
    EspHalBluetoothModemLpTimerStorageError, EspHalBluetoothSharedInterruptDispatchError,
    PublishedEspHalBluetoothInterruptOwners,
};
use static_cell::StaticCell;

use crate::interrupt_fault::DurableFirstFault;

type PublishedInterruptService =
    BluetoothControllerPublishedInterruptService<'static, PublishedEspHalBluetoothInterruptOwners>;
type RuntimeWakers = EmbassyBluetoothRuntimeWakers<CriticalSectionRawMutex>;

/// First fatal stable-storage error observed by a live Bluetooth ISR epoch.
///
/// The source is retained explicitly because primary and NRT share one lower
/// storage error type but have different hardware meaning.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31BluetoothInterruptFault {
    /// Source 124 could not service the published shared-register owner.
    Primary(EspHalBluetoothSharedInterruptDispatchError),
    /// Source 127 could not service the published timer owner.
    ModemLpTimer(EspHalBluetoothModemLpTimerStorageError),
    /// Source 133 could not service the published shared-register owner.
    NrtDefault(EspHalBluetoothSharedInterruptDispatchError),
}

struct Esp32s31BluetoothInterruptDispatch {
    service: &'static PublishedInterruptService,
    wakers: &'static RuntimeWakers,
    fault: DurableFirstFault<CriticalSectionRawMutex, Esp32s31BluetoothInterruptFault>,
}

impl Esp32s31BluetoothInterruptDispatch {
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
                        self.fault
                            .publish(Esp32s31BluetoothInterruptFault::Primary(error));
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
                            .publish(Esp32s31BluetoothInterruptFault::ModemLpTimer(error));
                        EspHalBluetoothInterruptDisposition::Quarantine
                    }
                }
            }
            EspHalBluetoothInterruptSource::NrtDefault => {
                match self.service.service_nrt_default_interrupt() {
                    Ok(_) => EspHalBluetoothInterruptDisposition::Serviced,
                    Err(error) => {
                        self.fault
                            .publish(Esp32s31BluetoothInterruptFault::NrtDefault(error));
                        EspHalBluetoothInterruptDisposition::Quarantine
                    }
                }
            }
        }
    }
}

static LIVE_INTERRUPT_DISPATCH: Mutex<
    CriticalSectionRawMutex,
    RefCell<Option<&'static Esp32s31BluetoothInterruptDispatch>>,
> = Mutex::new(RefCell::new(None));

fn dispatch_bluetooth_interrupt(
    source: EspHalBluetoothInterruptSource,
) -> EspHalBluetoothInterruptDisposition {
    let dispatch = LIVE_INTERRUPT_DISPATCH.lock(|slot| *slot.borrow());
    dispatch.map_or(
        EspHalBluetoothInterruptDisposition::Quarantine,
        |dispatch| dispatch.service(source),
    )
}

struct Esp32s31BluetoothInterruptRuntimeStorage {
    claimed: AtomicBool,
    service: StaticCell<PublishedInterruptService>,
    dispatch: StaticCell<Esp32s31BluetoothInterruptDispatch>,
}

impl Esp32s31BluetoothInterruptRuntimeStorage {
    const fn new() -> Self {
        Self {
            claimed: AtomicBool::new(false),
            service: StaticCell::new(),
            dispatch: StaticCell::new(),
        }
    }

    fn bind(
        &'static self,
        service: PublishedInterruptService,
        wakers: &'static RuntimeWakers,
    ) -> Result<Esp32s31BluetoothInterruptRuntime, Esp32s31BluetoothInterruptBindError> {
        if self
            .claimed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(Esp32s31BluetoothInterruptBindError::InUse);
        }

        let service = self.service.init(service);
        let dispatch = self.dispatch.init(Esp32s31BluetoothInterruptDispatch {
            service,
            wakers,
            fault: DurableFirstFault::new(),
        });

        let installed = LIVE_INTERRUPT_DISPATCH.lock(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.is_some() {
                false
            } else {
                *slot = Some(dispatch);
                true
            }
        });
        if !installed {
            return Err(Esp32s31BluetoothInterruptBindError::DispatcherInUse);
        }

        // The complete service, its executor notifications and its durable
        // fault sink are all reachable before the first CPU route can enter.
        let routes = service
            .storage()
            .bind_routes(dispatch_bluetooth_interrupt)
            .map_err(Esp32s31BluetoothInterruptBindError::Route)?;

        Ok(Esp32s31BluetoothInterruptRuntime { routes, dispatch })
    }
}

static PRODUCTION_INTERRUPT_RUNTIME: Esp32s31BluetoothInterruptRuntimeStorage =
    Esp32s31BluetoothInterruptRuntimeStorage::new();

/// Why final stable ISR composition could not activate all three CPU routes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31BluetoothInterruptBindError {
    /// The sole production interrupt composition was already consumed.
    InUse,
    /// Another dispatcher already occupies the process-wide callback slot.
    DispatcherInUse,
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
pub struct Esp32s31BluetoothInterruptRuntime {
    routes: BoundEspHalBluetoothInterruptEpoch<'static>,
    dispatch: &'static Esp32s31BluetoothInterruptDispatch,
}

/// Failed full-route shutdown retaining the unchanged live interrupt runtime.
#[must_use = "a rejected shutdown still owns all three live Bluetooth routes"]
pub struct Esp32s31BluetoothInterruptDisableFailure {
    error: EspHalBluetoothInterruptRouteError,
    runtime: Esp32s31BluetoothInterruptRuntime,
}

impl Esp32s31BluetoothInterruptDisableFailure {
    /// Exact all-route shutdown rejection.
    pub const fn error(&self) -> EspHalBluetoothInterruptRouteError {
        self.error
    }

    /// Recover the rejection and unchanged live runtime for a later retry.
    pub fn into_parts(
        self,
    ) -> (
        EspHalBluetoothInterruptRouteError,
        Esp32s31BluetoothInterruptRuntime,
    ) {
        (self.error, self.runtime)
    }
}

impl Esp32s31BluetoothInterruptRuntime {
    /// Observe the first fatal ISR storage error without consuming it.
    pub fn fault(&self) -> Option<Esp32s31BluetoothInterruptFault> {
        self.dispatch.fault.get()
    }

    /// Wait cancellation-safely for the first fatal ISR storage error.
    ///
    /// The fault remains stored after completion, so cancelling this future or
    /// polling it from a replacement outer-runner wait cannot lose the cause.
    pub async fn wait_fault(&self) -> Esp32s31BluetoothInterruptFault {
        self.dispatch.fault.wait().await
    }

    /// Disable the complete source-124/source-127/source-133 route set.
    ///
    /// A rejected ESP-HAL transition reconstructs this exact runtime, so a
    /// terminal quarantine can retain or retry shutdown without reminting any
    /// handler owner.
    pub fn disable(self) -> Result<(), Esp32s31BluetoothInterruptDisableFailure> {
        let Self { routes, dispatch } = self;
        match routes.disable() {
            Ok(()) => Ok(()),
            Err(failure) => {
                let (error, routes) = failure.into_parts();
                Err(Esp32s31BluetoothInterruptDisableFailure {
                    error,
                    runtime: Self { routes, dispatch },
                })
            }
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
) -> Result<Esp32s31BluetoothInterruptRuntime, Esp32s31BluetoothInterruptBindError> {
    PRODUCTION_INTERRUPT_RUNTIME.bind(service, wakers)
}
