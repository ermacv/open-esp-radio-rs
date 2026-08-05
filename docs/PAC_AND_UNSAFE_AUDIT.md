# PAC, MMIO and unsafe ownership audit

Verified against the workspace on 2026-07-31.

This document records the current ownership boundary. It is deliberately not
a chronological migration report. The completed PAC migration narrative is
retained as a [dated archive snapshot](archive/migration/2026-07-27-pac-and-unsafe-audit.md).

## Register ownership

`svd/esp32s31-radio.svd` is the editable source for undocumented radio
registers in the `0x2010_0000..0x201f_ffff` decode window. `cargo pac-gen`
generates `open-esp-radio-esp32s31-pac`; `cargo pac-gen --check` verifies that
the checked-in generated crate is reproducible and that every described span
fits the permitted MMIO window.

`open-esp-radio-esp32s31-registers::RadioRegisters` privately owns the generated
radio singleton and exposes finite semantic operations. The official
`esp-hal` PAC remains the sole register owner for chip-level dependencies such
as `MODEM_SYSCON`, `MODEM_LPCON`, `HP_SYS_CLKRST`, `PMU`, `LP_AON_CLK_RST`,
`LP_PERI`, `LP_TSENS`, and `I2C_ANA_MST`. The custom SVD must not duplicate
those peripherals.

The compiled parity verifier composes the custom radio map with
`svd/esp32s31-platform-radio-deps.svd`. That second file is only an address and
field catalog for decoding vendor ELF traces; it is not passed to `svd2rust`
and does not weaken the single-owner runtime rule above.

Wi-Fi DMA descriptors are SRAM shared with the MAC DMA engine, not MMIO. Their
memory-safety proof therefore belongs to the Wi-Fi MAC and integration layers,
separately from peripheral singleton ownership.

## Current unsafe boundaries

| Owner | Why unsafe is required | Required invariant |
| --- | --- | --- |
| generated `esp32s31/pac` | generated singleton, register pointers, array access and raw field writers | generated addresses and layouts match the reviewed SVD; only one `Peripherals` owner exists |
| `esp32s31/registers` | bounded `svd2rust` field writes whose safe API cannot express recovered numeric encodings | each value is masked or range-bounded and its source is recorded in SVD/PAC comments |
| `esp32s31/hal::Radio` | initial singleton claim and explicit adoption after an external comparison oracle initialized the radio | the integration token is unique and no vendor/open driver accesses the peripheral concurrently |
| `esp32s31/wifi/lmac` | volatile DMA descriptors, intrusive RX ownership, pinned TX/A-MPDU storage and referenced buffers | DMA-visible storage does not move or alias; ownership changes only at the documented descriptor/completion edges |
| `integration/network/embassy-net` | pinned copy-free network slots stored behind `UnsafeCell` | atomic slot state gives exactly one network or radio owner and acquire/release publication brackets byte access |
| `integration/esp32s31/wifi-embassy` | joins pinned network leases to S31 TX DMA storage | a lease outlives hardware ownership and is released only after completion and detach |
| `integration/esp32s31/wifi-esp-hal` | official PAC raw field writers and volatile PHY-I2C command access | encodings fit the official fields; singleton tokens retained by `EspHalRadioPeripheral` prove exclusive access |

Rust 2024 also requires `unsafe(...)` around attributes such as
`link_section`. Those attributes control target placement but are not pointer
or aliasing operations. They still require review because changing placement
can break the HIL timing and memory contract.

Portable WPA2 code no longer uses unsafe volatile erasure. Secret-bearing
types use safe zeroization. Portable IEEE 802.11 files contain placement
attributes for hot target code, but no raw descriptor ownership boundary.

## Layer rules

- PHY is compiled with `#![forbid(unsafe_code)]`. Hardware sequencing is
  expressed through actions and an exclusive `RadioRegisters` borrow.
- New register identities must enter through the SVD and PAC, with source and
  confidence metadata. Do not extend the temporary raw-register facade.
- Safe upper layers must not manufacture PAC singletons or retain raw MMIO
  pointers.
- Every public unsafe function must document the caller proof. Prefer a safe
  owner that establishes pinning, lifetime, and state before reaching it.
- HIL code may inspect raw addresses for diagnostics, but a stable runtime
  operation must move into SVD/PAC and a typed owner.

## Review procedure

For a change touching registers, DMA storage, pinning, or placement:

1. run `cargo pac-gen --check` when the SVD or generated PAC changes;
2. run the workspace tests and lints;
3. inspect new `unsafe` occurrences and ensure the invariant is stated next
   to the operation;
4. repeat the relevant HIL cell when ownership, linker placement, interrupt
   ordering, or DMA lifetime changes.

The [architecture](ARCHITECTURE.md) defines dependency direction. The
[feature ledger](ESP32S31_WIFI_FEATURE_STATUS.md) identifies the HIL cells
that must be repeated for behavioral changes.
