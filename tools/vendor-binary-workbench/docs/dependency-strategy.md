# Dependency and analysis-engine strategy

The workbench delegates standard formats, instruction decoding, and standard
algorithms. It owns the meaning of evidence and verification:

```text
object / rv-asm / petgraph
          |
          v
structural facts and reviewed metadata
          |
          v
Scenario + executable models
          |
          v
EffectTrace + coverage + comparator
          |
          v
MATCH | DIFF | INCOMPLETE
```

No third-party decoder, graph library, or solver may directly produce a
verification verdict.

## Adopted now

| Concern | Dependency | Boundary |
| --- | --- | --- |
| ELF, archives, sections, symbols and relocations | `object` | Binary-format facts only |
| RV32 instruction decoding | `rv-asm` | Bytes to decoded instructions; workbench owns semantics |
| SCC and future dominator/reachability algorithms | `petgraph` | Temporary adapter over domain-owned node/edge vectors |
| Executor property tests | `proptest` (dev only) | Generated tests and shrinking; never runtime verification |

`petgraph::Graph` is deliberately not serialized or exposed as linked IR.
The first adapter replaces the local SCC implementation used to identify
recursive call-graph components. CFG blocks, edges, addresses, evidence, and
blockers remain workbench types.

## Add when an owning workflow exists

`addr2line`/`gimli` are now technically unblocked as optional enrichment:
execution reports retain each event producer's PC, symbol and symbol-relative
offset separately from the compared observable effect. DWARF may therefore add
file, line, function and inline-frame labels without guessing from a surrounding
path. The remaining prerequisite is an owning source-enrichment workflow and a
stable presentation model. Missing or stale debug data must degrade to the
existing symbol/address output and cannot change a trace result.

`cargo_metadata` becomes appropriate when the workbench itself invokes Cargo
to build a Rust probe. The build workflow should select the exact executable
from Cargo JSON messages rather than guess target paths. It is unnecessary
while callers already provide explicit ELF paths.

Rust/C++ demangling belongs to the same optional presentation layer. Stable
machine identities continue to use raw linkage names and addresses.

## Optional scenario-solving addon

Z3 is useful only after a stable architecture-neutral `PathCondition` model
exists. It may translate supported 32-bit operations into bit-vector formulas
and propose arguments/MMIO reads for a requested branch outcome. A solver
result is always a `ScenarioSuggestion`; concrete replay by the existing
executor is required before it contributes coverage. Unsupported operations,
timeouts, and model gaps produce no candidate rather than weakening proof.

The schema-v37 bounded suggestion rules therefore precede Z3: they cover
simple equalities and polls without a native solver dependency and establish
the candidate/replay boundary first.

## Deferred until a second ISA

Capstone would replace only instruction decoding, not symbolic or execution
semantics, so it does not simplify the current RV32 backend. SLEIGH/P-code
bridges such as `libsla`, or higher-level translation frameworks such as
Fugue, should be evaluated as a separate experimental backend when a second
ISA makes duplicated lifting costs measurable. The existing RV32 backend
remains the differential oracle during such a spike; no migration should be
done merely to make the backend appear generic.

## Product-owned invariants

The following remain local even if more infrastructure is delegated:

- observable ordered effects and per-read tokens;
- MMIO and device-model ownership;
- runtime callback-table instances and reviewed layout joins;
- coverage, blockers, and fail-closed execution;
- evidence/provenance identities;
- comparison contracts and MATCH/DIFF/INCOMPLETE.
