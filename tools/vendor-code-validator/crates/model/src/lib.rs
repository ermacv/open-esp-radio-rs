//! Architecture-neutral symbolic values, observable-effect IR and MMIO catalog.
//!
//! This crate deliberately contains no instruction decoder, physical register
//! file or platform harness. Architecture backends produce these values and
//! platform harnesses may inspect them through the same stable vocabulary.

mod ir;
mod mmio;

pub use ir::*;
pub use mmio::{MmioRegisterMap, Register, Window, reject_register_collisions};
pub use open_radio_vendor_validator_core::*;

pub type Error = Box<dyn std::error::Error>;
pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn u32_literal(value: &str) -> Option<u32> {
    let value = value.trim();
    if let Some(hex) = value.strip_prefix("0x") {
        u32::from_str_radix(hex, 16).ok()
    } else {
        value.parse().ok()
    }
}
