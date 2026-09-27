# Vendor verification

This tree holds the evidence that the production drivers follow the pinned
vendor code, organized by level. It contains no production driver behavior,
no HIL board scenarios and no private vendor artifacts.

| Level | Claim | Where |
| --- | --- | --- |
| L0 pins | which vendor code is the reference | [`esp32s31/artifacts.toml`](esp32s31/artifacts.toml) |
| L1 facts | recovered constants, tables and register facts describe the pinned code | production `SOURCE:` blocks, [`registers`](../registers/README.md) evidence, [`esp32s31/facts`](esp32s31/facts) |
| L2 behavior | compiled production code behaves as the vendor code | [`esp32s31/probes`](esp32s31/probes/README.md), [`esp32s31/scenarios`](esp32s31/scenarios), [`esp32s31/host`](esp32s31/host/ieee802154/README.md), shards in [`esp32s31/evidence/scenarios`](esp32s31/evidence/scenarios) |
| L3 hardware | the drivers work on the board and calibrate as the vendor firmware does | [`hil`](../hil/README.md), tracked shards in `hil/evidence/`, [`esp32s31/hardware`](esp32s31/hardware/calibration/README.md) with [`esp32s31/hil-vendor`](esp32s31/hil-vendor/README.md) |
| L4 readiness | a capability is qualified | [`qualification`](../qualification/README.md) |

```text
verification/
  schema/            evidence shard schema, shared with qualification
  harness/           probe code generation and macros, and the chip-neutral
                     scenario engine (harness/scenarios)
  esp32s31/
    artifacts.toml   L0: every pinned archive, ROM ELF and SDK build
    facts/           L1: cited-function fingerprints, recovered name maps
    probes/          L2: isolated workspace of compiled production entries
    scenarios/       L2: typed Blobray comparisons against the pinned binaries
    host/ieee802154/ L2: the public IEEE 802.15.4 driver compiled on the host
    evidence/        L2 output: one generated shard per scenario
    hil-vendor/      L3: vendor ESP-IDF firmware for hardware cross-checks
    hardware/        L3: vendor-versus-production cross-checks on the board
```

**L0.** `cargo xtask vendor-fetch esp32s31` downloads and verifies every pinned
artifact into `target/vendor/`; scenarios default to those paths and reject
any other bytes. Changing a pin means changing the manifest, then following
the pinned behavior in production.

**L1.** A recovered fact names the vendor function it was read from. `cargo
xtask check provenance --chip esp32s31` fails when such a function changed since its facts
were reviewed; `cargo xtask vendor-diff --chip esp32s31` shows what changed between two pins,
and `tools/symbol-lineage` pairs obfuscated names across releases. See the
[source policy](../docs/source-policy.md).

**L2.** A typed scenario runs the pinned vendor function and the compiled
production probe in one [Blobray](../tools/blobray/README.md) session and
fails closed with MATCH, DIFF or INCOMPLETE. Each scenario writes its shard
of the evidence index with `--index`; qualification treats a shard as stale
when any source it records changed. CI does not rerun the vendor scenarios,
because the `local-build` pins of `esp32s31/artifacts.toml` cannot be
downloaded: Blobray's periodic local `cargo xtask evidence --chip <chip>
--check` is the gate. The [ESP32-S31 project](esp32s31/README.md)
describes the scenarios, their inputs and their reviewed decisions; the
[vendor contract reference](../docs/vendor/esp32s31/README.md) explains the
vendor behavior they cover.

**L3 and L4.** HIL runs exercise the drivers on hardware; `cargo xtask
vendor-firmware esp32s31` builds the vendor firmware of `hil-vendor/` against
the pinned ESP-IDF, with its PHY, coexistence, Wi-Fi and Bluetooth library
submodules replaced by the pinned archives, and the
[calibration cross-check](esp32s31/hardware/calibration/README.md) compares
its cold calibration with production's on the same board through the
tracking scenario's reviewed relation, writing a dated summary to
`esp32s31/evidence/hardware/`. A qualifying
observation is recorded as a tracked shard bound to its firmware and observer
sources. The only path from
comparison or HIL evidence to product readiness is the independent
[verification and qualification contract](../docs/verification-and-qualification.md).

Private inputs never enter the repository: `_oracles/` and fetched artifacts
stay ignored, and checked configuration never names machine-local paths.
Generated analysis stays in ignored outputs except the evidence shards.
