# ESP32-S31 vendor verification harness data

Public ABI and lifecycle fixture data for the ESP32-S31 verification harness:
external callback tables, mutable pointer-cell entry states and ROM function
table bindings. It depends only on `open-radio-vendor-contracts` and does
not load or authenticate vendor artifacts. This crate is owned by the target
project, not by the generic Workbench repository.
