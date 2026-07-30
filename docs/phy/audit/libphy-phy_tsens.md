# Line audit: `libphy.a[phy_tsens.o]`

Artifact:
`_oracles/libphy.a`, SHA-256
`51497819736295c9b33d6775495dade4c6fb39db887edfe095608c670d9ae223`.

The member defines five external code functions. Their direct bodies are
complete below. ROM conversion children and target integration bindings remain
open strict proofs, so all five ledger rows are **BODY-AUDITED**.

## `phy_set_tsens_power`

Size `0x1c`, weak IRAM definition. It performs one fresh 32-bit RMW at
`0x20818000`:

1. read the word;
2. clear bit 22 with mask `0xffbfffff`;
3. set bit 22 from bit zero of the input;
4. write the result.

Higher input bits are discarded. Rust
`PhyTemperatureSystemControl` exposes only
`enable_temperature_sensor_power()`, so the initialization path can represent
input one but not the input-zero trace used by `phy_xpd_tsens`. No target
implementation of this trait exists inside this repository, so the actual
address-level binding is also outside the current proof.

## `phy_set_tsens_range`

Size `0x14`. It:

1. loads `phy_param[0x16]` as the current range index;
2. sign-extends the low 16 bits of its temperature input;
3. tail-calls `phy_tsens_dac_cal(temperature, range_index)`.

The Rust temperature transition contains the corresponding range-selection
arithmetic and conditional PHY-I2C write, but only as part of a complete sample.
The standalone input domain and ROM child are not yet strictly closed.

## `phy_get_tsens_value`

Size `0x08`. It is an argument- and return-preserving tail-call to
`phy_tsens_temp_read_local`.

Rust has the same valid-DAC sample graph plus a documented vendor-defect
exception for invalid DAC indices. The wrapper remains open until the complete
ROM child proof is promoted into the strict ledger. See
[vendor defects](../vendor-defects.md#vendor-defect-001-invalid-temperature-dac-indexes-past-the-table).

## `phy_tsens_read_init`

Size `0x36`. Both incoming ABI arguments are ignored. It performs:

1. fresh-read OR bit 0 at `0x20818018`;
2. fresh-read OR bit 30 at `0x20710030`;
3. fresh-read OR bit 23 at `0x20818018`;
4. fresh-read OR bit 9 at `0x20818018`;
5. tail-call `phy_set_tsens_power(1)`, producing a fresh-read update of bit 22
   at `0x20818000`.

The Rust HAL `phy_temperature::initialize` emits five methods in exactly this
order. This is a strong structural match, but it is not yet strict
address-level parity because the production
`PhyTemperatureSystemControl` implementation is not present in this
repository.

## `phy_get_temp_init`

Size `0x4c`. Inputs are two boolean-like values; every nonzero value is true.
The complete body:

1. calls `phy_tsens_temp_read()`, which updates the current signed temperature
   at `phy_param[0x000..0x002)`;
2. when the second input is nonzero, copies that halfword to offsets `0x12e`,
   `0x048`, `0x1f8` and `0x1fa`, in that order;
3. always copies the halfword at `0x12e` to offset `0x004`;
4. when the first input is nonzero, copies the current halfword at offset zero
   to offset `0x130`;
5. returns without MMIO of its own.

Rust `apply_full_calibration_temperature` reproduces the parameter result for
the caller profile `(1, 1)`. It does not expose all four input branch
combinations, so the function is not globally matched.

## Member conclusion

The cold initialization and `(1, 1)` temperature-state path are close to the
vendor body. Strict blockers are:

- missing power-disable capability in the Rust temperature HAL contract;
- production register bindings living outside this repository;
- no standalone range/value APIs;
- only one of four `phy_get_temp_init` input branches is composed;
- the ROM invalid-DAC defect requires its explicit exception proof.

No status is promoted to MATCHED until those boundaries are closed.
