# Public source policy

This policy defines which inputs belong in the public source tree and how
production code may use hardware knowledge. Register publication and vendor
comparison retain their own validation rules.

Included:

- application facade and PAC/HAL/PHY crates;
- explicitly owned `phy_param` transforms and `phy_*.rs` state machines;
- reviewed hardware descriptions and typed PAC/HAL operations;
- generated-code, compiled-symbol and dependency-tree audit tools;
- current architecture, API contracts, operating instructions and capability
  limits;
- reviewed machine-readable provenance needed by publication and verification.

Excluded:

- vendor ELF files and static archives;
- disassembly dumps and extracted binary tables;
- generated proprietary headers;
- ROM or vendor ABI bindings;
- `esp-wifi-sys` and hidden runtime dependencies.

Production driver and ordinary HIL builds obey these exclusions. The isolated
vendor-oracle workspace may use caller-supplied vendor dependencies for
comparison; its private inputs and outputs remain outside tracked source.

Audit reports, work plans, experiment diaries and migration histories are not
tracked documentation. Run outputs belong under their owner's ignored output
directory. The [documentation policy](documentation.md) defines the retained
documentation and its ownership.

References to vendor/ROM symbol names in comments describe provenance and
behavioural comparison only. They are not link dependencies.

Source text is not treated as an API oracle. Verification must not require or
forbid functions by matching their names with regular expressions; public API
shape belongs in compile tests, behaviour in unit/HIL tests, and final-link
constraints in artifact inspection.
