# Verification

Verification is the project workflow that compares source-qualified vendor
functions with compiled Rust implementations. It intentionally keeps four
concerns separate:

| Concern | Document |
| --- | --- |
| Source inventory, pairing, verdicts and gates | this document |
| Profiles, concrete scenarios and branch coverage | [Execution and profiles](execution-and-profiles.md) |
| Dispositions, bindings and effect contracts | [Verification contracts](verification-contracts.md) |
| Schema-v6 reports, schema-v2 baselines and evidence review | [Verification evidence](verification-evidence.md) |

## Commands

`project verify` executes every configured suite and is the authoritative
project result. `verify source` and `verify inventory` are focused leaf tools;
`verify evidence` reviews a persisted inventory report without loading
proprietary artifacts.

```console
cargo vendor-binary-workbench-esp32s31 verify source \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --vendor-artifact "$ESP32S31_ROM_ELF" \
  --rust-artifact target/verification/esp32s31-probes/riscv32imafc-unknown-none-elf/release/open-esp-radio-verification-esp32s31-probes-elf

cargo vendor-binary-workbench-esp32s31 verify inventory \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --run-spec /path/to/authenticated.toml \
  --gate regression --match-floor 104 \
  --json-report /tmp/esp32s31-verification.json

cargo vendor-binary-workbench-esp32s31 project verify \
  --project verification/vendor/targets/esp32s31/vendor-project.toml

mkdir -p /tmp/evidence-candidates
cargo vendor-binary-workbench-esp32s31 project verify \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --candidate-evidence-dir /tmp/evidence-candidates

cargo vendor-binary-workbench-esp32s31 project verify --check \
  --project verification/vendor/targets/esp32s31/vendor-project.toml

cargo vendor-binary-workbench-esp32s31 project check \
  --project verification/vendor/targets/esp32s31/vendor-project.toml
```

`project check` is the single CI gate when analysis evidence, accepted
behavioral baselines and publication outputs must all reproduce. It delegates
to the same three typed workflows in check mode and never updates evidence.

Candidate generation never promotes or overwrites accepted baselines. It
writes one deterministic `<suite-id>.toml` into an existing review directory;
the reviewer must compare identities, proof classes and hashes before copying
accepted rows into the project.

These examples use the ESP32-S31 Cargo alias because the selected platform
pack requires its compiled harness. A generic build now rejects that mismatch
during command resolution, before starting artifact-wide analysis or
verification.

Run-spec paths are relative to the run spec. Explicit CLI inputs override the
same run-spec role. Artifact revision and authenticity remain caller-owned;
the workbench reports content hashes but never substitutes its own trust
policy for the invoking CI job.

Symbol naming conventions are suite data, not generic CLI defaults. Each suite
owns its `rust-prefix` and optional `source-prefixes`. Focused leaf commands
must pass `--rust-prefix`; an omitted vendor prefix selects all named vendor
functions.

Function identity is `(source, symbol)`, not the symbol spelling alone. Thus a
ROM function and an archive function with the same name remain two independent
rows. Binding-v1 probes are selected by their exact declared name. Unbound
probes use the source-aware naming convention and `--rust-prefix` controls
their pairing and orphan accounting.

## Replacement graph

A complete `project verify` also emits one `replacement_graph`. It deduplicates
suite inventories by the project identity `(source, symbol)` and keeps every
suite result as a separate proof edge. A function seen by six libpp suites is
therefore one vendor node with six proofs, not six vendor functions.

Each reviewed edge joins the vendor identity to its canonical Rust component
path, exact compiled probe symbols, disposition, protocol, proof contract and
qualification blockers. The component path is a validated Rust item path and
is the stable component id in schema v1. Conflicting reviewed component,
disposition or protocol assignments across suites fail the project run.

The graph summary deliberately distinguishes production ownership from a
verification-only executable boundary:

- `behavioral_matches`: unique vendor functions with at least one passing proof;
- `production_replacements`: functions with a reviewed production Rust owner;
- `verification_probe_bindings`: functions connected to compiled verification
  probes;
- `production_matches`: passing proofs with a reviewed production owner;
- `probe_only_matches`: passing probes that deliberately remain verification
  boundaries and do not claim production ownership;
- `unmapped_matches`: passing evidence with neither production owner nor probe;
- `implemented_unqualified`: reviewed implementations whose blockers still
  prevent a proof.

Each replacement target also carries `binding_scope = "production"` or
`"verification-probe-only"`. Probe symbol spelling is never used to infer a
production component.

`project status` reads the same summary from the last complete report. This
makes a green regression gate distinct from complete project mapping: a proof
can pass while its production Rust owner is still unknown.

Project-report schema v6 also contains `rust_component_index`. No additional
project configuration owns this data: reviewed component paths still come
from dispositions, Cargo workspace/package roots come from `cargo metadata`,
and suite ELF paths come from the existing run-spec roles. Rust source is
parsed as an AST to resolve functions, methods and types. ELF symbols are
demangled and DWARF inline frames supply compiled file/line evidence, so an
inlined production operation can still join its probe boundary. Source and
compiled statuses remain separate. A source match does not make an effect
proof, and missing compiled evidence remains visible instead of being inferred
from a probe name.

## Gates

The two gates answer different questions:

- `--gate regression --match-floor N` requires no mismatch, incomplete result,
  orphan probe or accepted-evidence regression, and retains at least `N`
  matches. The floor is mandatory.
- `--gate completion` additionally requires a proven replacement for every
  selected vendor function.

The aggregate result contains `MATCH`, `MISMATCH`, `INCOMPLETE`, `UNCOVERED`
and `IMPLEMENTED-UNQUALIFIED` per-function verdicts. A match identifies its
proof class: `symbolic`, `effect-contract`, `scenario`, `state`, or
`composition-state-scenario`. None of the concrete proof classes claims
equality outside its declared input domain.

The engines fail closed on unresolved control flow, calls, tail jumps, MMIO
values, unmapped registers and incomplete branch-outcome coverage. A present
Rust symbol is therefore not sufficient to count as a match.

## Project data

The project normally owns, per suite:

- the source and Rust artifact roles in an untracked local run spec;
- execution profile fragments;
- disposition fragments;
- accepted evidence-baseline fragments;
- the gate, match floor and exact Rust artifact role;
- the target/platform selection and register catalog.

The ESP32-S31 checked example is under
`verification/vendor/targets/esp32s31`. Private artifacts are deliberately not
part of the repository. Synthetic unit fixtures cover parsers, decoders,
policies and report contracts; protected CI supplies authenticated vendor
artifacts for the oracle regression.

No parity exceptions are implicit. A future exception must be a closed typed
rule with exact source, symbol and behavior scope plus tests.
