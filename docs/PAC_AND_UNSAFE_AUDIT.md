# PAC, MMIO and unsafe ownership audit

Verified against the workspace on 2026-08-05.

This document records the current ownership boundary. It is deliberately not
a chronological migration report. The completed PAC migration narrative is
retained as a [dated archive snapshot](archive/migration/2026-07-27-pac-and-unsafe-audit.md).

## Register ownership

`verification/vendor/targets/esp32s31/registers/device.toml` and its fragments
are the editable source for undocumented radio registers in the
`0x2010_0000..0x201f_ffff` decode window. The project-owned `registers`
commands validate the model and generate the clean SVD, PAC and binding index.
`project publish` validates with strict reviewed coverage before deriving any
output; `project publish --check` proves that every described span fits the
project MMIO map and that the checked SVD, PAC and bindings are current.

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
ownership. The MAC backend and runtime/network composition receive only typed leases.

## Current unsafe boundaries

| Owner | Why unsafe is required | Required invariant |
| --- | --- | --- |
| generated `driver/chips/esp32s31/pac` | generated singleton, register pointers, array access and raw field writers | generated addresses and layouts match the reviewed SVD; only one `Peripherals` owner exists |
| `driver/common/dma` | pinned TX/RX storage behind `UnsafeCell`, state-specific leases and stable-address capability construction | the atomic state machine admits exactly one owner; a live radio lease retains a non-moving, non-aliased allocation |
| `driver/chips/esp32s31/wifi/dma` | volatile RX buffer ownership and target linker placement for qualified hot code | completed/recycle tokens follow descriptor ownership; `.rwtext.*` maps to aligned executable internal SRAM |
| `adapters/embassy/esp32s31-platform` | executor polling, software-interrupt adoption and exported Embassy ABI/linker symbols | application supplies the unique interrupt/timer resources; exported names and sections match the board runtime/linker contract |

Rust 2024 also requires `unsafe(...)` around attributes such as
`link_section`. Those attributes control target placement but are not pointer
or aliasing operations. They still require review because changing placement
can break the HIL timing and memory contract.

Every other driver crate forbids unsafe code at its crate root. In particular,
register transactions, HAL, PHY, chip MAC/STA, portable Wi-Fi, Embassy Wi-Fi
composition, the network adapter and the `esp-hal` adapter are safe consumers
of these boundaries. MAC hot-path section placement is requested through a
macro owned by the chip DMA leaf; the MAC crate does not locally waive its prohibition.

Portable WPA2 code uses safe zeroization for secret-bearing types. No portable
protocol or role layer owns a volatile register, DMA pointer or target linker
attribute.

## DMA failure frontier

Normal TX lease destruction releases the pool slot before publishing its
index back to a producer queue. An A-MPDU owner which reaches
`HardwareOwned`, `Completed` or `ResetRequired` first forgets every retained
backing, leaving each network slot claimed, and its lower pinned DMA owner
then rejects destruction. Ordinary TX applies the same terminal `Drop` guard
to its pinned descriptor/buffer owner. On the target, where panic aborts, this
is the fail-closed platform reset boundary. A Rust scope exit can therefore
neither return potentially DMA-visible memory nor silently discard the last
software lifecycle owner.

The current tree has no token proving that a complete platform radio reset has
stopped every DMA actor. Consequently a quarantined slot cannot be recovered
within the same process; the application must enter its terminal reset/reboot
path. `PinnedDmaTxPool::claimed_slots` is observation for diagnostics and does
not grant reset authority. A future recoverable reset path must first introduce
an unforgeable reset-completion owner and test the complete MAC/DMA shutdown
ordering before it can consume quarantined leases.

On 32-bit hardware targets, initial RX-ring construction requires a
`&'static RxDmaStorage`. `RxRingLive::Drop` rejects destruction until
`try_stop` confirms the walker disabled. Explicitly forgetting a live owner
can still leak the ring and leave the walker active, but it cannot deallocate
or move the descriptor/buffer arena underneath DMA. Native host models retain
a borrowed constructor because they have no asynchronous hardware actor. Raw RX-DMA
construction, cold-ring publication and standalone walker enable are unsafe on
32-bit targets and remain safe only in native models with no DMA actor. The
validation probes state their synthetic/static-address proof explicitly.

RX walker mutation now also requires a non-forgeable `StableDmaRange` produced
by the audited DMA leaves. `RxRingStopped` creates a private `RxDmaBinding`
from its retained descriptor arena and moves that authority through stopped
and live typestates. The public `RxDma` backend and the lower
`RadioRegisters` descriptor-base, enable and reload methods all require the
binding/range; owning a register singleton or implementing a mock backend is
no longer enough to publish an arbitrary safe-Rust address. The raw target
constructor/publication entry points are absent from ordinary production
builds and exist only under the explicit `validation-raw-dma` feature; those
harness calls manufacture synthetic authority under their unsafe contract.
There is no standalone raw target walker-enable entry point.

## TX ownership frontier

Ordinary TX now reaches the phased capability boundary. Its descriptor and
buffer live in `chips/esp32s31/wifi/dma::tx_storage::TxDmaStorage`; production
compositions pin that lower allocation in static SRAM and give the MAC backend only the
movable `PinnedTxDmaStorage` owner. `TxSlot::new_model` exists solely in native
models with no asynchronous DMA actor and is absent on 32-bit targets.

Queue preparation receives `PreparedTxDma`, while the final ENABLE|VALID edge
receives the distinct `HardwareOwnedTxDma` token created only after the lower
owner records its state transition. Both `TxHardware` and `RadioRegisters`
validate that PLCP0 names the retained chain. Completion, abort and detach
return backing through lower state transitions; any impossible sequence or
failed detach quarantines it.

Backing reuse no longer trusts a boolean returned by a safely implementable
hardware trait. `RadioRegisters::with_detached_mac_tx` creates a private
`MacTxQueueDetached` proof only after queue disable/invalid readback and lends
it to a callback for the duration of its exclusive register borrow. Ordinary
TX and production A-MPDU consume that proof in their lower DMA owners and
check it against the published descriptor head. Native mocks can
construct model proofs only on non-32-bit hosts, where no asynchronous DMA
actor exists. A target-side safe mock therefore cannot claim that DMA stopped
and make live backing reusable.

Production TX now reaches that ownership boundary for ordinary MPDU and
external-buffer A-MPDU. `HtAmpduTxResources` couples the upper recovered MAC
metadata with a separately pinned lower descriptor arena, and station
reconnect/teardown transfers that pair as one value. `RetainedDmaAmpduTx`
retains each network lease in `RetainedAmpduDma<B>`, resolves metadata ranges
inside those leases, publishes only through `PreparedTxDma`, records lower
hardware ownership before the doorbell receives `HardwareOwnedTxDma`, and
requires the detach proof before retry or release.

`chips/esp32s31/wifi/dma::tx_ampdu_storage` now supplies the lower owners needed for
both backing strategies. Internally buffered aggregates retain a static
descriptor array and aligned MPDU buffers. Descriptor-only zero-copy
aggregates use `RetainedAmpduDma<B>`, which owns every `StableDmaBacking`
lease referenced by the published chain. Both paths distinguish completion
from confirmed queue detach and issue separate prepare/start capabilities
only after validating the entire chain. Attempting to drop an external owner
after the hardware edge deliberately forgets the leases and then reaches the
lower pinned owner's terminal `Drop` guard unless detach was confirmed. Thus
a Rust destructor cannot return potentially DMA-visible memory or continue as
if the owner had completed normally.

`HtAmpduTxStorage` now contains only protocol/lifecycle metadata and its
bounded frame-formatting workspace. The workspace exists only in native
oracle/test builds; its buffers and upper-only commit/retry methods are absent
from 32-bit production builds, and a target compile-time layout assertion
ensures the metadata size is independent of the model buffer capacity. Its
duplicate descriptor array and the retired `submit`, `submit_he` and
`submit_he_smpdu` entry points have been removed. `AmpduFrameLayout` validates
the word-aligned metadata-prefix offset before a backing is retained, while
HT/HE request values keep length, rate and delimiter/TXOP policy together;
neither value can grant DMA access. Descriptor publication and queue-detach
proof exist only in the composed lower owner. `TxHardware` no longer
translates bare CPU addresses or contains raw legacy/HT/HE prepare/start
operations; the corresponding register methods are private helpers behind
capability-bound calls. The public TX hardware API is therefore
capability-closed for legacy, HT and HE.

The recovered `BasicHtAmpdu*` descriptor transformations are qualification
models, not runtime capabilities. They now live in a native-only `model`
module together with their raw descriptor constants and are absent from the
32-bit production API.

Do not fix this by adding an unchecked address token in the MAC backend. The required
order is:

1. move any future internally buffered hardware path onto the lower DMA leaf
   instead of restoring descriptors in the upper formatter;
2. add a recoverable reset authority only after the complete MAC/DMA shutdown
   order is qualified; until then quarantine remains terminal.

The reset-required/quarantine rule remains unchanged during this extraction:
no backing becomes reusable merely because its Rust queue owner was dropped.

The application facade exposes this lower composition layer explicitly as
`esp32s31::wifi::dma`, alongside `mac` and `sta`; it is not re-exported as if
DMA allocation were MAC policy. No known public TX submission method above
this boundary accepts an unowned raw descriptor address.

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

1. run `project configure --check` and `project publish --check` through the
   ESP32-S31 project manifest;
2. run the workspace tests and lints;
3. inspect new `unsafe` occurrences and ensure the invariant is stated next
   to the operation;
4. repeat the relevant HIL cell when ownership, linker placement, interrupt
   ordering, or DMA lifetime changes.

The [architecture](ARCHITECTURE.md) defines dependency direction. The
[feature ledger](ESP32S31_WIFI_FEATURE_STATUS.md) identifies the HIL cells
that must be repeated for behavioral changes.
