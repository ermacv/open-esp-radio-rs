# Vendor Binary Workbench

`vendor-binary-workbench` is a project-oriented environment for compiled vendor
binaries and Rust replacements. It can inventory MMIO, export a linked
best-effort IR for manual reverse engineering, maintain reviewed interface,
function and register models, publish SVD/PAC artifacts, generate fail-closed
Rust reference models, execute deterministic scenarios, verify behavioral
contracts, and audit final images.

The only executable is `vendor-binary-workbench`; removed product names and
flat command aliases are intentionally rejected.

The backend reads ELF files and static archives directly with the Rust
`object` crate and decodes RV32IMAC instructions from symbol bytes. It does
not require binutils and does not scan source text for register addresses or
function names.

Repeated analysis should use a project manifest:

```console
cargo vendor-binary-workbench mmio discover \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --run-spec /path/to/local.run
```

The project composes a target, local inputs, an independent memory map, and
optional SVD catalogs. See [project workspace](docs/project-workspace.md).
Create a new generic RV32 workspace with `project init`; its
[bootstrap guide](docs/project-init.md) defines which files are generated,
reviewed, local, and publishable.
Explicit target selection remains available for focused target-pack
development:

```console
cargo vendor-binary-workbench <workflow> <command> \
  --target-spec verification/vendor/targets/esp32s31/target.spec \
  ...
```

The ESP32-S31 target specification selects the architecture, ABI, SVD catalog,
profiles, dispositions, and evidence baseline. Its project-selected platform
pack supplies the harness and reusable semantic catalogs. Vendor artifact paths
and trusted digests stay outside both packs; callers authenticate inputs and
pass them directly or through an untracked project-local `local.run`.

## Workflows

| Workflow | Purpose | Detailed documentation |
| --- | --- | --- |
| `project init` | Create a validated generic project, MMIO map and editable register model | [Creating a project](docs/project-init.md) |
| `project configure` | Attach or verify a reusable platform/harness/semantic composition | [Platform packs](docs/platform-packs.md) |
| `project inputs init` | Validate caller-owned ELF/archive roles and create or check untracked `local.run` | [Creating a project](docs/project-init.md#local-inputs-and-first-analysis) |
| Project configuration | Compose target, inputs, memory regions and SVD catalogs | [Project workspace](docs/project-workspace.md) |
| `project doctor` | Check backend, harness, memory, SVD and local artifact readiness | [Project workspace](docs/project-workspace.md#project-diagnostics) |
| `project status` | Emit phase-based lifecycle and publication readiness as text or stable JSON | [Project status](docs/project-status.md) |
| `project analyze [--check]` | Generate or non-mutatingly verify project-owned symbol/navigation, MMIO, interface, IR, and review evidence | [Project analysis](docs/project-pipeline.md) |
| `project publish` | Strictly validate reviewed registers and write or check configured SVD/PAC/bindings | [Project publication](docs/project-publication.md) |
| `symbols inventory` | Preserve ELF/archive symbol facts and conservative cross-input associations | [Artifact and symbol inventory](docs/symbol-inventory.md) |
| `interfaces discover` | Recover pointer provenance, table-slot candidates and indirect-call sites without assigning platform semantics | [Interface discovery](docs/interface-discovery.md) |
| `interfaces init-pack` / `validate` | Review table layouts and ABI, then bind slots to reusable semantic operations | [Interface packs](docs/interface-packs.md) |
| `functions init-pack` / `validate` / `review` | Review function roles and context layouts, then render a source-like reading view | [Function and context packs](docs/function-packs.md) |
| `mmio discover` | Build a register/access/field-candidate inventory from ELF and archives | [MMIO discovery](docs/mmio-discovery.md) |
| `registers init-model` / `import-svd` / `review` / `validate` / `export-svd` / `generate-pac` / `generate-bindings` | Review discovered addresses and functions, maintain the register model, and derive clean SVD/PAC/binding outputs | [Register workspace](docs/register-workspace.md) |
| `ir export` | Produce linked JSON and pseudo-Rust IR for manual analysis | [Linked function IR](docs/linked-ir.md) |
| `ir build` | Generate or check all project-owned linked-IR profiles | [Project linked-IR builds](docs/project-ir-build.md) |
| `execute run` / `execute compare` | Execute deterministic scenarios and compare ordered effects | [Execution and profiles](docs/execution-and-profiles.md) |
| `reference generate` / `generate-batch` | Generate fail-closed Rust reference programs | [Reference generation](docs/reference-generation.md) |
| `verify source` / `verify inventory` / `verify evidence` | Apply behavioral gates, then review protected-run evidence without rewriting an accepted baseline | [Verification](docs/verification.md) |
| `image audit-targets` | Reject calls into forbidden executable address ranges | This page |
| `inspect` | Inspect artifacts and decoded functions | Run without subcommand options for syntax |
| `tooling completions` / `manpage` | Generate shell integration and roff documentation from the canonical clap grammar | Run `tooling --help` |

Internal ownership and dependency boundaries are described in
[workbench internals](docs/internals.md). The longer-term crate architecture
and migration constraints live in the repository-level
[architecture document](../../docs/VENDOR_BINARY_WORKBENCH_ARCHITECTURE.md).
The product, CLI, schema and lifecycle vocabulary is fixed by the
[naming contract](../../docs/vendor-binary-workbench/NAMING.md).

## Typical reverse-engineering pass

Start by recording artifact and linkage facts:

```console
cargo vendor-binary-workbench symbols inventory \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --run-spec /path/to/local.run \
  --json-report /tmp/esp32s31-symbols.json
```

Then find structurally recoverable callback/function-table use:

```console
cargo vendor-binary-workbench interfaces discover \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --run-spec /path/to/local.run \
  --json-report /tmp/esp32s31-interfaces.json
```

This report keeps relocation/global-symbol associations, pointer-load chains,
slot offsets, call sites, and recoverable argument provenance separate from
RTOS/NVS/logging names supplied by reviewed semantic packs. Interface
validation preserves those concrete sites under each reviewed slot, and the
function review can display the exact caller, instruction, call kind, and
recovered arguments.

Initialize and validate the separate reviewed layer after configuring the
platform pack and `[interfaces].pack` in the project:

```console
cargo vendor-binary-workbench interfaces init-pack \
  --project verification/vendor/targets/esp32s31/vendor-project.toml

cargo vendor-binary-workbench interfaces validate \
  --project verification/vendor/targets/esp32s31/vendor-project.toml
```

Then build an address inventory:

```console
cargo vendor-binary-workbench mmio discover \
  --target-spec verification/vendor/targets/esp32s31/target.spec \
  --artifact rom="$ESP32S31_ROM_ELF" \
  --artifact libphy="$ESP32S31_LIBPHY_ARCHIVE" \
  --range phy=0x20100000..0x20110000 \
  --json-report /tmp/esp32s31-phy-mmio.json
```

Then export the functions and reachable internal callees:

```console
cargo vendor-binary-workbench ir export \
  --target-spec verification/vendor/targets/esp32s31/target.spec \
  --artifact libphy="$ESP32S31_LIBPHY_ARCHIVE" \
  --symbol-prefix phy_ \
  --include-reachable \
  --pseudo-rust /tmp/libphy.pseudo.rs \
  --json-report /tmp/libphy.ir.json
```

Both outputs are intentionally best-effort and explicitly set
`completeness_claim=false`. They are navigation and manual-analysis tools,
not proof that all executable bytes, indirect calls, register semantics, or
high-level types were recovered. Use reference generation and verification for
fail-closed claims.

## Final-image policy

The SVD-independent final-image audit scans all executable sections and rejects
statically resolved `JAL`/`JALR` targets inside forbidden half-open ranges:

```console
cargo vendor-binary-workbench image audit-targets \
  --target-spec verification/vendor/targets/esp32s31/target.spec \
  --artifact target/path/to/runtime-elf \
  --forbid 'radio-api=0x2f800bf0..0x2f8016bc' \
  --forbid 'radio-body=0x2f823c12..0x2f83e6d0'
```

Runtime-loaded function pointers are outside this binary check and must be
covered by platform or effect contracts.

## Development checks

```console
cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy -p open-radio-vendor-binary-workbench --all-targets -- -D warnings
```

Tests use synthetic fixtures and do not require the ignored private-oracle
directory. Private ROM/archive verification remains a separate integration
workflow with authenticated inputs; repository qualification consumes its
reviewed evidence separately.
