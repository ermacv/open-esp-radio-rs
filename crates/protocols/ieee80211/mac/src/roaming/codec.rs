//! k/v errors adapt shared bounded slicing without a second parser.
pub(super) use crate::codec::{BodyReader, BodyWriter};

impl From<crate::codec::Truncated> for super::WireError {
    fn from(_: crate::codec::Truncated) -> Self {
        Self::Truncated
    }
}
