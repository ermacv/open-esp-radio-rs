# Public source policy

This repository contains only source-authored Rust, tests, build metadata and
provenance documentation.

Included:

- application facade and PAC/HAL/PHY crates;
- explicitly owned `phy_param` transforms and `phy_*.rs` state machines;
- finite register-only leaves currently awaiting PAC/HAL placement;
- generated-code, compiled-symbol and dependency-tree audit tools;
- documentation of observed behaviour, evidence and remaining uncertainty.

Excluded:

- vendor ELF files and static archives;
- disassembly dumps and extracted binary tables;
- generated proprietary headers;
- ROM or vendor ABI bindings;
- `esp-wifi-sys` and hidden runtime dependencies.

References to vendor/ROM symbol names in comments describe provenance and
behavioural comparison only. They are not link dependencies.

Source text is not treated as an API oracle. Verification must not require or
forbid functions by matching their names with regular expressions; public API
shape belongs in compile tests, behaviour in unit/HIL tests, and final-link
constraints in artifact inspection.
