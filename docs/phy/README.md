# ESP32-S31 PHY parity audit

This directory is the maintained comparison between the open Rust PHY and the
pinned ESP32-S31 vendor implementation. It is an audit of observable radio
behaviour, not a claim that Rust type or function boundaries must resemble the
vendor ABI.

Audit baseline: 2026-07-30.

## Pinned evidence

| Artifact | SHA-256 | Role |
| --- | --- | --- |
| `_oracles/libphy.a` | `51497819736295c9b33d6775495dade4c6fb39db887edfe095608c670d9ae223` | ESP32-S31 vendor archive parents and target-specific leaves |
| `_oracles/esp32s31_rev0_rom.elf` | local container `d01bde81d9b3806e37ef1d9ac3b58af4f5b3d91eeef4f44d20e79d6a9f227542`; canonical container `a52ad7513deb656a910a5740125f1cce2c7941f11ce57213b7b43aea93d5ab87` | revision-zero ROM algorithms and finite MMIO leaves |
| `svd/esp32s31-radio.svd` | `67c81a8bfdfac7b5c0dd2aef3b782b92a97e261eeb535356c037c628fd495b59` | recovered register identities, fields, masks and provenance at audit completion |

The archive contains 21 members. Fifteen members define 161 externally visible
code symbols; six members define no externally visible code symbol. Of the 161
symbols, 133 start with `phy_`. The ROM ELF contains 305 externally visible
`phy_*` code symbols.

The local ROM ELF container has a different whole-file hash and size from the
canonical ELF cited by the SVD. This audit compared `.fixed.text`, `.init.text`,
`.text`, `.rodata` and `.rodata.interface` from both files; all five section
images are byte-identical. The difference is therefore recorded as container
provenance rather than an instruction/data-oracle difference.

Those counts must not be compared directly with Rust `fn` counts. A blocking
vendor function is normally represented by several Rust types and methods: a
transition, an action, an identity-bound completion, an external binding and
one or more finite HAL leaves.

## Layer status

| Layer | Current inventory | Audit meaning |
| --- | ---: | --- |
| Vendor archive | 161 external code symbols | Complete symbol and direct-body instruction/relocation inventory |
| Revision-zero ROM | 305 external `phy_*` code symbols | Complete symbol inventory; reachable cold Wi-Fi algorithms mapped by Rust module |
| Recovered SVD/PAC | 10 PHY peripherals, 123 register declarations, 228 field declarations | Address/field ownership and access-width evidence; not by itself proof of transaction order |
| Rust HAL | 15 non-`lib.rs` modules | Finite ownership-bound leaves for the currently reached PHY graph |
| Rust PHY | 26 `phy_*.rs` modules, plus `executor.rs` and transitional `radio_hal.rs` | Source-only state machines and pure transforms |

The detailed pages are:

- [vendor oracle inventory](vendor-oracle-inventory.md);
- [vendor defects and fragile contracts](vendor-defects.md);
- [complete instruction-audit method](audit-method.md);
- [complete function-audit ledger](function-audit-ledger.md);
- [PAC and HAL layer inventory](pac-hal-layer.md);
- [Rust PHY functional inventory](rust-phy-layer.md);
- [behaviour comparison and open findings](behavior-parity.md).

## Status vocabulary

- **Matched**: the audited input domain retains the vendor arithmetic, register
  images, ordering, bounds and successful outcome.
- **Scheduling-equivalent**: the same successful hardware operations occur,
  but a vendor busy loop or synchronous delay is exposed as an executor-owned
  readiness or timer edge.
- **Profile-matched**: matched only for an explicitly named production profile,
  such as full calibration, 40 MHz XTAL, Wi-Fi channel 1 through 13 and CBW
  selector zero.
- **Partial**: a reachable branch, child or lifecycle mode is absent.
- **Not ported**: the vendor capability has no Rust-owned implementation in the
  current PHY graph.
- **Intentional divergence**: Rust deliberately returns a typed error, adds a
  deadline or rejects invalid state where the vendor blocks, indexes invalid
  memory or relies on an ABI/global invariant.

“Matched” never means “all possible vendor entry points are implemented.” It is
always scoped to the row and input domain in the functional inventory.

## Complete-audit progress

The newer all-function requirement uses the stricter
[instruction-audit method](audit-method.md). At the current checkpoint:

- all 466 target archive and ROM PHY functions are inventoried;
- all 161 archive function bodies have been inspected instruction by
  instruction;
- 112 archive functions and 92 ROM functions are strictly closed;
- 49 additional archive functions have complete direct bodies recorded but
  retain child or Rust trace proofs;
- eight ROM functions have complete direct bodies recorded;
- 205 functions remain unreviewed under the strict all-branch criterion.

These numbers are maintained in
[function-audit-ledger.md](function-audit-ledger.md). Earlier profile-matched
rows do not count as complete until promoted through that ledger.

## Reproducible checks

Symbol counts:

```console
llvm-nm -A --defined-only --extern-only --print-size _oracles/libphy.a
llvm-nm -A --defined-only --extern-only --print-size _oracles/esp32s31_rev0_rom.elf
```

Primary parent disassembly:

```console
ar x _oracles/libphy.a phy_init.o phy_rfpll.o
llvm-objdump -dr --symbolize-operands phy_init.o
llvm-objdump -dr --symbolize-operands phy_rfpll.o
```

Host verification:

```console
cargo test -p open-esp-radio-esp32s31-phy \
  -p open-esp-radio-esp32s31-hal \
  -p open-esp-radio-esp32s31-pac
```

At this baseline the command passed 207 PHY, 18 HAL and 64 PAC unit tests.
Warnings were present, but there were no failed tests.

## Maintenance rule

Every new ported vendor root must add or update:

1. its symbol and oracle location in the vendor inventory;
2. its Rust owner and supported input profile in the PHY inventory;
3. transaction-level parity notes and any deliberate divergence;
4. the SVD source tag when new register or bit evidence is promoted;
5. a test that fixes pure arithmetic, table geometry, MMIO image or action
   ordering, as applicable.

A hardware pass is qualification evidence, not a substitute for the complete
body and relocation audit. Conversely, instruction parity without a hardware
run does not prove analog calibration quality.
