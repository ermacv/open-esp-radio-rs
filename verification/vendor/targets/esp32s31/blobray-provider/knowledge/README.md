# ESP32-S31 investigation knowledge overlay

This crate owns reviewed lifting knowledge whose current evidence is bounded
to the ESP32-S31 vendor investigation: exact symbol/body identities, linked
addresses and relocation schemas, registered archive-table assumptions and
the combined RISC-V harness. The harness first applies the reusable rev0 chip
add-on, then applies these exact project summaries.

It must not depend on the production PAC, HAL, PHY, MAC, driver, HIL, or
qualification ledger. Those dependencies belong to the generic verifier or
to qualification tooling.

See [`../OWNERSHIP.md`](../OWNERSHIP.md) for the per-module promotion audit and
the reasons exact ROM-looking summaries remain fail-closed in this overlay.
