#![no_std]
#![forbid(unsafe_code)]

//! Staged-boot layout of ESP32-S31-Function-CoreBoard-1.
//!
//! The bootstrap, the stage-two runtime linker scripts and the host image
//! packer and auditor share these definitions. [`memory`] owns the address map
//! and the runtime placement profiles; [`stage_two`] owns the image header and
//! payload checksum that the bootstrap validates before handoff. With the
//! `build` feature, [`build`] emits both to the linker scripts.

pub mod memory;
pub mod stage_two;

#[cfg(feature = "build")]
pub mod build;
