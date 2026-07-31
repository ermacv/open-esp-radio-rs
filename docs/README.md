# Documentation

Start with the documents in this section. Audit logs are retained for
traceability, but they are not all current design instructions.

## Maintained documents

- [Architecture](ARCHITECTURE.md) — crate boundaries and dependency direction.
- [Public source policy](SOURCE_POLICY.md) — allowed and excluded inputs.
- [ESP32-S31 feature status](ESP32S31_WIFI_FEATURE_STATUS.md) — implemented and
  hardware-qualified capabilities.
- [PHY port status](PHY_PORT_STATUS.md) — current PHY scope and limitations.
- [PAC/MMIO/unsafe audit](PAC_AND_UNSAFE_AUDIT.md) — generated PAC and ownership
  boundary.
- [Driver/HIL integration audit](ESP32S31_RUST_INTEGRATION_AUDIT.md) — remaining
  reusable logic in the HIL application.

## Technical reference

- [`phy/`](phy/README.md) contains the maintained PHY parity inventory and its
  per-function evidence.
- [Register provenance](esp32s31-radio-register-provenance.md) records the
  basis and confidence for recovered register descriptions.
- [Debug oracles](esp32s31-debug-oracles.md) records comparison-only symbol and
  descriptor evidence.
- [`hil/`](hil/) contains dated, immutable hardware qualification records.

## Historical material

[`archive/`](archive/README.md) contains completed transfer reports and
vendor-library analyses. These files explain how the current code was derived;
they do not define the current crate layout or verification workflow.
