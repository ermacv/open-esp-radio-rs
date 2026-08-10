# Register model, discovery facts, SVD and PAC generation

The project register workspace separates three kinds of data:

- generated MMIO `facts` record what vendor code actually accessed;
- the versioned `model` is the editable hardware description committed to the
  project;
- CMSIS-SVD, Rust PAC source and audit reports are derived outputs.

SVD is an interchange and code-generation format. It is not the review
database and does not carry artifact paths, function names, discovery notes or
provenance annotations.

## Project configuration

```toml
[registers]
facts = "generated/findings/mmio.json"
model = "registers/device.toml"
owned-ranges = ["radio"]

[registers.review]
output = "generated/reports/register-review.md"
non-operational-functions = ["archive:register_dump"]
linked-ir = [
    "generated/findings/rom.ir.json",
    "generated/findings/libraries.ir.json",
]

[registers.svd]
output = "generated/svd/device.svd"

[registers.pac]
output = "generated/pac/src/lib.rs"
target = "none"
edition = "2024"

[registers.bindings]
output = "generated/svd/device.bindings.toml"
crate-name = "device_pac"

[registers.api]
pack = "registers/api.toml"

[registers.lints]
pack = "registers/lints.toml"

[registers.evidence]
catalogs = ["registers/evidence.toml"]
```

All paths are relative to `vendor-project.toml`. PAC target is `none` or
`riscv`; edition is `2021` or `2024`. Command-line `--output`, `--target` and
`--edition` override these defaults. `crate-name` is the Rust import name
(normally the Cargo package name with `-` replaced by `_`), not a package name.
`owned-ranges` is required and names MMIO regions from the project memory map
that this register model owns and may publish. Discovery still scans every
configured MMIO region. Observations outside this list remain visible as
`ignored` evidence, but they do not become radio-model review debt and do not
block SVD/PAC publication. An unmatched observation inside an owned range
remains `unreviewed` and does block strict publication.

`non-operational-functions` is an optional reviewed policy for diagnostic or
introspection-only code such as a complete register dump. An observation is
classified `non-operational-only` only when every function that reads or
writes it is in this list. Mixed-use addresses remain ordinary operational
review debt, and stale function names are rejected. The raw observation,
functions and access patterns stay in the report; this policy only prevents a
diagnostic sweep from forcing otherwise unused words into the published SVD.
Review reports are generated and should normally stay under an ignored
`generated/` directory.

The optional lint pack is project policy over an otherwise valid hardware
model. For example, a target may forbid filler-style field names without
turning that naming preference into a generic CMSIS-SVD rule:

```toml
schema = 1
forbidden-field-name-substrings = ["PRESERVED"]
```

The register workspace accepts only the schema-2 editable model selected by
`model`. Unknown register configuration keys are rejected.

## Starting from an existing SVD

Import is a one-shot operation and refuses to overwrite an existing model:

```console
cargo vendor-binary-workbench registers import-svd \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --input svd/esp32s31-radio.svd
```

The importer:

- preserves standard device, peripheral, register, cluster, array, field,
  enum, interrupt, access and reset data;
- creates one editable TOML fragment per peripheral;
- writes addresses, offsets and masks as hexadecimal values;
- removes `SOURCE[...]` and `CONFIDENCE[...]` prefixes from hardware
  descriptions;
- retains those annotations as structured `[[review]]` records;
- ignores non-standard XML vendor extensions rather than copying them into a
  supposedly portable SVD model.

Target-specific safe transactions and provenance therefore belong in the
project's separate API and evidence packs. They are validated against the
generic model but are not embedded into the clean SVD.

## Starting from discovered MMIO

First generate facts from local artifacts and project memory-map ranges:

```console
cargo vendor-binary-workbench mmio discover \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --run-spec /path/to/local.toml
```

Then create an empty reviewed model with one peripheral fragment per discovery
range:

```console
cargo vendor-binary-workbench registers init-model \
  --project verification/vendor/targets/esp32s31/vendor-project.toml
```

Facts are not copied into the reviewed model. A discovered address remains
unreviewed until a user gives it a hardware identity in a fragment. This avoids
turning a generated placeholder name into an accidental public PAC API.

Generate the manual review queue after discovery and whenever facts or model
fragments change:

```console
cargo vendor-binary-workbench registers review \
  --project verification/vendor/targets/esp32s31/vendor-project.toml
```

The deterministic Markdown report joins each observed address and width to its
current model identity, read/write counts, catalog name, read users, write
users and full write-pattern bit provenance. Every unmatched observation gets
a copyable TOML register draft. Draft field ranges are the non-overlapping
partition induced by observed partial-write masks; whole-register writes do
not fabricate a field. These ranges are explicitly candidates, not claims
about names, reset state, W1C behavior or hardware completeness. Copy the
relevant draft into the correct `registers/peripherals/*.toml` fragment, fix
its base-relative offset, replace mechanical names and add reviewed semantics.

The report is never an input to SVD or PAC generation. This keeps local
artifact paths and generated placeholder names outside the versioned hardware
model. CI may check that a committed or separately archived report is current
with `registers review --check`; `--output PATH` overrides the configured
destination.

### Linked-IR enrichment

The basic report needs only schema-v5 `mmio discover` facts, including exact
instruction PCs for recovered direct accesses. Optional schema-v40
`ir export` JSON reports add evidence that the artifact-wide MMIO pass does not
carry: poll masks, direct branch predicates, producer-return chains and links
from register bits to guarded semantic actions.

Generate one project report or several focused reports, then either list them
under `[registers.review].linked-ir` or pass them explicitly:

```console
cargo vendor-binary-workbench ir export \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --run-spec /path/to/local.toml \
  --symbol-prefix phy_ --include-reachable \
  --json-report verification/vendor/targets/esp32s31/generated/findings/phy.ir.json

cargo vendor-binary-workbench registers review \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --ir-report verification/vendor/targets/esp32s31/generated/findings/phy.ir.json
```

For a repeated workspace, prefer a configured `[[analysis.ir]]` profile and
`ir build`; the profile output can be the same project-relative path listed in
`linked-ir`. Before its first build, `project doctor` reports that owned output
as `not-generated`; an equally missing external report remains an error. See
[project linked-IR builds](project-ir-build.md).

Repeated `--ir-report` options replace the configured list for that invocation.
Use `--no-ir-reports` to temporarily generate or check the basic report while
leaving configured enrichment in the project manifest; it conflicts with an
explicit `--ir-report`.
Paths in `linked-ir` are relative to the project manifest; explicit
`--json-report` and `--ir-report` paths are relative to the process working
directory, as shown above for a command launched at the repository root.
Equal `(address, width, bit range)` candidates from multiple reports are merged:
shape counts are added and function, predicate and semantic evidence is
deduplicated. Schema, report command, masks and candidate ranges are validated
strictly; a newer incompatible IR schema fails instead of being guessed.

Linked-IR-only addresses are listed separately and never become drafts until
they also exist in current MMIO discovery facts. Semantic operation names are
navigation links showing which recovered actions are guarded by candidate
bits. They are not field names, access rules or proof of hardware behavior.
Draft fields prefer linked-IR boundaries when available and otherwise fall
back to the partial-write masks in MMIO facts. Whole-register masks create no
field candidate in either path.

Validate the model and its coverage against current facts:

```console
cargo vendor-binary-workbench registers validate \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --deny-unreviewed
```

Schema 2 models can be validated and exported before facts exist. In that
case model registers are reported as `manual`, while observed/reviewed coverage
is zero. Once facts are generated, identities are reconciled by absolute
address and access width.

## Multi-file model

`registers/device.toml` holds stable device metadata and a deterministic list
of fragments:

```toml
schema = 2
address-space = "cpu"
fragments = [
    "peripherals/wifi-mac-interrupt.toml",
    "peripherals/wdev-pwr.toml",
]

[device]
name = "ESP32S31_RADIO"
version = "1.0"
description = "Reviewed radio register map"
address-unit-bits = 8
width = 32
svd-schema = "1.3"
svd-schema-location = "CMSIS-SVD.xsd"
```

A peripheral fragment contains standard register semantics and separate review
metadata:

```toml
schema = 2

[[peripherals]]
name = "WIFI_MAC_INTERRUPT"
baseAddress = 0x20104C40

[[peripherals.registers]]
[peripherals.registers.register]
name = "STATUS"
addressOffset = 0x8
size = 32
access = "read-only"

[[peripherals.registers.register.fields]]
name = "RX_SUCCESS"
bitOffset = 14
bitWidth = 1

[[review]]
entity = "WIFI_MAC_INTERRUPT.STATUS.RX_SUCCESS"
sources = ["BLOB_LIBPP_WDEV_PROCESS_FIQ"]
confidence = "instruction-exact"
```

Camel-case names inside peripheral bodies deliberately follow the CMSIS-SVD
data model. Project-only keys such as `address-space`, `fragments`, `review`
and output configuration use kebab-case.

Fragment paths must be safe relative paths. Duplicate fragments, peripheral
names, review identities and register address/width identities are rejected.
Review identities must resolve to a peripheral, interrupt, cluster, register,
field or enumerated value still present in the model. Arrays and clusters are
expanded when model coverage is calculated.

## Clean SVD export

For a configured project, the normal release boundary is `project publish`;
it validates strict review coverage and prepares every configured output
before writing any of them. The individual commands below remain useful for
one-output inspection and explicit overrides. See
[project publication](project-publication.md).

SVD export contains only reviewed hardware metadata:

```console
cargo vendor-binary-workbench registers export-svd \
  --project verification/vendor/targets/esp32s31/vendor-project.toml
```

It contains no discovery placeholders, artifact paths, function names,
`SOURCE`, `CONFIDENCE` or workbench-specific vendor extensions. Generated
observations are review input and never an alternate SVD export profile.

Use `--deny-unreviewed` to make incomplete fact coverage an error. CI can
verify a checked output without rewriting it:

```console
cargo vendor-binary-workbench registers export-svd \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --check --deny-unreviewed
```

The workbench loads schema 2 project models directly as register catalogs.
Analysis does not need the generated XML to exist, avoiding a bootstrap cycle.
Additional read-only SVD catalogs may remain in the project-level `svd = [...]`
list.

Register lifecycle commands share the normal output contract: human coverage
and validation-check tables, or a typed JSON/JSONL report for automation.
Workspace validation nests optional PAC API, lint, memory-map and evidence
summaries inside `register-workspace`. A strict coverage failure is reported as
`status = "unreviewed"` with a non-success exit code.

## Rust PAC generation

### Evidence catalogs

`[registers.toml].catalogs` contains reviewed source descriptions and an
optional controlled confidence vocabulary outside both the hardware model and
safe API policy. Register-model `[[review]]` annotations and API-pack
`sources = [...]` refer to catalog IDs. Validation rejects undefined IDs and
confidence levels.

Catalogs may also record coarse half-open address ranges supported by one or
more sources. These ranges are word-aligned, non-overlapping and must fit
inside an MMIO region from `memory.toml`. They document evidence coverage; they
never create registers or fields. Validation separately proves that every
register in the editable model lies inside the project MMIO map.

That model containment check runs whenever `registers validate` has both a
schema-2 model and project memory map; it does not depend on configuring an
evidence catalog. The full byte width of each register must fit one MMIO
region. This catches a stale base address or a register straddling the end of a
declared peripheral window before SVD/PAC publication.

### Reviewed safe API pack

The optional `[registers.api].pack` is target-owned reviewed policy layered on
top of the generic register model. It declares a small vocabulary of safe
transactions: interrupt sample/ack pairs, full and fixed register writes,
whole-register images, zero-based field writes, zero writes and partitioned
masked read-modify-write operations. For example:

```toml
schema = 1

[options]
peripheral-ownership = true
device-access = true

[[masked-register-modifies]]
name = "publish_command"
peripheral = "RADIO"
register = "COMMAND"
preserve-mask = 0xffff0000
input-mask = 0x0000fff0
set-mask = 0x0000000f
sources = ["REVIEWED_COMMAND_BODY"]
```

The pack does not define addresses, fields, W1C behavior or reset values. Those
remain in the editable register model. Loading the pack proves that every
referenced peripheral/register/field exists in the release SVD and checks the
required access, width, write constraints, enum variants, modified-write
semantics and mask partition. RTOS, NVS, logging and delay semantics do not
belong in this pack.

Generate deterministic, formatted `svd2rust` source from the same in-memory
release SVD:

```console
cargo vendor-binary-workbench registers generate-pac \
  --project verification/vendor/targets/esp32s31/vendor-project.toml
```

And verify it in CI:

```console
cargo vendor-binary-workbench registers generate-pac \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --check
```

The tool uses its pinned `svd2rust` library and `rustfmt`; it does not depend on
a separately installed `svd2rust` executable. A configured API pack is applied
by default. Use `--no-api-pack` for a plain generic PAC, or `--api-pack PATH` to
select an explicit reviewed pack.

For ESP32-S31, the clean SVD output is checked at `svd/esp32s31-radio.svd`.
The project-owned generator and `registers/api.toml` reproduce the complete
production PAC byte-for-byte. Project validation owns provenance, confidence,
evidence ranges, MMIO coverage, platform SVD parsing and structural register
invariants. No project metadata is embedded into the clean SVD. The completed
standalone-generator retirement is documented in
[`history/pac-gen-migration.md`](history/pac-gen-migration.md).

## PAC binding index

The optional binding index joins a physical MMIO address to the exact
svd2rust peripheral, cluster, register and field path generated from the same
release SVD. It is consumed by driver generation; it contains no target
semantics or safe transaction policy.

The generated index is strict schema-2 TOML. Each `[[registers]]` entry owns
its address, access mode, exact SVD identity, svd2rust method path and nested
`[[registers.fields]]` entries. It is deterministic generated output and is
not edited by hand.

```console
cargo vendor-binary-workbench registers generate-bindings \
  --project verification/vendor/targets/esp32s31/vendor-project.toml
```

Use `--check --deny-unreviewed` in CI. `--output` and `--crate-name` override
`[registers.bindings]`. The command validates the Rust crate identifier and
does not require a generated SVD file: both SVD and index are rendered from the
schema-2 model in memory.

Direct SVD, PAC and binding generation emits the typed `svd-publication`,
`pac-publication` or `binding-publication` report in JSON/JSONL mode. These are
the same typed leaf results suppressed and aggregated by `project publish`.

For ESP32-S31 this project-owned command reproduces
`svd/esp32s31-radio.bindings.toml` byte-for-byte.
