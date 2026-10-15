# Unsafe boundaries

Safe callers must not manufacture register, interrupt, descriptor or DMA
ownership. The policy lives in standard Rust and Cargo lint settings, so every
`cargo check`, `cargo build` and `cargo clippy` enforces it:

- Most production crates start with `#![forbid(unsafe_code)]`.
- Trusted exceptions start with
  `#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]`, permit unsafe
  code only through documented item-level allowances and retain
  `unsafe_op_in_unsafe_fn = "deny"` from `[workspace.lints]`. Membership in
  the exception list is not blanket permission to add unsafe operations.
- `[workspace.lints.clippy]` denies `disallowed_methods`; `clippy.toml` lists
  them, and an intentional use carries a local `allow` with a reason.

In audited packages every
`unsafe` block and `unsafe impl` needs a `// SAFETY:` comment on the lines
immediately before it, stating why its prerequisites hold. A forwarding
call inside an `unsafe fn` names the `# Safety` contract it relies on. Put the
comment below any `#[allow(unsafe_code, ...)]` attribute: Clippy 1.97 does
not accept a comment above an attribute that spans several lines.

`cargo clippy --workspace --all-targets` applies these lints on the host, and
`cargo xtask check architecture` runs Clippy for every production feature
profile on the target. It also
[checks](../tools/xtask/src/checks/architecture/unsafe_policy.rs) that the
reviewed audited-unsafe list and the crate-root attributes agree, and the
direct-PAC-dependency list; generated PAC code has no crate-root unsafe
attribute. The table below maps package identities to source owners.

## Generated access and trusted handwritten code

`hardware/esp32s31/pac/raw/` is the `oer-esp32s31-pac-raw` package,
`hardware/esp32c5/pac/raw/` the `oer-esp32c5-pac-raw` package and
`hardware/ieee80211/pac/raw/` the `oer-ieee80211-pac-raw` package of the
Wi-Fi MAC register blocks both chips place at their own addresses. Each
contains only SVD-generated register access, checked through the
generator/publisher pipeline; the shared package contains no address. The publisher has no declaration for
handwritten modules in this package.

`hardware/esp32s31/pac/` contains the handwritten semantic radio PAC and its
generated capability catalog in `src/generated.rs`. The
[PAC provenance map](hardware/esp32s31/pac/README.md) distinguishes both generated
Rust outputs from its handwritten ownership, domain and validation modules,
including the IEEE 802.15.4 task/interrupt split. Non-radio register
access through upstream `esp-hal`/`esp-pacs` belongs to
`adapters/esp-hal/esp32s31/soc/`, which also contains cache/MMU and GDMA
transactions. Its Cargo package is `oer-esp32s31-soc-esp-hal`; its implementation
is handwritten code.

The executable policy permits scoped unsafe exceptions in these handwritten
packages. Prefixes below omit `oer-` only to keep the mapping readable.

| Package suffix | Source path |
| --- | --- |
| `memory` | `memory/` |
| `trace` | `trace/` |
| `embassy-net-owned` | `adapters/embassy-net/owned/` |
| `esp32s31-bluetooth` | `hardware/esp32s31/driver/bluetooth/` |
| `esp32s31-hal` | `hardware/esp32s31/hal/` |
| `esp32s31-pac` | `hardware/esp32s31/pac/` |
| `esp32s31-soc-esp-hal` | `adapters/esp-hal/esp32s31/soc/` |
| `esp32s31-phy` | `hardware/esp32s31/phy/` |
| `espressif-ieee802154-engine` | `hardware/espressif/ieee802154/engine/` |
| `esp32s31-ieee80211-form` | `runtime/esp32s31/ieee80211-form/` |
| `esp32s31-ieee80211-dma` | `hardware/esp32s31/driver/ieee80211/dma/` |
| `esp32s31-radio-esp-hal` | `adapters/esp-hal/esp32s31/radio/` |
| `espressif-executor-embassy` | `adapters/embassy/espressif/executor/` |
| `espressif-interrupt-table-esp-hal` | `adapters/esp-hal/espressif/interrupt-table/` |
| `esp32s31-bluetooth-system` | `composition/esp32s31/embassy/bluetooth/` |
| `esp32s31-ieee80211-system` | `composition/esp32s31/embassy/ieee80211/` |
| `esp32s31-ieee802154-system` | `composition/esp32s31/embassy/ieee802154/` |
| `esp32c5-pac` | `hardware/esp32c5/pac/` |
| `interrupt-table` | `runtime/interrupt-table/` |

These exceptions cover distinct obligations: singleton acquisition and MMIO
serialization, stable addresses and CPU/DMA transfer, target ABI and placement,
and one-time static resource or interrupt binding. `embassy-net-owned` has one
unsafe module, `external_rx`: it adopts a detached RX DMA buffer from an
`oer-memory` handoff slot as a Xarxa `PacketBuf` (`ExternalPacketOrigin::new`
and `adopt`) and returns the slot through `release_adopted` when the packet
drops. A slot is adopted again only after that release returned, because the
radio must first claim the slot the release freed. Preserve the proof at the
smallest operation; a safe state machine in an audited crate remains safe.

Wi-Fi's [S31 execution form](runtime/esp32s31/ieee80211-form/README.md)
keeps the permanent backing lease, driver and IRQ registration in one
primitive. The future owns only an affine `Pin<&'static mut ScopeStorage>`
lease. Context and the result latch are outside every registered callback's
graph. IRQ callbacks project a separate region containing no driver address;
a driver step's exclusive borrow cannot alias an ISR borrow. Admission counts
an entrant before checking the open gate. Detach masks all inputs, closes
admission and waits within a fixed bound on every core. Successful quiescence
allows one synchronous stop and returns the lease. Timeout never borrows the
driver or claims hardware stopped: the backing, DMA and owners remain in
permanent quarantine, even after the future is dropped. Only reset recovers
from that outcome. ISR wake tails own their snapshots and wakers; all
installation borrows end before wake. Composition wakers may only schedule,
never synchronously poll or detach. No interrupt-priority order is a safety
premise. The driver's waker carries only the permanent slot address: clone
and drop do not access an installation, and wake acquires fresh admission
before reading the current registration. A retained waker after detach cannot
reach an old backing. The unsafe binding constructor checks routing and step/stop callbacks
together; a safe route configuration cannot authorize later arbitrary callbacks.
Command storage uses `oer-memory::AffineIndexedSpscQueue`: the permission's
place selects its value cell, and the existing audited SPSC ring stores only
indices. Memory derives available ring space from N distinct indices, without
an occupancy check, a backend free-credit pool or another await. The form's
only queue unsafe is the private `UniqueQueueIndex` implementation: its sole
Mint, disjoint place ranges and typed terminal/gate transitions prove index
bounds and uniqueness. Producer Release publishes cells; consumer Acquire
moves them completely before permission return orders the next use. The
queue's typed endpoints are exclusive; no uncredited insertion or second
issuer is public. These branded queues are never registered with an
external ISR or driver step. The S31 Driver value owns only its permanent
backing lease and caller-side result loan. Its driving future borrows Driver;
consuming owner recovery requires that loan to have ended. Explicit abandon
closes the same admission domain before the core quarantines Driver without
Drop. Destroying the outer Driver container cannot reclaim the backing or
invalidate an entered actor's address. A completed shutdown reply remains in
permanent storage so a subsequent abandon cannot inspect extracted owners or
stop twice. Owned Driver cancellation uses that same synchronous primitive.

## PAC dependency authority

Direct dependencies on the semantic radio PAC are restricted independently
of unsafe syntax. The only allowed consumers are the chip's `pac/raw`, `pac`
and `hal` under `hardware/esp32s31/`, and the chip-neutral Wi-Fi MAC register
crates under `hardware/ieee80211/pac/` that those PACs place: drivers and
adapters reach registers through HAL owners. Check the executable list when changing this boundary.

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
