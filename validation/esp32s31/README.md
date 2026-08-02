# ESP32-S31 validation harness

This directory owns target-specific input for compiled vendor-to-Rust
validation. It is deliberately outside the generic validator tool.

- `target.spec` selects the RISC-V 32-bit backend, ILP32 calling convention,
  ESP32-S31 PHY harness and Rust recompilation target.
- `run.spec.example` documents the separate caller-owned artifact bindings.
- `profiles/` contains concrete compiled-equivalence scenarios.
- `dispositions/` maps vendor inventory symbols to Rust components and
  executable contracts.
- `baselines/` contains expected evidence classifications.

No file here selects a proprietary artifact path or authenticates one. The
caller validates the desired vendor revision and passes absolute paths at run
time, either as command options or through an untracked copy of
`run.spec.example` passed with `--run-spec`. Private integration tests
recognize these explicit variables:

- `OPEN_ESP_RADIO_ESP32S31_ROM_ELF`
- `OPEN_ESP_RADIO_ESP32S31_LIBPHY_ARCHIVE`
- `OPEN_ESP_RADIO_ESP32S31_LIBPP_ARCHIVE`

The protected `oracle-regression` GitHub environment must provide
`ESP32S31_ROM_SHA256` and `ESP32S31_LIBPHY_SHA256` as Actions configuration
variables. The workflow checks them before building or invoking the validator.
Those values are caller policy and deliberately do not live in this target
pack or the validator binary.

ABI versions, callback tables and lifecycle entry contracts are compiled from
the dedicated `tools/vendor-code-validator/crates/harness-esp32s31` fixture
crate. The remaining typed semantic adapters stay in the validator facade
until their backend-facing API is extracted; see
[`docs/VENDOR_CODE_VALIDATOR_ARCHITECTURE.md`](../../docs/VENDOR_CODE_VALIDATOR_ARCHITECTURE.md).
