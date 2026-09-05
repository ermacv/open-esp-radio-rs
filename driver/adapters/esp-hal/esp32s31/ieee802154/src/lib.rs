//! ESP-HAL CPU-route ownership for the ESP32-S31 IEEE 802.15.4 MAC IRQ.
//!
//! ESP32-S31's PAC does not yet name modem source 132, so this adapter uses the
//! pinned ESP-HAL raw-source hook. It keeps the source number, priority, bound
//! core, and process-wide route claim behind one affine owner. This adapter
//! retains the unique PAC interrupt-register owner in stable ISR storage;
//! the chip IRQ layer handles its status semantics, and Embassy receives
//! acknowledged event tokens without PAC handles.

#![no_std]
#![cfg(feature = "esp32s31")]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

use core::cell::{Cell, RefCell};

use critical_section::Mutex;
use esp_hal::{
    interrupt::{self, InterruptHandler, Priority},
    system::Cpu,
    time::Instant,
};
use open_esp_radio_esp32s31_ieee802154_irq::{
    IEEE802154_MAC_INTERRUPT_SOURCE, Ieee802154AcknowledgedInterruptSink,
    Ieee802154InterruptDisposition, handle_ieee802154_interrupt,
};
use open_esp_radio_esp32s31_ieee802154_runtime::Ieee802154MonotonicMicrosecondClock;
use open_esp_radio_esp32s31_pac::{
    Ieee802154InterruptRegisters, Ieee802154InterruptSetup, Ieee802154TaskRegisters,
};

const SOURCE: u16 = IEEE802154_MAC_INTERRUPT_SOURCE.number();
const ROUTE_PRIORITY: Priority = Priority::Priority1;

const _: () = assert!(SOURCE == 132);
const _: () = assert!(ROUTE_PRIORITY as u8 == 1);

/// Bind the ACK watchdog to ESP-HAL's one-microsecond monotonic clock.
///
/// The low 32 bits intentionally reproduce the wrapping time domain used by
/// the reviewed ESP-IDF IEEE 802.15.4 driver. [`esp_hal::init`] must have run
/// before the returned capability is sampled.
pub const fn monotonic_microsecond_clock() -> Ieee802154MonotonicMicrosecondClock {
    Ieee802154MonotonicMicrosecondClock::new(sample_monotonic_microseconds)
}

fn sample_monotonic_microseconds() -> u32 {
    Instant::now().duration_since_epoch().as_micros() as u32
}

static ROUTE_CLAIMED: Mutex<Cell<bool>> = Mutex::new(Cell::new(false));
static INTERRUPT_REGISTERS: Mutex<RefCell<Option<Ieee802154InterruptRegisters>>> =
    Mutex::new(RefCell::new(None));

/// Failure to create or quiesce the unique IEEE 802.15.4 CPU route.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspHalIeee802154InterruptRouteError {
    /// Another live route owner already controls modem source 132.
    AlreadyActive,
    /// The handler does not use the source-confirmed priority-one route.
    WrongPriority {
        /// Numeric ESP-HAL priority supplied by the handler.
        observed: u8,
    },
    /// Teardown was attempted from a CPU other than the binding CPU.
    WrongCore,
    /// The process-wide route claim and stable PAC storage disagree.
    StorageInvariant,
}

/// Active ESP-HAL route for modem source 132.
///
/// The owner is minted only after the handler has been bound and the CPU route
/// enabled. It must be disabled on the same core before PAC interrupt storage
/// is recovered or a later epoch can bind the source again.
#[must_use = "source 132 must be disabled before interrupt storage is recovered"]
pub struct BoundEspHalIeee802154InterruptRoute {
    core: Cpu,
}

/// Activate the peripheral interrupt epoch and bind modem source 132.
///
/// The consuming transition preserves the required order: install the closed
/// PAC event/abort baseline, acknowledge one exact stale W1C snapshot, publish
/// the unique interrupt-register owner in stable ISR storage, and only then
/// enable the CPU route. Priority is fixed to the public esp-radio handler's
/// `Priority1`.
///
/// On failure neither `setup` nor task-side PAC state is consumed.
pub fn activate(
    task: &mut Ieee802154TaskRegisters,
    setup: Ieee802154InterruptSetup,
    handler: InterruptHandler,
) -> Result<
    BoundEspHalIeee802154InterruptRoute,
    (
        EspHalIeee802154InterruptRouteError,
        Ieee802154InterruptSetup,
    ),
> {
    if let Err(error) = validate_priority(handler.priority()) {
        return Err((error, setup));
    }

    let acquired = critical_section::with(|critical_section| {
        let claimed = ROUTE_CLAIMED.borrow(critical_section);
        if claimed.get() {
            Err(EspHalIeee802154InterruptRouteError::AlreadyActive)
        } else if INTERRUPT_REGISTERS.borrow_ref(critical_section).is_some() {
            Err(EspHalIeee802154InterruptRouteError::StorageInvariant)
        } else {
            claimed.set(true);
            Ok(())
        }
    });
    if let Err(error) = acquired {
        return Err((error, setup));
    }

    let registers = setup.activate(task);
    critical_section::with(|critical_section| {
        *INTERRUPT_REGISTERS.borrow_ref_mut(critical_section) = Some(registers);
    });
    let core = Cpu::current();
    interrupt::bind_raw_handler(SOURCE, handler.handler().callback(), ROUTE_PRIORITY);
    Ok(BoundEspHalIeee802154InterruptRoute { core })
}

/// Service one complete source-132 interrupt from the stable PAC owner.
///
/// The application handler bound through [`activate`] calls this function with
/// its static Embassy handoff. `None` means the route is inactive; otherwise
/// the returned disposition reports whether a nonzero affine snapshot was
/// acknowledged and posted.
#[inline]
pub fn service_interrupt<Sink: Ieee802154AcknowledgedInterruptSink + ?Sized>(
    sink: &Sink,
) -> Option<Ieee802154InterruptDisposition> {
    critical_section::with(|critical_section| {
        INTERRUPT_REGISTERS
            .borrow_ref_mut(critical_section)
            .as_mut()
            .map(|registers| handle_ieee802154_interrupt(registers, sink))
    })
}

impl BoundEspHalIeee802154InterruptRoute {
    /// Disable source 132 and return inactive PAC interrupt ownership.
    ///
    /// The route is disabled before stable ISR storage is recovered. The PAC
    /// teardown then writes zero to event and abort enables and consumes one
    /// final exact W1C snapshot before the process-wide claim is released.
    /// On a core mismatch no state changes and the intact route owner is
    /// returned for teardown on the binding CPU.
    pub fn quiesce(
        self,
        task: &mut Ieee802154TaskRegisters,
    ) -> Result<Ieee802154InterruptSetup, (EspHalIeee802154InterruptRouteError, Self)> {
        if Cpu::current() != self.core {
            return Err((EspHalIeee802154InterruptRouteError::WrongCore, self));
        }

        interrupt::disable_raw(self.core, SOURCE);
        let registers = critical_section::with(|critical_section| {
            INTERRUPT_REGISTERS.borrow_ref_mut(critical_section).take()
        })
        .expect("an active source-132 route must retain its PAC interrupt owner");
        let setup = registers.deactivate(task);
        critical_section::with(|critical_section| {
            ROUTE_CLAIMED.borrow(critical_section).set(false);
        });
        Ok(setup)
    }
}

fn validate_priority(priority: Priority) -> Result<(), EspHalIeee802154InterruptRouteError> {
    if priority == ROUTE_PRIORITY {
        Ok(())
    } else {
        Err(EspHalIeee802154InterruptRouteError::WrongPriority {
            observed: priority as u8,
        })
    }
}
