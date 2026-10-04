#![no_std]
#![forbid(unsafe_code)]

//! Staged-boot layout of ESP32-S31-Function-CoreBoard-1.
//!
//! The bootstrap, the stage-two runtime linker scripts and the host image
//! packer and auditor share these definitions. [`memory`] owns the address map;
//! [`interrupts`] the interrupt contract the interrupt-stack gate checks;
//! [`stage_two`] owns the image header and
//! payload checksum that the bootstrap validates before handoff; [`zeroed`]
//! owns the input sections the boot zeroes. With the
//! `build` feature, [`build`] emits both to the linker scripts.

pub mod interrupts;
pub mod memory;
pub mod stage_two;
pub mod zeroed;

#[cfg(feature = "build")]
pub mod build;
