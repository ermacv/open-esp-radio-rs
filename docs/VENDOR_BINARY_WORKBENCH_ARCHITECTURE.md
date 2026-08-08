# Vendor Binary Workbench architecture

Status: active. Neutral contracts, shared IR/MMIO, semantic adapter interfaces,
the RISC-V backend, optional ESP32-S31 compiled addon and typed verification
have compile-time crate boundaries. Project workflows and persistent artifact
schemas live outside the CLI, so further backend work does not change frontend
ownership.

## Purpose

Vendor Binary Workbench provides a project-oriented path from compiled vendor
binaries to analysis observations, reviewed models, publishable hardware
descriptions and strict Rust conformance verification. Verification answers
whether an independently written implementation has the same declared
observable behavior as compiled vendor code; exploratory analysis and review
do not inherit that proof strength.

The system does not prove that an input ELF or archive is authentic. The
caller owns download, licensing, revision selection, signatures and digest
checks. The workbench reports the identity of bytes it processed for
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
7. ESP32-S31 PHY verification, state projections and production-driver
   adapters.

This creates false genericity. For example, `artifact` accepts only RISC-V,
the shared IR refers to the RV32 argument ABI, the external ABI registry knows
the ESP32-S31 Wi-Fi OSI table, and verification dispatches directly to
ESP32-S31 driver code. Moving these files into smaller `mod` blocks without
changing dependencies would preserve the architectural problem.

## Implemented migration slice

The first two boundary slices are complete:

- the package and sole CLI are named `vendor-binary-workbench`; removed
  executables and flat command spellings are rejected;
- every invocation loads an explicit target spec and validates the
  architecture/calling-convention pair before reading an artifact;
- target specs select architecture, calling convention and Rust recompilation
  target; reusable platform packs alone select an optional compiled harness;
- an optional caller-owned run spec maps input roles to local paths without
  putting those paths in the target pack or command history;
- shareable project IR profiles own symbol/source selection and generated
  destinations while the run spec remains the only owner of local artifact
  paths; `ir build --check` makes those exploratory views reproducible;
- reusable platform packs compose ABI-compatible harness selection and
  semantic catalogs above generic target/backend analysis; concrete table
  anchors and slot bindings remain in project interface packs;
- schema-versioned project status reports expose configuration, private-input,
  generated-analysis, human-review and publication readiness without making
  those lifecycle concerns backend dependencies;
- the ESP32-S31 project supplies register sources, profiles, dispositions and
  baseline defaults, substantially reducing repeated CLI arguments;
- profiles, dispositions and baselines live under `verification/vendor/targets/esp32s31`, not
  inside the tool;
- artifact paths in private tests come only from explicit environment
  variables; workbench source contains no `_oracles` path;
- embedded vendor artifact, inventory, source and reviewed-body digests were
  removed. Binding manifests no longer authenticate inputs or encode a closed
  vendor-revision enum;
- RISC-V artifact loading, reference CFG construction, code generation,
  execution and image audit are compiled by the standalone
  `open-radio-vendor-backend-riscv` crate;
- the zero-dependency `open-radio-vendor-contracts` crate owns opaque,
  architecture-neutral ABI and entry-contract types;
- `open-radio-vendor-analysis-model` owns architecture-neutral symbolic
  values, observable/reference IR, indexed-MMIO proofs and the SVD-derived
  register catalog;
- `open-esp-radio-register-model` owns the target-neutral editable register
  schema, clean SVD encoder and reusable PAC/evidence publication formats;
- the facade owns generated MMIO facts and the `registers review` join report;
  function names, write-pattern provenance and draft placeholders never enter
  the shared register model or release SVD/PAC;
- `open-radio-vendor-harness-esp32s31` depends only on contracts and owns the OSI
  table version/layout and mutable PHY lifecycle entry contracts;
- architecture-neutral effect-contract, driver-adapter and evidence-source
  interfaces are compiled by `open-radio-vendor-semantics`;
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

The facade owns application workflows, generated-reference compile/re-extract,
profiles, dispositions, reports and frontend dispatch. Compiled addons are a
feature-gated static registry: `--no-default-features` produces a neutral build
without ESP32-S31 production dependencies, while the default
`esp32s31-harness` feature contributes that descriptor. Neither model,
semantic interfaces nor the RISC-V backend depends on the facade, production
driver or a platform harness.

## Target layout

The user-facing command is `vendor-binary-workbench`. There is no executable
alias or alternate flat command surface.

```text
tools/vendor-binary-workbench/
  crates/
    contracts/                 neutral contracts (implemented)
    analysis-model/            shared symbolic/effect IR and MMIO (implemented)
    semantics/                 neutral verification interfaces (implemented)
    backend-riscv/             RV32 + riscv-ilp32 backend (implemented)
    harness-esp32s31/          ABI/lifecycle fixture data (implemented)
    harness-esp32s31-semantic/ reviewed summaries and typed verification
  src/application/             project workflows and frontend-neutral API
  src/artifacts/               persistent evidence schemas and strict readers
  src/cli/                     command grammar, adapters and presentation
  src/orchestration/           generated-reference compile/prove workflow
  src/harnesses/               feature-gated static addon registry
tools/register-model/          editable hardware schema + clean SVD encoder
```

These may initially be workspace crates below one directory. Compile-time
dependencies must point only downwards:

```text
frontends -> application/domain services -> orchestration + registries
               |-> semantic interfaces -> analysis-model -> contracts
               |-> riscv backend -------> analysis-model -> contracts
               |-> esp32s31 ABI fixture --------------------> contracts
               \-> esp32s31 semantic harness -> semantic interfaces
                                                + riscv backend
                                                + esp32s31 ABI fixture
                                                + production PHY
```

`contracts` and `analysis-model` must not depend on an architecture backend, a chip crate,
HIL code, a repository-relative path or a platform target pack. Backends must
not depend on platform harnesses. A harness may depend on the production
driver because comparing its typed state is the harness's purpose.
The register-model crate has the same target-neutral restriction. Target-owned
PAC API and evidence packs may depend on its output schema, but target helper
semantics must not flow back into the generic model.

## Detailed documents

- [Naming contract](vendor-binary-workbench/NAMING.md)
- [Contracts, analysis model, and backend responsibilities](vendor-binary-workbench/CONTRACTS_ANALYSIS_MODEL_BACKEND.md)
- [Platform, CLI, and configuration boundaries](vendor-binary-workbench/PLATFORM_AND_CONFIGURATION.md)
- [CLI hierarchy](vendor-binary-workbench/CLI_AND_MIGRATION.md)

## Enforced invariants

- no `esp32s31` token in contracts, analysis model or architecture backends;
- no production-driver or HIL dependency from contracts/backends;
- no `_oracles` literal in workbench source or unit tests;
- no embedded expected artifact or function-body digest in workbench source;
- every execution/analysis run records an explicit architecture and calling
  convention;
- no target pack contains an input artifact path;
- unsupported ISA, relocation, ABI slot, MMIO behavior or state ownership
  remains fail-closed.
