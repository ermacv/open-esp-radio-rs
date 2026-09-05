//! HCI command codecs, classification, response values and affine response ordering.
//!
//! Transport and resource epochs stay in their own owners. These modules decode
//! semantic commands and retain command authority until the matching response
//! is published; hardware execution remains the caller's responsibility.

pub(super) mod advertising;
pub(super) mod classification;
pub(super) mod dtm;
pub(super) mod order;
pub(super) mod response;
pub(super) mod scanning;
