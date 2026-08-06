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

[registers.review]
output = "generated/reports/register-review.md"
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
```

All paths are relative to `vendor-validator.toml`. PAC target is `none` or
`riscv`; edition is `2021` or `2024`. Command-line `--output`, `--target` and
`--edition` override these defaults. Review reports are generated and should
normally stay under an ignored `generated/` directory.

The old `overlay = "..."` spelling remains accepted for schema 1 projects,
but new projects should use `model` and schema 2.

## Starting from an existing SVD

Import is a one-shot operation and refuses to overwrite an existing model:

```console
cargo vendor-code-validator registers import-svd \
  --project verification/vendor/targets/esp32s31/vendor-validator.toml \
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

Target-specific extensions therefore belong in a separate add-on consumed by
the target code generator.

## Starting from discovered MMIO

First generate facts from local artifacts and project memory-map ranges:

```console
cargo vendor-code-validator mmio discover \
  --project verification/vendor/targets/esp32s31/vendor-validator.toml \
  --run-spec /path/to/local.run
```

Then create an empty reviewed model with one peripheral fragment per discovery
range:

```console
cargo vendor-code-validator registers init-model \
  --project verification/vendor/targets/esp32s31/vendor-validator.toml
```

Facts are not copied into the reviewed model. A discovered address remains
unreviewed until a user gives it a hardware identity in a fragment. This avoids
turning a generated placeholder name into an accidental public PAC API.

Generate the manual review queue after discovery and whenever facts or model
fragments change:

```console
cargo vendor-code-validator registers review \
  --project verification/vendor/targets/esp32s31/vendor-validator.toml
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

The basic report needs only `mmio discover` facts. Optional schema-v30
`ir export` JSON reports add evidence that the artifact-wide MMIO pass does not
carry: poll masks, direct branch predicates, producer-return chains and links
from register bits to guarded semantic actions.

Generate one project report or several focused reports, then either list them
under `[registers.review].linked-ir` or pass them explicitly:

```console
cargo vendor-code-validator ir export \
  --project verification/vendor/targets/esp32s31/vendor-validator.toml \
  --run-spec /path/to/local.run \
  --symbol-prefix phy_ --include-reachable \
  --json-report verification/vendor/targets/esp32s31/generated/findings/phy.ir.json

cargo vendor-code-validator registers review \
  --project verification/vendor/targets/esp32s31/vendor-validator.toml \
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
cargo vendor-code-validator registers validate \
  --project verification/vendor/targets/esp32s31/vendor-validator.toml \
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

The default `release` profile exports only reviewed hardware metadata:

```console
cargo vendor-code-validator registers export-svd \
  --project verification/vendor/targets/esp32s31/vendor-validator.toml
```

It contains no discovery placeholders, artifact paths, function names,
`SOURCE`, `CONFIDENCE` or validator vendor extensions. With a legacy schema 1
overlay, unreviewed observations are excluded from release output. The legacy
diagnostic form remains available as `--profile audit`.

Use `--deny-unreviewed` to make incomplete fact coverage an error. CI can
verify a checked output without rewriting it:

```console
cargo vendor-code-validator registers export-svd \
  --project verification/vendor/targets/esp32s31/vendor-validator.toml \
  --check --deny-unreviewed
```

The validator loads schema 2 project models directly as register catalogs.
Analysis does not need the generated XML to exist, avoiding a bootstrap cycle.
Additional read-only SVD catalogs may remain in the project-level `svd = [...]`
list.

## Rust PAC generation

Generate deterministic, formatted `svd2rust` source from the same in-memory
release SVD:

```console
cargo vendor-code-validator registers generate-pac \
  --project verification/vendor/targets/esp32s31/vendor-validator.toml
```

And verify it in CI:

```console
cargo vendor-code-validator registers generate-pac \
  --project verification/vendor/targets/esp32s31/vendor-validator.toml \
  --check
```

The tool uses its pinned `svd2rust` library and `rustfmt`; it does not depend on
a separately installed `svd2rust` executable. This command intentionally emits
the generic PAC only. Chip-specific safe helpers, ownership APIs and other
extensions remain the responsibility of a target add-on.

For ESP32-S31, the clean SVD output is checked at `svd/esp32s31-radio.svd`.
Production `cargo pac-gen` reads the same schema-2 model through the shared
`tools/register-model` crate, validates the target-owned
`registers/pac-addon.xml`, and appends its safe helper API to the generic
svd2rust output. The add-on is never embedded into the clean SVD.

## Legacy schema 1 overlay

`registers init-overlay` and schema 1 overlays remain supported for existing
projects. They use:

```toml
[registers]
facts = "generated/findings/mmio.json"
overlay = "registers/reviewed.toml"
```

Schema 1 keeps generated facts and reviewed names separate, but it maps one
discovery range to one peripheral and cannot faithfully represent a complete
multi-peripheral SVD. It is a compatibility path, not the format for new
project register databases.
