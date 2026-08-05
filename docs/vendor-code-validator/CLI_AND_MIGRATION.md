# CLI hierarchy and migration

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

The exploratory linked-IR façade is likewise separated into stable model,
identity/diagnostic catalog, call normalization, guarded direct-call tracing,
return provenance, effect extraction, reachable summaries, MMIO field
indexing and pseudo rendering. The `ir export` command owns only orchestration;
input validation and the human, pseudo-Rust and JSON renderers are separate
consumers of the same report.

Shared trace IR now keeps observable/draft events, CFG/input queries,
fail-closed validation and function-level eligibility in separate modules
behind the unchanged `ir::*` façade. Effect Contract follows the same rule:
its closed data model, textual parser and vendor/Rust comparator are separate,
and the comparator uses policy query methods rather than its backing map.

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
