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

`hardware/esp32s31/pac/raw/` is the `oer-esp32s31-pac-raw` package.
It contains only SVD-generated register access, checked through the
generator/publisher pipeline. The publisher has no declaration for
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
| `esp32s31-bluetooth` | `hardware/esp32s31/driver/bluetooth/` |
| `esp32s31-hal` | `hardware/esp32s31/hal/` |
| `esp32s31-pac` | `hardware/esp32s31/pac/` |
| `esp32s31-soc` | `adapters/esp-hal/esp32s31/soc/` |
| `esp32s31-phy` | `hardware/esp32s31/phy/` |
| `esp32s31-ieee802154` | `hardware/esp32s31/driver/ieee802154/` |
| `esp32s31-wifi-dma` | `hardware/esp32s31/driver/ieee80211/dma/` |
| `esp32s31-radio-platform-esp-hal` | `adapters/esp-hal/esp32s31/radio/` |
| `esp32s31-executor-embassy` | `adapters/embassy/esp32s31/executor/` |
| `esp32s31-bluetooth-system` | `composition/esp32s31/embassy/bluetooth/` |
| `esp32s31-embassy-wifi` | `composition/esp32s31/embassy/ieee80211/` |

These exceptions cover distinct obligations: singleton acquisition and MMIO
serialization, stable addresses and CPU/DMA transfer, target ABI and placement,
and one-time static resource or interrupt binding. Preserve the proof at the
smallest operation; a safe state machine in an audited crate remains safe.

## PAC dependency authority

Direct dependencies on the semantic radio PAC are restricted independently
of unsafe syntax. The only allowed paths are `pac/raw`, `pac` and `hal`
under `hardware/esp32s31/`: drivers and adapters reach registers through HAL
owners. Check the executable list when changing this boundary.

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
