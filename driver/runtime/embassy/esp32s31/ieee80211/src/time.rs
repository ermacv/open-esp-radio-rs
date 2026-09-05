//! Embassy time bindings for the hardware ports used by this adapter.
//!
//! This PHY delay delegates directly to Embassy without imposing the
//! one-megahertz validation policy used by Bluetooth composition.

pub mod phy;
