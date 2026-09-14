#![no_std]
#![forbid(unsafe_code)]

//! Portable radio service policy and application-side ownership states.
//!
//! This crate defines requests, validates role combinations, and moves an
//! application between idle and active Wi-Fi typestates. It does not own PAC
//! registers, DMA descriptors, interrupt routes, clocks, an executor, or a
//! network stack. A chip composition implements the service port and retains
//! those resources in its actor; applications normally enter through that
//! composition rather than constructing this crate's internal machinery.
//!
//! With `wifi`, start at `wifi::WifiIdle`. A successful role start consumes
//! the idle capability, and only a successful stop returns a reusable one.
//! Rejected preparation returns the unchanged request and idle capability;
//! a fault after materialization does not. With `wifi-embassy`,
//! `runtime::embassy` supplies the one-command mailbox used by the concrete
//! ESP32-S31 runner.
//!
//! # Command and data planes
//!
//! The service port transports owned control requests and value reports. It
//! never transports hardware owners. Frames follow separate adapter and
//! stable-memory paths, so command completion alone says nothing about DMA
//! completion or packet-buffer reuse.
//!
//! # Cancellation
//!
//! Dropping an Embassy client future stops waiting; it does not retract a
//! command already published to the actor. The port drains that command's
//! response before publishing another command. Public role typestates are
//! affine, so callers cannot interpret a dropped start/stop wait as recovery
//! of the consumed capability.
//!
//! The ESP32-S31 application examples under `examples/` are the buildable
//! consumers. The concrete lifecycle, including hardware quarantine, is
//! documented by `oer-esp32s31-embassy-wifi`.

#[cfg(test)]
extern crate std;

#[cfg(feature = "wifi-embassy")]
pub mod runtime;
#[cfg(feature = "wifi")]
pub mod wifi;
