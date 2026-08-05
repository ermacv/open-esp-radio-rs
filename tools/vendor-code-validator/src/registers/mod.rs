//! Generated MMIO facts, reviewed overlays, and SVD materialization.

mod facts;
mod overlay;
mod overlay_parse;
#[cfg(test)]
mod overlay_tests;
mod svd;
#[cfg(test)]
mod svd_tests;

pub(crate) use facts::*;
pub(crate) use overlay::*;
pub(crate) use svd::*;
