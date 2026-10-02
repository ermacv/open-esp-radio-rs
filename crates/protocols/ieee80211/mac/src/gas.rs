//! IEEE 802.11 GAS wire formats, independent of the advertised query protocol.
//!
//! SOURCE: IEEE 802.11-2012 8.4.2.95 and 8.5.8.12–15; hostap
//! `src/common/gas.c` and `wpa_supplicant/gas_query.c`.
//! Management headers, protection, channel admission and FCS are external.

mod advertisement;
mod frames;
pub use advertisement::*;
pub use frames::*;

use crate::codec::{BodyReader, BodyWriter, Truncated};
use crate::management::elements::ElementError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WireError {
    Truncated,
    Elements(ElementError),
    WrongAction,
    InvalidAdvertisement,
    InvalidFragment,
    InconsistentFields,
    PayloadTooLong,
    OutputTooSmall { required: usize },
}
impl From<Truncated> for WireError {
    fn from(_: Truncated) -> Self {
        Self::Truncated
    }
}
impl From<ElementError> for WireError {
    fn from(error: ElementError) -> Self {
        Self::Elements(error)
    }
}

fn output(bytes: &mut [u8], required: usize) -> Result<&mut [u8], WireError> {
    bytes
        .get_mut(..required)
        .ok_or(WireError::OutputTooSmall { required })
}

#[cfg(test)]
mod tests;
