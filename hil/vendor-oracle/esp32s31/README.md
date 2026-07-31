# ESP32-S31 vendor PHY oracle

This is an intentionally isolated hardware oracle. It links Espressif's PHY
implementation, performs vendor cold PHY initialization, and then hands the
radio peripheral to the open MAC/channel code for differential observation.

It is a separate Cargo workspace on purpose:

- the root driver workspace and `hil/esp32s31` remain source-only;
- normal builds, tests, metadata, and publication do not resolve vendor PHY
  packages;
- the vendor and fully open drivers are never linked into the same firmware
  as competing MAC owners;
- execution is opt-in through `cargo hil oracle ...`.

The checked-in source records each wrapped blob/ROM boundary. Binary evidence
stays in the ignored repository-local `_oracles/` directory and is validated
against `oracles.lock` before the oracle is built.
