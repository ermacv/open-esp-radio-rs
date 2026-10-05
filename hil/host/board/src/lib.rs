//! Board I/O: everything the host does to a board of the stand.
//!
//! - [`ports`]: the attached USB serial ports and a board's port by its MAC
//!   (its `/dev/serial/by-id` link);
//! - [`flash`]: the one flash writer, an image bundle's segments through
//!   one espflash connection for the chip its profile names, with retries,
//!   skipping unchanged segments and the OTA selection last;
//! - [`Board`]: a board of the stand file with its write, its start (the
//!   chip profile's `[flash] start`), its reset ladder and its power;
//! - [`reset`]: the USB Serial/JTAG openers and resets and the one ladder
//!   ([`reset::climb`]) every recovery and `cargo hil board reset` take;
//! - [`openocd`]: the one OpenOCD (locate, reset, registers, program);
//! - [`power`]: hub port power through `uhubctl` and its one report parser;
//! - [`console`]: the one serial line reader and bounded captures.
//!
//! It encodes nothing (`oer-image` builds every bundle), leases nothing
//! (the arbiter does) and journals nothing (the board journal does); the
//! flash operation (`oer-hil-flash`) puts them together.
#![forbid(unsafe_code)]

mod board;
pub mod console;
pub mod flash;
pub mod openocd;
pub mod ports;
pub mod power;
pub mod reset;

pub use board::{Board, Via};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
