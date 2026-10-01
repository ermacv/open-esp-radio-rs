//! IEEE 802.11k/v Neighbor Report, RRM, BTM and selected WNM wire formats.
//!
//! The caller owns the management header, protection, sequence number and FCS.
//! Borrowed views validate complete bodies before exposing them; encoders check
//! every field and the output capacity before writing. Unknown information
//! elements and Neighbor Report subelements are retained byte for byte.
//!
//! Wire identities and optional-field order follow IEEE 802.11 and the
//! hostap project's `ieee802_11_defs.h`, `rrm.c` and `wnm_sta.c` (upstream
//! sources distributed in FreeBSD's `contrib/wpa`). Original Event/Diagnostic
//! formats use IEEE 802.11-2012 sections 8.4.2.69–72 and 10.23.2–3.

mod actions;
mod btm;
mod capabilities;
mod codec;
mod diagnostics;
mod dms;
mod elements;
mod events;
mod management;
mod measurement;
mod neighbor;
mod tclas;
mod tfs;
mod wnm;

pub use actions::*;
pub use btm::*;
pub use capabilities::*;
pub use diagnostics::*;
pub use dms::*;
pub use elements::*;
pub use events::*;
pub use management::*;
pub use measurement::*;
pub use neighbor::*;
pub use tclas::*;
pub use tfs::*;
pub use wnm::*;

pub use crate::management::{MAC_ADDRESS_LEN, MacAddress};
use codec::{BodyReader, BodyWriter};
// Capacity for octet-indexed syntax validation; this is not a protocol field.
const OCTET_VALUE_COUNT: usize = u8::MAX as usize + 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WireError {
    Truncated,
    WrongAction,
    ZeroDialogToken,
    ZeroMeasurementToken,
    DuplicateMeasurementToken(u8),
    MalformedElement,
    DuplicateElement(u8),
    InvalidElementLength(u8),
    ElementTooLong,
    SsidTooLong,
    InconsistentFields,
    OutputTooSmall { required: usize },
}

pub(super) fn header(bytes: &[u8], category: u8, action: u8, len: usize) -> Result<(), WireError> {
    if bytes.len() < len {
        return Err(WireError::Truncated);
    }
    if bytes[..DIALOG_TOKEN_OFFSET] != [category, action] {
        return Err(WireError::WrongAction);
    }
    if bytes[DIALOG_TOKEN_OFFSET] == 0 {
        return Err(WireError::ZeroDialogToken);
    }
    Ok(())
}

pub(super) fn output(output: &mut [u8], len: usize) -> Result<&mut [u8], WireError> {
    output
        .get_mut(..len)
        .ok_or(WireError::OutputTooSmall { required: len })
}

pub(super) fn token(token: u8) -> Result<(), WireError> {
    if token == 0 {
        Err(WireError::ZeroDialogToken)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests;
