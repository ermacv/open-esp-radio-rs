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

- the package and primary CLI are named `vendor-code-validator`; the obsolete
  `phy-trace` compatibility alias has been removed;
- commands have hierarchical workflow spellings while legacy flat spellings
  remain accepted;
- every invocation loads an explicit target spec and validates the
  architecture/calling-convention pair before reading an artifact;
- target specs also select a harness and Rust recompilation target explicitly;
- an optional caller-owned run spec maps input roles to local paths without
  putting those paths in the target pack or command history;
- the ESP32-S31 target spec supplies SVD, profile, disposition and baseline
  defaults, substantially reducing repeated CLI arguments;
- profiles, dispositions and baselines live under `verification/vendor/targets/esp32s31`, not
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
- `open-esp-radio-register-model` owns the target-neutral editable register
  schema and clean SVD encoder used by both validator projects and `pac-gen`;
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

The user-facing command is named `vendor-code-validator`. The earlier
`phy-trace` compatibility facade has completed its migration window and is no
longer part of the workspace command surface.

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
tools/register-model/          editable hardware schema + clean SVD encoder
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
The register-model crate has the same target-neutral restriction. Target PAC
add-ons may depend on its output schema, but target helper semantics must not
flow back into the generic model.

## Detailed documents

- [Core, model, and backend responsibilities](vendor-code-validator/CORE_MODEL_BACKEND.md)
- [Platform, CLI, and configuration boundaries](vendor-code-validator/PLATFORM_AND_CONFIGURATION.md)
- [CLI hierarchy and migration](vendor-code-validator/CLI_AND_MIGRATION.md)

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
