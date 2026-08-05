# Vendor code validator architecture

Status: active migration. Neutral contracts, shared IR/MMIO, semantic adapter
interfaces, the RISC-V backend, ESP32-S31 ABI fixture data and ESP32-S31 typed
qualification now have compile-time crate boundaries. The remaining migration
is concentrated in orchestration and proving the interfaces with a second
architecture backend.

## Purpose

The validator answers whether an independently written implementation has the
same declared observable behavior as compiled vendor code. It may inspect,
execute, compare and generate reference code, but those are workflows of one
validation system rather than unrelated modes of a PHY trace utility.

The system does not prove that an input ELF or archive is authentic. The
caller owns download, licensing, revision selection, signatures and digest
checks. The validator reports the identity of bytes it processed for
reproducibility, but it must not contain an allow-list of artifact digests or
silently infer a vendor revision from one.

## Current problem

The original `phy-trace` implementation combined these products in one crate
and one flat CLI:

1. ELF/archive loading, symbol and relocation handling;
2. RV32 instruction decoding and static symbolic analysis;
3. concrete RV32 execution with RAM and MMIO scenarios;
4. Rust reference generation and compilation;
5. effect/profile/disposition verification and evidence reports;
6. final-image direct-target auditing;
7. ESP32-S31 PHY qualification, state projections and production-driver
   adapters.

This creates false genericity. For example, `artifact` accepts only RISC-V,
the shared IR refers to the RV32 argument ABI, the external ABI registry knows
the ESP32-S31 Wi-Fi OSI table, and verification dispatches directly to
ESP32-S31 driver code. Moving these files into smaller `mod` blocks without
changing dependencies would preserve the architectural problem.

## Implemented migration slice

The first two boundary slices are complete:

- the package and primary CLI are named `vendor-code-validator`; `phy-trace`
  is only a compatibility alias;
- commands have hierarchical workflow spellings while legacy flat spellings
  remain accepted;
- every invocation loads an explicit target spec and validates the
  architecture/calling-convention pair before reading an artifact;
- target specs also select a harness and Rust recompilation target explicitly;
- an optional caller-owned run spec maps input roles to local paths without
  putting those paths in the target pack or command history;
- the ESP32-S31 target spec supplies SVD, profile, disposition and baseline
  defaults, substantially reducing repeated CLI arguments;
- profiles, dispositions and baselines live under `validation/esp32s31`, not
  inside the tool;
- artifact paths in private tests come only from explicit environment
  variables; validator source contains no `_oracles` path;
- embedded vendor artifact, inventory, source and reviewed-body digests were
  removed. Binding manifests no longer authenticate inputs or encode a closed
  vendor-revision enum;
- RISC-V artifact loading, reference CFG construction, code generation,
  execution and image audit are compiled by the standalone
  `open-radio-vendor-backend-riscv` crate;
- the zero-dependency `open-radio-vendor-validator-core` crate owns opaque,
  architecture-neutral ABI and entry-contract types;
- `open-radio-vendor-validator-model` owns architecture-neutral symbolic
  values, observable/reference IR, indexed-MMIO proofs and the SVD-derived
  register catalog;
- `open-radio-vendor-harness-esp32s31` depends only on core and owns the OSI
  table version/layout and mutable PHY lifecycle entry contracts;
- architecture-neutral effect-contract, driver-adapter and evidence-source
  interfaces are compiled by `open-radio-vendor-validator-semantic`;
- target-specific reviewed summaries and semantic adapters are compiled by
  `open-radio-vendor-harness-esp32s31-semantic` and supplied to the RISC-V
  backend through an explicit `RiscvHarnessSpec` hook table;
- shared call/reference IR stores a variable-length argument list and a
  generic modeled return word; the 8 register + 8 stack-word layout is owned
  only by the RISC-V backend;
- exploratory linked IR recovers affine caller-memory locations as
  `(ABI argument, byte offset)` context fields, preserves conditional paths,
  and derives RMW write/preserve/forced-bit masks without assigning semantic
  field names;
- semantic-contract and driver-adapter IDs are opaque manifest strings in the
  verifier; their registry and dispatch live in the platform harness.

The facade now registers these crates and owns the generated-reference
compile/re-extract workflow, profiles, dispositions, reports and CLI dispatch.
It no longer compiles ESP32-S31 qualification or depends directly on the
production PHY. Neither model, semantic interfaces nor the RISC-V backend
depends on the facade, production driver or a platform harness.

## Target layout

The final user-facing command is named `vendor-code-validator`. During the
migration the `phy-trace` command remains as a compatibility facade and emits
the same results.

```text
tools/vendor-code-validator/
  crates/
    core/                      neutral contracts (implemented)
    model/                     shared symbolic/effect IR and MMIO (implemented)
    semantic/                  neutral qualification interfaces (implemented)
    backend-riscv/             RV32 + riscv-ilp32 backend (implemented)
    harness-esp32s31/          ABI/lifecycle fixture data (implemented)
    harness-esp32s31-semantic/ reviewed summaries and typed qualification
  src/cli/                     command hierarchy and run manifests
  src/orchestration/           cross-layer compile/prove workflows
  src/harnesses/               thin harness registry
```

These may initially be workspace crates below one directory. Compile-time
dependencies must point only downwards:

```text
facade/cli -> orchestration + registries
               |-> semantic interfaces -> model -> core
               |-> riscv backend -------> model -> core
               |-> esp32s31 ABI fixture --------------> core
               \-> esp32s31 semantic harness -> semantic interfaces
                                                + riscv backend
                                                + esp32s31 ABI fixture
                                                + production PHY
```

`core` and `model` must not depend on an architecture backend, a chip crate,
HIL code, a repository-relative path or a platform target pack. Backends must
not depend on platform harnesses. A harness may depend on the production
driver because comparing its typed state is the harness's purpose.

## Responsibilities

### Core

- opaque external callback-table and function references;
- harness-owned external semantic overlays with opaque operation IDs, C types,
  argument directions and replacement hints;
- immutable ABI table descriptions and return models;
- entry lifecycle, pointer-cell and function-table contracts;
- platform-independent contract lookup by caller-supplied string identity.

Core is deliberately dependency-free. Its identifiers are opaque strings
supplied by configuration. It must not have enums such as
`Esp32s31Eco0Rom`, `Esp32s31WifiOsiV9` or `Esp32s31Channel`.

An external slot may be ABI- and semantics-known while still using the
`Unmodeled` effect model. The backend preserves its call and opaque result in
exploratory IR, but emits a reference blocker. This separation is important:
adding a friendly `rtos.event.post` or `nvs.blob.read` label must never make an
effect-equivalence proof accept scheduler, storage or pointer effects that it
does not model.

Linked IR aggregates those labels into a report-level semantic-boundary index
containing callers, ABI targets and replacement hints. This index is a
migration inventory for manual analysis; it does not weaken the per-function
completeness or reference-eligibility checks.

Project export may aggregate several named primary artifacts. Function IDs are
source-namespaced and report summaries span all inputs, but each primary keeps
an independent address space. The machine-readable `linkage_mode` makes that
boundary explicit. A linked ELF with companions remains the mode for genuine
cross-image address and relocation resolution; resolving undefined symbols
across independent static archives requires a later project linker layer.

As a narrower navigation aid, project IR associates an unresolved call
relocation with a callee only when one exported definition exists across all
named inputs. It ignores local definitions and leaves duplicate global/weak
definitions ambiguous. This produces a `project-linked` call-graph edge but
does not substitute arguments, return values or addresses; the caller therefore
retains its reference blocker and incomplete status.

The exploratory layer may follow those resolved edges to produce a reachable
effect inventory. The fixed-point summary groups MMIO, delay and typed semantic
shapes by the functions in which they were recovered, and identifies recursive
components. It is closed only when every reachable function body and edge is
closed. This propagation is intentionally provenance-only: it neither maps
caller context fields into callee arguments nor turns the inventory into an
effect-equivalence claim.

The linked report also projects reference-flow MMIO into per-function access
shapes and a project-wide `(address, width)` register index. Static accesses,
bounded indexed candidates and poll shapes retain path, address-expression and
write-bit provenance. This connects the manual pseudo-source to the register
inventory without treating candidate sets as dynamic occurrence counts.
Distinct write masks are retained at the register level and split into
contiguous candidate bit ranges linked back to their producing functions.
Whole-register and read-modify-write shapes are counted separately, without
promoting those mechanical masks to a peripheral-semantics claim.

### Shared model

- observable effect IR: memory, MMIO, calls, delays, fences and state ranges;
- symbolic values that do not name physical argument registers;
- affine caller-memory provenance used to recover context-structure offsets;
- SVD/register catalogs;
- draft and resolved reference-control-flow types shared between analysis and
  code generation;
- indexed-MMIO domain proof independent of an instruction set.

Profiles, dispositions, report rendering and workflow scenarios remain in the
facade. Effect-policy and semantic-adapter request/result interfaces live in
the neutral semantic crate; target dispatch and production-driver projections
live in the ESP32-S31 semantic harness.

### Architecture backend

- accepted object architectures and endianness;
- instruction decoding and control-flow classification;
- relocation interpretation;
- register file, stack, call/return and trap semantics;
- supported calling conventions and argument/return locations;
- architecture-specific final-image target discovery.

An architecture and calling convention are selected explicitly as one
validated pair. Initial supported pairs are `riscv32` + `riscv-ilp32`.
Planned pairs are Xtensa `call0` and windowed conventions and Thumb code using
`aapcs32-softfloat` or `aapcs32-hardfloat`. A backend rejects an unsupported
or contradictory pair rather than guessing from a chip name or Rust target
triple.

### Platform harness

- target identity and memory map;
- SVD composition;
- external callback-table descriptions;
- mutable pointer-cell and function-table entry contracts;
- global-state regions and typed projections;
- semantic scenario adapters and production Rust-driver bindings;
- target-specific reviewed summaries that are enabled only when the caller
  explicitly selects this harness.

The ESP32-S31 harness is allowed to say `esp32s31`, `phy_param` and
`g_osi_funcs_p`. Those names are an error in core or the RISC-V backend.
Versions and layouts are data owned by the harness, not variants compiled into
the validator engine.

### CLI/orchestrator

- loads a checked target pack and an optional local run manifest;
- binds input roles to paths supplied by the caller;
- selects an architecture backend and validates its ABI;
- invokes one workflow and renders text/JSON reports.

The CLI does not derive `_oracles` from `CARGO_MANIFEST_DIR`. Proprietary input
paths are supplied as ordinary command options, environment variables in an
external script, or a local untracked run manifest. Checked target packs
contain no proprietary paths.

## Configuration boundaries

A checked target pack contains public validation knowledge:

```text
schema 1
target esp32s31-rev0
harness esp32s31-phy-v1
architecture riscv32
calling-convention riscv-ilp32
endianness little
pointer-width 32
rust-target riscv32imafc-unknown-none-elf
svd esp32s31-radio.svd
svd esp32s31-platform-radio-deps.svd
```

The public target pack does not bind artifacts. A local run spec does:

```text
schema 1
input rom-artifact /absolute/path/to/rom.elf
input archive-artifact /absolute/path/to/linked-vendor.elf
input archive-inventory /absolute/path/to/libphy.a
input rust-artifact /absolute/path/to/rust-probes.elf
```

No expected digest belongs in either validator source code or a binding
manifest. A CI job that requires an exact vendor revision authenticates those
files before invoking the validator. Content digests in generated reports are
descriptive evidence, not an acceptance policy.

Reviewed semantic summaries inherit that trust boundary. Core never chooses a
summary from a vendor digest. The explicitly selected platform harness chooses
its summary by target, symbol and structural identity after the caller has
authenticated the complete input artifact.

## CLI hierarchy

The flat command list is replaced with workflow groups:

```text
vendor-code-validator inspect analyze
vendor-code-validator inspect trace
vendor-code-validator mmio discover
vendor-code-validator ir export
vendor-code-validator reference generate
vendor-code-validator reference generate-batch
vendor-code-validator execute run
vendor-code-validator execute compare
vendor-code-validator verify profiles
vendor-code-validator verify inventory
vendor-code-validator verify contract
vendor-code-validator image audit-targets
```

Common artifact, SVD, architecture and ABI options come from the target/run
manifests. A command accepts only workflow-specific overrides. The dedicated
`qualify-esp32s31-*` commands become named contracts selected from the
ESP32-S31 harness.

The artifact layer exposes two intentionally separate symbol inventories.
Evidence-producing validation/qualification uses only global and weak code
definitions, preserving its reviewed scope. Exploratory `ir export` opts into
all named, sized text definitions, including local/private functions. Resolver
identity is `(archive member, symbol name, object address)`; the presentation
layer adds an address suffix only when two definitions would otherwise have the
same readable identity. Direct ELF targets and archive call relocations share
that canonical catalog. Neither inventory recovers stripped function
boundaries, so the IR report makes no completeness claim.

## Migration order

1. Remove embedded artifact allow-lists and digest-bearing binding directives.
   Keep computed content identities only as report output.
2. Make private-oracle tests use caller-supplied paths and move them out of the
   generic crate's unit-test tree.
3. Introduce `TargetSpec` with explicit architecture, endianness, pointer
   width and calling convention. Reject unsupported combinations.
4. Move ESP32-S31 external ABI tables and entry contracts into a dedicated
   harness-data crate; connect reviewed summaries through backend hooks.
   **Implemented.** Neutral semantic interfaces and typed ESP32-S31
   qualification are separate crates; the facade is only their registry.
5. Group RV32 decoding, reference analysis, code generation, relocations,
   execution and direct-target analysis under the RISC-V backend.
   **Implemented as a standalone Cargo crate.** Platform-specific secondary
   return and summary recognition enter through typed hooks.
6. Extract neutral contract types into core and replace fixed RV32 call
   arguments in shared IR. **Implemented.** MMIO and symbolic/reference IR live
   in the neutral model crate; verification reports remain in orchestration
   until their platform adapter callbacks are separated.
7. Add the hierarchical CLI and run manifest, retain old command aliases for
   one migration window, then remove `phy-trace`.
8. Add a synthetic second backend conformance fixture before claiming the
   core API is architecture-neutral. ARM Thumb is the preferred first proof;
   Xtensa follows once both required calling conventions are modeled.

Each step must leave the existing ESP32-S31 verification result reproducible.
Large source files are split when a responsibility boundary is identified;
line count alone is not an architectural boundary.

The first responsibility-driven source split is also implemented:
`static_analysis` is separated into analysis context, memory/relocations,
stack ABI, polling recovery and trace orchestration; symbolic values are
separated into construction, rewriting, operations and inspection; codegen is
separated into event rendering, control-flow rendering and runtime scaffold.
The large structural and ESP32-S31 oracle tests follow the same functional
phase boundaries.

## Backend feasibility notes

The `object` crate already recognizes RISC-V, Arm and Xtensa ELF machine
identities, so container parsing can be shared. It does not provide instruction
semantics, calling-convention recovery or concrete execution; those remain
backend responsibilities.

ARM Thumb is the preferred second-backend proof because the ABI and Rust bare
metal targets are standardized. The first slice should support one T32 subset,
Arm ELF relocations and `aapcs32-softfloat`, using synthetic fixtures before a
chip harness is added. Hard-float is a separate ABI mode, not a feature bit
that may be silently accepted.

Xtensa follows. Its `call0` and windowed ABIs use different incoming register
locations, and windowed calls rotate the register file. The backend must also
model literal pools, density instructions, `MEMW`, direct/long-call lowering
and the selected Xtensa core configuration. An Xtensa chip harness must not be
added by copying RV32 `a0`/stack assumptions into new match arms.

## Enforced invariants

- no `esp32s31` token in core or architecture backends;
- no production-driver or HIL dependency from core/backends;
- no `_oracles` literal in validator source or unit tests;
- no embedded expected artifact or function-body digest in validator source;
- every execution/analysis run records an explicit architecture and calling
  convention;
- no target pack contains an input artifact path;
- unsupported ISA, relocation, ABI slot, MMIO behavior or state ownership
  remains fail-closed.
