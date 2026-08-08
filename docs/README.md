# Documentation

Documentation is divided by maintenance contract. Current documents describe
the checked-in workspace. Evidence records preserve a result at a named date
and are not silently rewritten to match later API paths. Archive reports are
provenance, not instructions.

## Current architecture and policy

- [Architecture](ARCHITECTURE.md) — crate boundaries, protocol/chip scope,
  dependency direction and the separate test-harness layer.
- [Radio lifecycle and ownership](RADIO_LIFECYCLE_AND_OWNERSHIP.md) — normative
  physical-radio, subsystem, coexistence and Wi-Fi role state/owner model.
- [Naming and repository layout](NAMING_AND_LAYOUT.md) — canonical layer
  vocabulary, target directory tree and migration rules.
- [Public source policy](SOURCE_POLICY.md) — allowed and excluded inputs.
- [PAC/MMIO/unsafe audit](PAC_AND_UNSAFE_AUDIT.md) — current register and
  memory-ownership boundary.

## Current status and backlog

- [Qualification progress ledger](../qualification/README.md) — machine-checked
  production ownership, host/vendor/HIL proof and async readiness for the ten
  supported STA capability roots.
- [ESP32-S31 Wi-Fi feature status](ESP32S31_WIFI_FEATURE_STATUS.md) — canonical
  detailed PHY/MAC feature and hardware-cell matrix.
- [Integration backlog](INTEGRATION_BACKLOG.md) — reusable runtime logic that
  still resides in the ESP32-S31 HIL application.
- [PHY binary parity](phy/README.md) — entry point to the compiled verifier and
  its machine-generated open-work report.

Current status documents must state when they were last verified. Update them
when code ownership, public paths, qualification state or listed counts
change; do not append completed chronology to a live backlog.

## Research and qualification evidence

- [Vendor Binary Workbench](../tools/vendor-binary-workbench/README.md)
  builds reviewable binary IR and MMIO/interface/function evidence, publishes
  reviewed register artifacts and verifies Rust replacements.
- The [workbench architecture](VENDOR_BINARY_WORKBENCH_ARCHITECTURE.md)
  separates the generic engine, ISA backends, CLI and chip-specific harnesses.
- [Register provenance](esp32s31-radio-register-provenance.md) records the
  basis and confidence for recovered register descriptions.
- [Debug oracles](esp32s31-debug-oracles.md) records comparison-only symbol and
  descriptor evidence, including evidence relevant to future radio protocols.
- [ESP32-S31 qualification records](../qualification/targets/esp32s31/records/README.md)
  contain dated, immutable hardware results and their reproduction contracts.

Historical paths inside an evidence record describe the tested revision. If a
current path is needed, add a clearly marked note rather than rewriting the
original command or result.

## Historical archive

[`archive/`](archive/README.md) contains completed transfer reports, superseded
audits and vendor-library analyses. No archive document defines the current
crate layout, API or verification workflow. Git history is sufficient for
small transfer bookkeeping; retain large reports only when they carry unique
reverse-engineering evidence.
