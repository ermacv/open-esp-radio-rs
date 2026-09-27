//! ESP-HAL CPU-route ownership and time for the ESP32-S31 IEEE 802.15.4 MAC.
//!
//! The typed esp-hal interrupt identifies the IEEE 802.15.4 MAC route. This
//! adapter keeps its priority, bound core and process-wide claim behind one
//! affine owner, and supplies the microsecond clock the MAC engine reads and
//! the random words of CSMA-CA backoffs.
//! The MAC owners live in the IEEE 802.15.4 runtime: the handler bound here
//! calls the runtime's interrupt entry.
//!
//! Bring-up order: activate the HAL interrupt owner, install the runtime,
//! then `bind`. Teardown reverses it: `BoundEspHalIeee802154InterruptRoute::quiesce`,
//! uninstall the runtime, then deactivate the HAL interrupt owner. This is
//! ESP-IDF's order: `ieee802154_mac_init` allocates the route last with
//! `esp_intr_alloc(132, 0)`, the lowest free level of 1 to 3 on the calling
//! core, and `ieee802154_mac_deinit` frees it first.

#![no_std]
#![cfg(feature = "esp32s31")]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

use core::cell::Cell;

use critical_section::Mutex;
use esp_hal::{
    interrupt::{self, InterruptHandler, Priority},
    peripherals::Interrupt,
    rng::Rng,
    system::Cpu,
    time::Instant,
};

const SOURCE: Interrupt = Interrupt::IEEE802154;
const ROUTE_PRIORITY: Priority = Priority::Priority1;

/// ESP-HAL's one-microsecond monotonic clock (`esp_timer_get_time`).
///
/// `esp_hal::init` must have run before the clock is sampled.
pub fn now_micros() -> u64 {
    Instant::now().duration_since_epoch().as_micros()
}

/// A random word from the hardware generator, for CSMA-CA backoffs where
/// ESP-IDF's OpenThread draws from its non-cryptographic generator.
pub fn random() -> u32 {
    Rng::new().random()
}

static ROUTE_CLAIMED: Mutex<Cell<bool>> = Mutex::new(Cell::new(false));

/// Failure to create or quiesce the unique IEEE 802.15.4 CPU route.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspHalIeee802154InterruptRouteError {
    /// Another live route owner already controls modem source 132.
    AlreadyActive,
    /// The handler does not use the vendor's priority-one route.
    WrongPriority {
        /// Numeric ESP-HAL priority supplied by the handler.
        observed: u8,
    },
    /// Teardown was attempted from a CPU other than the binding CPU.
    WrongCore,
}

/// Active ESP-HAL route for modem source 132.
///
/// It must be disabled on the binding core before the runtime returns the
/// MAC owners or a later epoch binds the source again.
#[must_use = "source 132 must be disabled before the MAC owners are recovered"]
pub struct BoundEspHalIeee802154InterruptRoute {
    core: Cpu,
}

/// Bind `handler` to modem source 132 at the vendor's `Priority1`.
///
/// # Errors
///
/// The handler has another priority, or a route is already bound.
pub fn bind(
    handler: InterruptHandler,
) -> Result<BoundEspHalIeee802154InterruptRoute, EspHalIeee802154InterruptRouteError> {
    if handler.priority() != ROUTE_PRIORITY {
        return Err(EspHalIeee802154InterruptRouteError::WrongPriority {
            observed: handler.priority() as u8,
        });
    }
    critical_section::with(|critical_section| {
        let claimed = ROUTE_CLAIMED.borrow(critical_section);
        if claimed.get() {
            return Err(EspHalIeee802154InterruptRouteError::AlreadyActive);
        }
        claimed.set(true);
        Ok(())
    })?;
    let core = Cpu::current();
    interrupt::bind_handler(SOURCE, handler);
    Ok(BoundEspHalIeee802154InterruptRoute { core })
}

impl BoundEspHalIeee802154InterruptRoute {
    /// Disable source 132 on the binding core and release the claim.
    ///
    /// # Errors
    ///
    /// On another core nothing changes and the route owner is returned.
    pub fn quiesce(self) -> Result<(), (EspHalIeee802154InterruptRouteError, Self)> {
        if Cpu::current() != self.core {
            return Err((EspHalIeee802154InterruptRouteError::WrongCore, self));
        }
        interrupt::disable(self.core, SOURCE);
        critical_section::with(|critical_section| {
            ROUTE_CLAIMED.borrow(critical_section).set(false);
        });
        Ok(())
    }
}
