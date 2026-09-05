# Unsafe boundaries

Safe callers must not manufacture register, interrupt, descriptor or DMA
ownership. Most driver crates forbid unsafe code. Trusted exceptions use
`deny(unsafe_code)` with documented item-level allowances and retain
`deny(unsafe_op_in_unsafe_fn)`; membership in an exception list is not blanket
permission to add unsafe operations.

The executable policy is
[`tools/audit-driver-safety.sh`](../tools/audit-driver-safety.sh). Its generated
package handling, audited-unsafe list and direct-PAC-dependency list are
separate controls. Package names remain stable across the directory moves
shown below.

## Generated access and trusted handwritten code

`chips/esp32s31/pac/raw/` is the `open-esp-radio-esp32s31-pac-raw` package.
It contains SVD-generated register access and handwritten ownership/transport
sidecars. Generated volatile access is checked through the generator/publisher
pipeline. Handwritten sidecars must carry their own safety proofs, deny unsafe
operations in unsafe functions and permit unsafe code only at documented
scoped exceptions. Do not classify the whole package as generated source.

`chips/esp32s31/pac/` contains the handwritten semantic radio PAC and its
generated capability catalog in `src/generated.rs`. The
[PAC provenance map](chips/esp32s31/pac/README.md) distinguishes both generated
Rust outputs from ownership modules and raw sidecars. Non-radio register
access through upstream `esp-hal`/`esp-pacs` belongs to
`adapters/esp-hal/esp32s31/soc/`, which also contains cache/MMU and GDMA
transactions. Its retained Cargo name ends in `platform-pac`; that name does
not make its implementation generated code.

The audit currently permits scoped unsafe exceptions in these handwritten
packages. Prefixes below omit `open-esp-radio-` only to keep the mapping readable.

| Package suffix | Source path |
| --- | --- |
| `dma` | `memory/` |
| `esp32s31-bluetooth` | `chips/esp32s31/bluetooth/` |
| `esp32s31-hal` | `chips/esp32s31/hal/` |
| `esp32s31-pac` | `chips/esp32s31/pac/` |
| `esp32s31-platform-pac` | `adapters/esp-hal/esp32s31/soc/` |
| `esp32s31-phy` | `chips/esp32s31/phy/` |
| `esp32s31-ieee802154-dma` | `chips/esp32s31/ieee802154/dma/` |
| `esp32s31-ieee802154-runtime` | `chips/esp32s31/ieee802154/runtime/` |
| `esp32s31-wifi-dma` | `chips/esp32s31/ieee80211/dma/` |
| `esp32s31-radio-platform-esp-hal` | `adapters/esp-hal/esp32s31/radio/` |
| `esp32s31-embassy-runtime` | `adapters/embassy/esp32s31/runtime/` |
| `esp32s31-bluetooth-integration` | `integration/esp32s31/embassy/bluetooth/` |
| `esp32s31-embassy-wifi` | `integration/esp32s31/embassy/ieee80211/` |

These exceptions cover distinct obligations: singleton acquisition and MMIO
serialization, stable addresses and CPU/DMA transfer, target ABI and placement,
and one-time static resource or interrupt binding. Preserve the proof at the
smallest operation; a safe state machine in an audited crate remains safe.

## PAC dependency authority

Direct dependencies on the semantic radio PAC are restricted independently
of unsafe syntax. The current allowed paths are `pac/raw`, `pac`, `hal`,
`bluetooth`, `ieee802154/irq` and `ieee802154/runtime` under
`chips/esp32s31/`, plus `adapters/esp-hal/esp32s31/{soc,ieee802154}/`.
In particular, the IEEE 802.15.4 IRQ crate's dependency permission does not
permit unsafe code. Check the executable lists when changing these boundaries.

Upper layers use opaque, finite capabilities rather than raw pointers,
unchecked lifetimes, generic PAC callbacks or independently reusable interrupt
owners. MMIO outside the restricted PAC must use typed accessors. Missing
register fields are reviewed and published in the SVD/PAC, not reconstructed
with local masks in a HAL or adapter.

## Active-owner lifetime

Normal stop proves IRQ, RX, TX and queue quiescence before returning reusable
storage. `Drop` cannot perform that asynchronous proof: abnormal drop retains
or quarantines active static storage and faults the lifecycle. It must not
reset hardware, panic as a release mechanism or claim successful shutdown.
Because safe Rust permits `mem::forget`, forgetting an owner must remain a
harmless leak rather than release memory still accessible by hardware.

Wi-Fi's Core0 supervisor may transfer its affine owner to a controlled child
task and await its return. Task boundaries, cancellation and rendezvous do not
weaken the same ownership or quiescence obligations. The wider execution and
memory-placement contracts are in the [driver architecture](README.md).
