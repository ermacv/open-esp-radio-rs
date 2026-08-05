# PAC, MMIO and unsafe ownership audit

Verified against the workspace on 2026-08-05.

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
memory-safety proof therefore terminates in the portable DMA-ownership crate
and the ESP32-S31 Wi-Fi DMA leaf, separately from peripheral singleton
ownership. LMAC and runtime/network composition receive only typed leases.

## Current unsafe boundaries

| Owner | Why unsafe is required | Required invariant |
| --- | --- | --- |
| generated `esp32s31/pac` | generated singleton, register pointers, array access and raw field writers | generated addresses and layouts match the reviewed SVD; only one `Peripherals` owner exists |
| `driver/dma` | pinned TX/RX storage behind `UnsafeCell`, state-specific leases and stable-address capability construction | the atomic state machine admits exactly one owner; a live radio lease retains a non-moving, non-aliased allocation |
| `esp32s31/wifi/dma` | volatile RX buffer ownership and target linker placement for qualified hot code | completed/recycle tokens follow descriptor ownership; `.rwtext.*` maps to aligned executable internal SRAM |
| `integration/esp32s31/embassy-runtime` | executor polling, software-interrupt adoption and exported Embassy ABI/linker symbols | application supplies the unique interrupt/timer resources; exported names and sections match the board runtime/linker contract |

Rust 2024 also requires `unsafe(...)` around attributes such as
`link_section`. Those attributes control target placement but are not pointer
or aliasing operations. They still require review because changing placement
can break the HIL timing and memory contract.

Every other driver crate forbids unsafe code at its crate root. In particular,
register transactions, HAL, PHY, chip LMAC/STA, portable Wi-Fi, Embassy Wi-Fi
composition, the network adapter and the `esp-hal` adapter are safe consumers
of these boundaries. LMAC hot-path section placement is requested through a
macro owned by the chip DMA leaf; LMAC does not locally waive its prohibition.

Portable WPA2 code uses safe zeroization for secret-bearing types. No portable
protocol or role layer owns a volatile register, DMA pointer or target linker
attribute.

## DMA failure frontier

Normal TX lease destruction releases the pool slot before publishing its
index back to a producer queue. If an A-MPDU owner is destroyed while its
descriptor remains `HardwareOwned` or `ResetRequired`, it instead forgets the
retained backing and leaves the slot claimed. This is an intentional
fail-closed quarantine: reusing that memory would be unsound while the walker
may still hold its address.

The current tree has no token proving that a complete platform radio reset has
stopped every DMA actor. Consequently a quarantined slot cannot be recovered
within the same process; the application must enter its terminal reset/reboot
path. `PinnedDmaTxPool::claimed_slots` is observation for diagnostics and does
not grant reset authority. A future recoverable reset path must first introduce
an unforgeable reset-completion owner and test the complete MAC/DMA shutdown
ordering before it can consume quarantined leases.

## Layer rules

- All crates above the three audited leaves are compiled with
  `#![forbid(unsafe_code)]`. Hardware sequencing is expressed through actions,
  typed leases and exclusive semantic owners.
- Audited leaves use crate-level `#![deny(unsafe_code)]` and narrow, reasoned
  allowances at the exact operation or target-binding module. Do not add a
  fourth leaf merely to make a local implementation convenient.
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
