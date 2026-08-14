# ESP32-S31 production verification provider

Vendor execution scenarios, typed PHY state projections and comparisons
against the compiled production Rust driver. Reusable reviewed summaries live
in the sibling `knowledge` crate, which has no production dependency. This is
the only Workbench provider allowed to depend on the ESP32-S31 PHY/MAC
implementation. It is linked by `../../workbench-host/`; the generic Workbench
facade has no dependency on this crate.

This crate is an execution adapter, not a parallel driver implementation.
Stateful probes must call an exact production entry or a shared production
core. Adapter code may construct scenarios, model vendor-only services and
check a compact reviewed relation, but must not reproduce the production
algorithm as a shadow expected-event list. Dispositions declare reviewed
bindings and claim ceilings; compiled artifacts and recorded observations,
not dispositions, are the source of observed behavior.
