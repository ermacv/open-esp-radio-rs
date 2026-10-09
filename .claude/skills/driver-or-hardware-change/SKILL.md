---
name: driver-or-hardware-change
description: Use when changing chip hardware code in this repository — crates/hardware/ (ESP32-S31/C5 PAC, HAL, PHY, calibration, Wi-Fi/Bluetooth/802.15.4 drivers, DMA), crates/roles/, crates/runtime/esp32s31/, esp-hal adapters, unsafe code, MMIO access, register models under registers/, or recovered vendor tables and coefficients.
---

# Change a driver or hardware access

Read first (about 3k tokens): [unsafe boundaries](../../../crates/UNSAFE.md),
[recovered tables](../../../docs/source-policy.md#recovered-tables-and-coefficients)
and, for register changes, [registers/CLAUDE.md](../../../registers/CLAUDE.md)
(the full map is in [register ownership](../../../registers/esp32s31/README.md)).
Known silicon problems: [hardware errata](../../../docs/hardware-errata.md).

## Checklist

1. **MMIO through the PAC only.** Use typed PAC accessors and transactions.
   Find them by grepping `crates/hardware/<chip>/pac/src/generated.rs` or
   `pac/raw/src/lib.rs` with an explicit path; never read those files whole.
2. **Missing field?** Publish it: edit `registers/<chip>/model/` (geometry,
   semantics, evidence) and `policy/api.toml` (transactions), then run
   `cargo registers generate` and commit the regenerated outputs with the
   change. Never add a local mask, shift or raw address.
3. **Unsafe.** Keep it narrow; every `unsafe` block or impl gets a
   `// SAFETY:` comment directly above it, below any `#[allow]`. A new
   unsafe-permitting package or a new direct PAC dependency needs the
   reviewed lists in `cargo xtask check architecture`.
4. **Ownership.** Prefer typed acquisition, handoff and release over raw
   addresses or register images; document error, cancellation and
   quarantine paths in rustdoc.
5. **Recovered data.** Commit required tables and coefficients in production
   source with source identity, purpose, representation and applicable
   hardware; cite the vendor function in a `SOURCE(<chip>):` block. Binary
   origin is never a reason to omit data or use an older profile.
6. **Tests.** Test behavior and ownership; never generated addresses, masks,
   shifts, field positions or PAC type names.
7. **Evidence.** Vendor behavior comparisons: `vendor-evidence` skill.
   Hardware-facing changes run the HIL scenarios they need: `hil-run` skill.

## Commands (all `run_in_background: true`)

```console
cargo test -p <package>
cargo registers validate --manifest registers/<chip>/publication/registers.toml
cargo registers generate --manifest registers/<chip>/publication/registers.toml
cargo xtask check architecture
cargo hil images check --class performance --type-check
cargo verification check provenance --chip <chip>   # after SOURCE citations change
cargo xtask check changed
```

`<chip>` is `esp32s31` or `esp32c5`. ESP32-C5 has a PAC, HAL and SoC
adapter but no radio driver, role or runtime yet.

A PR calls out every generated SVD/PAC change.
