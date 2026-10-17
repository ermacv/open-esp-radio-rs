# Public source policy

This policy defines which inputs belong in the public source tree and how
production code may use hardware knowledge. Register publication retains its
own validation rules.

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

Production driver and ordinary HIL builds obey these exclusions. The vendor
reference firmware (`verification/<chip>/hil-vendor`) links the pinned vendor
archives; its fetched inputs and build outputs remain outside tracked source.

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
register or field description names is cited by the pinned revision and its
symbol, and `verification/<chip>/facts/provenance.toml` records the reviewed
relocation-normalized code fingerprint of each such function at the revision
its facts were reviewed against. After a pin update that changes or removes
such a function, its facts are reviewed again against the pinned code before
its fingerprint is recorded.
A function that several pinned artifacts define (the ROM's copy and a
library's, which can handle the same register bits differently) is cited by
the copy its facts were read from (`libpp[pm.o]::pm_parse_beacon`,
`rom[]::pm_tbtt_process`), never by a bare name.

A `SOURCE` block names the chips whose pinned artifacts its facts describe in
parentheses directly after the marker, before any bracketed evidence tag:
`SOURCE(esp32s31):` or `SOURCE(esp32s31,esp32c5)[EVIDENCE_ID]:`. Each name must
be a chip with a `verification/<chip>/artifacts.toml`. A block under a chip's
own directory belongs to that chip and may omit the list; naming only other
chips there is a violation. A block under a chip-neutral path that cites a
function of a chip's pins must name its chips, and describes only the chips
it names. A neutral block that cites no
vendor function, such as a standard or a HIL record, needs no list. A word
directly followed by `.o` is an archive member name, not a cited function.

Raw archive/ELF files, bulk section dumps, disassembly and unreviewed extraction
outputs remain private oracle inputs or ignored reports. A reviewed table
expressed as named source constants is production hardware knowledge, not an
excluded dump. This distinction is the canonical interpretation of source
policy throughout the repository.

Audit reports, work plans, experiment diaries and migration histories are not
tracked documentation. Run outputs belong under their owner's ignored output
directory.

References to vendor/ROM symbol names in comments describe provenance and
behavioural comparison only. They are not link dependencies.

Source text is not treated as an API oracle. Verification must not require or
forbid functions by matching their names with regular expressions; public API
shape belongs in compile tests, behaviour in unit/HIL tests, and final-link
constraints in artifact inspection.

## Attribution and licenses

The root [`NOTICE`](../NOTICE) holds every attribution: each third-party work
the repository derives from, its license, and the files derived from it, with
the full text of each BSD-3-Clause license. A new derived file is recorded in
the same change:

- **its own file** names the source, its pinned revision and its license
  beside the code; a file ported from an Apache-2.0 work carries the
  modification notice "Ported from … and modified by Vladislav Ermachenkov"
  (Apache-2.0 §4(b));
- **`NOTICE`** gains the file under its source's entry, or a new entry;
- **the crate** becomes `Apache-2.0` when it holds a port of Apache-2.0 code
  (as the ESP-IDF port crates are), and stays `MIT OR Apache-2.0` for
  BSD-3-Clause material, whose notice `NOTICE` keeps; a crate depending on an
  `Apache-2.0` crate states its effective license (the
  [facade](../crates/oer/README.md#license)).

Copyleft code (GPL, such as Linux `mac80211`) is never translated: such a
project may be cited for observed behaviour that a specification also fixes.

### Sources without a license

From a source that has no license, such as `esp32s31-bt-lib`, the repository
records only facts: values, layouts, addresses, order of operations and names.
It records neither code nor verbatim data blocks (whole tables copied as they
are). A verbatim table is replaced by values derived independently, measured
on the stand; where that is not possible, it goes to the owner as a question
and is never kept silently.
