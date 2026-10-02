//! ANQP element syntax, separate from GAS transport and credential selection.
//!
//! SOURCE: IEEE 802.11-2012 8.4.4; hostap `src/ap/gas_serv.c`,
//! `wpa_supplicant/interworking.c`, `wpa_supplicant/hs20_supplicant.c`.
//! Location reports and external 3GPP payload versions retain their exact
//! representations; their producers own geographic/network facts.

mod envelope;
pub mod hs20;
pub mod realm;
mod values;
pub use envelope::*;
pub use values::*;

use crate::codec::{BodyReader, BodyWriter, Truncated};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WireError {
    Truncated,
    InvalidLength,
    InvalidValue,
    InvalidText,
    UnorderedIds,
    Duplicate(InfoId),
    ElementTooLong,
    OutputTooSmall { required: usize },
}
impl From<Truncated> for WireError {
    fn from(_: Truncated) -> Self {
        Self::Truncated
    }
}
fn output(bytes: &mut [u8], required: usize) -> Result<&mut [u8], WireError> {
    bytes
        .get_mut(..required)
        .ok_or(WireError::OutputTooSmall { required })
}
fn utf8(bytes: &[u8]) -> Result<&str, WireError> {
    core::str::from_utf8(bytes).map_err(|_| WireError::InvalidText)
}
fn short_bytes<'a>(fields: &mut BodyReader<'a>) -> Result<&'a [u8], WireError> {
    let len = usize::from(fields.u8()?);
    Ok(fields.take(len)?)
}
fn long_bytes<'a>(fields: &mut BodyReader<'a>) -> Result<&'a [u8], WireError> {
    let len = usize::from(u16::from_le_bytes(fields.array()?));
    Ok(fields.take(len)?)
}
fn end(fields: &BodyReader<'_>) -> Result<(), WireError> {
    if fields.remaining().is_empty() {
        Ok(())
    } else {
        Err(WireError::InvalidLength)
    }
}

#[cfg(test)]
mod tests;
