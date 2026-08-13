# ESP32-S31 semantic harness

Reviewed RISC-V summaries, vendor execution scenarios, typed PHY state
projections and comparisons against the production Rust driver. This is the
only Workbench provider allowed to depend on the ESP32-S31 PHY implementation.
It is linked by `../../workbench-host/`; the generic Workbench facade has no
dependency on this crate.

This crate is an execution adapter, not a parallel driver implementation.
Stateful probes must call an exact production entry or a shared production
core. Adapter code may construct scenarios, model vendor-only services and
check a compact reviewed relation, but must not reproduce the production
algorithm as a shadow expected-event list. The binding audit enforces the
declared oracle/binding trust ceiling before evidence can be release eligible.
