//! Architecture-neutral symbolic values, observable-effect IR and MMIO catalog.
//!
//! This crate deliberately contains no instruction decoder, physical register
//! file or target provider. Architecture backends produce these values and
//! providers may inspect them through the same stable vocabulary.

mod ir;
mod mmio;
mod standard_runtime;

pub use ir::*;
pub use mmio::{
    MmioAccessError, MmioAccessIdentity, MmioAccessKind, MmioMap, MmioRegion, Register,
    RegisterCatalog, reject_register_collisions,
};
pub use open_radio_vendor_contracts::*;
pub use standard_runtime::StandardMemoryFunction;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Message(String),

    #[error(transparent)]
    Xml(#[from] roxmltree::Error),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl From<String> for Error {
    fn from(value: String) -> Self {
        Self::Message(value)
    }
}

impl From<&str> for Error {
    fn from(value: &str) -> Self {
        Self::Message(value.to_owned())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn u32_literal(value: &str) -> Option<u32> {
    let value = value.trim();
    if let Some(hex) = value.strip_prefix("0x") {
        u32::from_str_radix(hex, 16).ok()
    } else {
        value.parse().ok()
    }
}
