#![no_std]
#![forbid(unsafe_code)]

#[cfg(test)]
extern crate std;

#[cfg(feature = "wifi-embassy")]
pub mod runtime;
#[cfg(feature = "wifi")]
pub mod wifi;
