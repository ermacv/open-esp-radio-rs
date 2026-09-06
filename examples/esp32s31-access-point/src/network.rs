//! Application stack setup and the small API difference between network contracts.
#[cfg(feature = "owned-network")]
mod owned;
#[cfg(feature = "owned-network")]
pub use owned::*;
#[cfg(feature = "upstream-network")]
mod upstream;
#[cfg(feature = "upstream-network")]
pub use upstream::*;

#[cfg(feature = "compat-network")]
mod compat;
#[cfg(feature = "compat-network")]
pub use compat::*;
