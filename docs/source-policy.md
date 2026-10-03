# Public source policy

This policy defines which inputs belong in the public source tree and how
production code may use hardware knowledge. Register publication and vendor
comparison retain their own validation rules.

Included:

- application facade and PAC/HAL/PHY crates;
- explicitly owned `phy_param` transforms and `phy_*.rs` state machines;
- reviewed hardware descriptions and typed PAC/HAL operations;
- necessary recovered hardware tables and calibration coefficients, including
  values recovered from vendor binaries, represented in production source;
- generated-code, compiled-symbol and dependency-tree audit tools;
- current architecture, API contracts, operating instructions and capability
  limits;
- reviewed machine-readable provenance needed by publication and verification.

Excluded:

- vendor ELF files and static archives;
- disassembly dumps and unreviewed extraction artifacts;
- generated proprietary headers;
- ROM or vendor ABI bindings;
- `esp-wifi-sys` and hidden runtime dependencies.

Production driver and ordinary HIL builds obey these exclusions. The isolated
vendor-oracle workspace may use caller-supplied vendor dependencies for
comparison; its private inputs and outputs remain outside tracked source.

## Recovered tables and coefficients

Required hardware data belongs in the implementation that consumes it. Its
origin in a binary is not a reason to exclude it, replace it with an older
table, approximate it, or defer an otherwise supported implementation.
Reviewed numeric tables and coefficients may be committed and used directly
by production and ordinary HIL builds. This permission applies to existing
and newly recovered data; it does not require a separate approval per table.

For each recovered set, record its source identity (revision/hash and symbol
or section), purpose, representation and applicable hardware/profile beside
the definition or in reviewed provenance. State unknown physical units
explicitly. Retain exact numerical behavior and validate the consuming
production code against the real source artifact; do not assert equivalence
by comparing a table with a duplicate fixture. Preserve applicable attribution
and license notices.

Every vendor function a `SOURCE:` block, ROM function summary
(`platform/<chip>/linker/rom/functions.toml`), register-model evidence source or
register or field description names is registered with the
relocation-normalized code fingerprint of the revision its facts were reviewed
against (`verification/<chip>/facts/provenance.toml`). A pin update
that changes or removes such a function fails `cargo xtask check provenance --chip <chip>`;
`cargo xtask vendor-diff --chip <chip>` shows what changed, and after the facts follow the
pinned code, `cargo xtask vendor-provenance --chip <chip> --accept` records its fingerprint.

A `SOURCE` block names the chips whose pinned artifacts its facts describe in
parentheses directly after the marker, before any bracketed evidence tag:
`SOURCE(esp32s31):` or `SOURCE(esp32s31,esp32c5)[EVIDENCE_ID]:`. Each name must
be a chip with a `verification/<chip>/artifacts.toml`. A block under a chip's
own directory belongs to that chip and may omit the list; naming only other
chips there is a violation. A block under a chip-neutral path that cites a
function of a chip's pins must name its chips, and the check of each chip
counts only the neutral blocks that name it. A neutral block that cites no
vendor function, such as a standard or a HIL record, needs no list. A word
directly followed by `.o` is an archive member name, not a cited function.

Raw archive/ELF files, bulk section dumps, disassembly and unreviewed extraction
outputs remain private oracle inputs or ignored reports. A reviewed table
expressed as named source constants is production hardware knowledge, not an
excluded dump. This distinction is the canonical interpretation of source
policy throughout the repository.

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
