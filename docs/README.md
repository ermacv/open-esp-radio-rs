# Documentation

Documentation is divided by maintenance contract. Current documents describe
the checked-in workspace. Evidence records preserve a result at a named date
and are not silently rewritten to match later API paths. Archive reports are
provenance, not instructions.

## Current architecture and policy

- [Architecture](ARCHITECTURE.md) — crate boundaries, protocol/chip scope,
  dependency direction and the separate test-harness layer.
- [Public source policy](SOURCE_POLICY.md) — allowed and excluded inputs.
- [PAC/MMIO/unsafe audit](PAC_AND_UNSAFE_AUDIT.md) — current register and
  memory-ownership boundary.

## Current status and backlog

- [ESP32-S31 Wi-Fi feature status](ESP32S31_WIFI_FEATURE_STATUS.md) — canonical
  implemented/HIL-qualified capability ledger.
- [Integration backlog](INTEGRATION_BACKLOG.md) — reusable runtime logic that
  still resides in the ESP32-S31 HIL application.
- [PHY binary parity](phy/README.md) — entry point to the compiled verifier and
  its machine-generated open-work report.

Current status documents must state when they were last verified. Update them
when code ownership, public paths, qualification state or listed counts
change; do not append completed chronology to a live backlog.

## Research and qualification evidence

- The [compiled PHY verifier](../tools/phy-trace/README.md) inventories vendor
  functions and reports instruction-level parity gaps directly from binaries.
- [Register provenance](esp32s31-radio-register-provenance.md) records the
  basis and confidence for recovered register descriptions.
- [Debug oracles](esp32s31-debug-oracles.md) records comparison-only symbol and
  descriptor evidence, including evidence relevant to future radio protocols.
- [`hil/`](hil/README.md) contains dated, immutable hardware qualification
  records and their reproduction contracts.

Historical paths inside an evidence record describe the tested revision. If a
current path is needed, add a clearly marked note rather than rewriting the
original command or result.

## Historical archive

[`archive/`](archive/README.md) contains completed transfer reports, superseded
audits and vendor-library analyses. No archive document defines the current
crate layout, API or verification workflow. Git history is sufficient for
small transfer bookkeeping; retain large reports only when they carry unique
reverse-engineering evidence.
