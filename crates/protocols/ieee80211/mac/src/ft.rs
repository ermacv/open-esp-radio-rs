//! IEEE 802.11r formats, independent of keys, association policy and IO.
//!
//! SOURCE: IEEE 802.11r-2008; hostap `src/common/wpa_common.{c,h}`,
//! `src/rsn_supp/wpa_ft.c` and `src/ap/wpa_auth_ft.c`.
//! The SHA-256 FT-PSK, FT-802.1X and FT-SAE layouts are supported; later
//! SHA-384, SAE-EXT-KEY and multi-link layouts are distinct protocols.

mod elements;
mod frames;
pub use crate::security::rsn::FtAkm;
pub use elements::*;
pub use frames::*;

use crate::management::elements::{ELEMENT_HEADER_LEN, ElementError, Elements};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WireError {
    Elements(ElementError),
    MissingElement(u8),
    InvalidElement(u8),
    UnsupportedSuite,
    WrongFrame,
    InconsistentFields,
    OutputTooSmall { required: usize },
}

impl From<ElementError> for WireError {
    fn from(error: ElementError) -> Self {
        Self::Elements(error)
    }
}

fn element(bytes: &[u8], id: u8) -> Result<crate::management::elements::Element<'_>, WireError> {
    let elements = Elements::parse(bytes)?;
    let mut iter = elements.iter();
    let value = iter.next().ok_or(WireError::MissingElement(id))?;
    if value.id != id || iter.next().is_some() {
        return Err(WireError::InvalidElement(id));
    }
    Ok(value)
}

fn output(bytes: &mut [u8], required: usize) -> Result<&mut [u8], WireError> {
    bytes
        .get_mut(..required)
        .ok_or(WireError::OutputTooSmall { required })
}

#[cfg(test)]
mod tests;
