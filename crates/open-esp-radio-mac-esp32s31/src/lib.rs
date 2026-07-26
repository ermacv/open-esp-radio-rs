#![no_std]

//! Source-owned ESP32-S31 Wi-Fi MAC building blocks.
//!
//! This crate intentionally contains no ESP-IDF ABI, vendor archive, allocator,
//! executor, or `esp-hal` dependency. Target code supplies volatile MMIO and
//! owns the interrupt binding; the state machines here stay host-testable.

pub mod descriptor;
pub mod irq;
pub mod registers;
pub mod rx;
pub mod tx;
