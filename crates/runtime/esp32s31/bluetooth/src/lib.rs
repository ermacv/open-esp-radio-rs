#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! Executor-independent Bluetooth LE radio runtime for the ESP32-S31.
//!
//! [`BluetoothRuntime`] holds the radio role — the portable radio event
//! contract over the scheduler executor — together with its hardware behind
//! one async lock over a caller-chosen raw mutex.
//!
//! - [`BluetoothRuntime::install`] publishes the global receive chains, takes
//!   the first controller-time sample and installs the radio. It returns the
//!   radio's two handles: the [`BluetoothPort`] for the one outcome consumer
//!   and the [`BluetoothControl`] for the composition, whose uninstall
//!   consumes both. Neither exists without an installed radio, so no port
//!   call is refused as not installed.
//! - [`BluetoothRuntime::run`] drives the scheduler on one task: it reports
//!   finished events, inserts admitted events, starts the idle scheduler and
//!   carries list transactions through their hardware waits. A scheduler
//!   interrupt, a request or a short recheck delay resumes it.
//! - The port's submission admits one request against a fresh
//!   controller-time sample while its lifecycle enables it.
//! - [`BluetoothControl::quiesce`] quiesces the port, stops the scheduler,
//!   lends the Bluetooth quiescence proof to shared-PHY maintenance, resumes
//!   the scheduler and enables the port again.
//!
//! The port is the [`oer_bluetooth_radio::LeRadioPort`] of the portable
//! Controller service loop: a refused request is an answer to the Controller
//! core, and a fault poisons the port with its [`BluetoothFault`].
//! Submission and the clock stay asynchronous because each takes a fresh
//! controller-time latch, which completes only after the hardware latched
//! the time, and shares the lock the runner holds across scheduler stops.
//! [`BluetoothRuntime::capabilities`] states LE 1M roles and hardware Link
//! Layer acknowledgement: the hardware keeps SN and NESN, retransmits and
//! answers with empty PDUs.
//!
//! Outcomes leave the lock as owned values through a bounded queue that any
//! executor may await through the port's event stream; each admitted event
//! reserves the slots of its terminal outcomes. The platform
//! calls [`BluetoothRuntime::on_scheduler_wake`] when its scheduler interrupt
//! publishes a wake for the worker.
//!
//! [`BluetoothRadioHardware`] names every hardware operation; on the chip,
//! `LiveBluetoothHardware` joins the powered task endpoint of one Controller
//! epoch with the platform's interrupt-owner storage, and `run_modem_timer`
//! drives the source-127 timer task beside the radio.

#[cfg(test)]
extern crate std;

#[cfg(any(target_arch = "riscv32", test))]
mod hardware;
#[cfg(target_arch = "riscv32")]
mod modem_timer;
mod outcome;
#[cfg(any(target_arch = "riscv32", test))]
mod port;
#[cfg(any(target_arch = "riscv32", test))]
mod runtime;

#[cfg(target_arch = "riscv32")]
pub use hardware::LiveBluetoothHardware;
#[cfg(any(target_arch = "riscv32", test))]
pub use hardware::{BluetoothRadioHardware, RxChainPublicationError};
#[cfg(target_arch = "riscv32")]
pub use modem_timer::{ModemTimerFault, run_modem_timer, settle_modem_timer};
pub use outcome::{BluetoothOutcome, BluetoothReceivedPdu, MAX_PDU_BYTES};
#[cfg(any(target_arch = "riscv32", test))]
pub use port::{BluetoothControl, BluetoothPort};
#[cfg(any(target_arch = "riscv32", test))]
pub use runtime::{
    BluetoothFault, BluetoothInstallError, BluetoothRuntime, BluetoothRuntimeFault,
    BluetoothTimeError, HARDWARE_RECHECK, TIME_REFRESH,
};

#[cfg(test)]
mod tests;
