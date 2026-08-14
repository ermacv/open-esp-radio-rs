# ESP32-S31 Workbench knowledge provider

This crate owns reviewed chip-specific lifting knowledge: exact symbol/body
identities, semantic summaries, intrinsic models, and the RISC-V harness that
exposes them to the generic Workbench.

It must not depend on the production PAC, HAL, PHY, MAC, driver, HIL, or
qualification ledger. Those dependencies belong to the generic verifier or
to qualification tooling.
