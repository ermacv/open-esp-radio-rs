# Unsafe boundary

Driver policy, protocols, RF/PHY state machines, register transactions and
public lifecycle code are safe Rust and forbid unsafe code.

Audited exceptions:

- `chips/esp32s31/pac`: generated volatile MMIO access;
- `common/dma`: stable-address and exclusive DMA-memory proofs;
- `chips/esp32s31/wifi/dma`: descriptor layout and live ring publication;
- `adapters/embassy/esp32s31-platform`: executor/time ABI and linker sections;
- concrete ESP32-S31 integration ISR declarations: linker placement only;
- the `esp-hal` singleton binding: conversion of owned HAL peripherals into
  the private register backend.

Handwritten exception crates use `#![deny(unsafe_code)]` and reopen the lint on
the smallest audited item. Upper crates must not expose raw pointers, unchecked
lifetimes or interrupt/DMA ownership.

An active owner is never converted to reusable storage by `Drop`. Normal stop
is asynchronous and proves IRQ, RX, TX and queue quiescence. An abnormal drop
retains or quarantines the static owner and marks the runner faulted; it does
not panic, reset hardware or claim success. Since safe Rust cannot prohibit
`mem::forget`, leaked handles are harmless leaks, not ownership release.

Run `tools/audit-driver-safety.sh` from the repository root.
