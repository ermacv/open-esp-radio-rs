# Documentation

This directory holds the contracts that cross code owners. A component's own
details live beside it, in its `README.md` and rustdoc; `cargo xtask doc`
builds the API documentation, and each binary's `--help` is its CLI
reference.

- [Repository architecture](architecture.md): ownership, layers, host
  layers, radio ports and the
  [shared contracts](architecture.md#changing-a-shared-contract).
- [Documentation policy](documentation.md): where information belongs and how
  documents are checked.
- [Issues and dependencies](issues.md): issue classification, decisions and
  approval.
- [Public source policy](source-policy.md): permitted inputs and source checks.
- [Verification and qualification](verification-and-qualification.md):
  evidence strength, freshness and the sole readiness authority.
- [HIL terminology](hil-terminology.md): one meaning for each
  hardware-in-the-loop term and role.
- [HIL provenance and replay](hil-reproducibility.md): artifact identity,
  source reconstruction, rebuild comparison and lab observations.
- [Protocol terminology](protocol-naming.md): IEEE 802.11, Wi-Fi,
  IEEE 802.15.4, Bluetooth and module naming.
- [Hardware errata](hardware-errata.md): hardware and ROM behaviour the
  stand's boards show.
- [PHY comparison](phy/README.md): compiled comparison and incomplete
  outcomes.
- [ESP32-S31 vendor contracts](vendor/esp32s31/README.md): pinned vendor
  ordering, software state and memory ownership, with each observation's limits.
