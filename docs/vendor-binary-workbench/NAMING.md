# Vendor Binary Workbench naming contract

Status: active. These are the only supported product and project names.

## Product identity

| Surface | Canonical name |
| --- | --- |
| Product | Vendor Binary Workbench |
| CLI | `vendor-binary-workbench` |
| Cargo package | `open-radio-vendor-binary-workbench` |
| Source tree | `tools/vendor-binary-workbench` |
| Project manifest | `vendor-project.toml` |

The product is a project-oriented environment for analysis, reconstruction,
review, publication and verification of compiled vendor binaries. Verification
is one strict workflow within the workbench; it is not the identity of the
whole product.

Removed product names, executable aliases, flat commands and project-manifest
names are rejected. They must not be accepted silently, translated at runtime
or emitted by generated examples.

## Confidence and lifecycle verbs

These verbs are not interchangeable:

| Verb | Meaning |
| --- | --- |
| `discover` / `analyze` | Produce best-effort observations and explicitly bounded candidates from binaries |
| `review` | Join generated analysis with human-maintained names, layouts and decisions |
| `validate` | Check syntax, schema and internal consistency of configuration or a reviewed model |
| `check` | Compare reproducible generated output without writing it |
| `verify` | Compare vendor behavior with an explicit Rust implementation or effect contract |
| `audit` | Reject a forbidden property in a final artifact |
| `qualify` | Make repository-level readiness claims from current implementation and evidence |
| `publish` | Derive clean consumer artifacts from reviewed models |

`qualify` belongs to the repository qualification layer. Workbench internals
that execute a verification contract use `verify` or `evaluate`.

## Data nouns

| Noun | Contract |
| --- | --- |
| observation | Directly decoded or structurally recovered evidence with provenance |
| candidate | An explicitly non-authoritative inference requiring review |
| report | A generated reading view; never an editable source of truth |
| catalog | Reusable independent definitions selected by identity |
| bindings | Project-reviewed mapping from binary structures to catalog entries |
| annotations | Project-reviewed function roles, arguments and context layouts |
| model | Canonical editable representation from which consumer artifacts are derived |
| profile | A named selection, scenario or policy applied to a workflow |
| preset | Reusable composition defaults that do not own project-specific bindings |

Heuristic field or interface candidates are observations or candidates, not
hardware facts. A version number is local to its format and does not imply
that obsolete versions of a different format are accepted.

## Component names

The facade owns the product name. Dependency crates describe their role:

```text
crates/contracts      -> open-radio-vendor-contracts
crates/analysis-model -> open-radio-vendor-analysis-model
crates/semantics      -> open-radio-vendor-semantics
crates/backend-riscv  -> open-radio-vendor-backend-riscv
```

Platform-specific executable adapters remain above generic analysis. Reusable
RTOS, NVS, logging and delay operation definitions are semantic catalogs;
project-specific trampoline anchors, layouts and slots are interface bindings.

## Naming invariants

- New public names describe a domain role, not an implementation accident.
- A removed spelling is tested as invalid instead of retained as an alias.
- Hash-domain separators and generated report identities are versioned when
  renamed; old evidence is not treated as current evidence.
- Historical repository-readiness terms such as `qualification_blockers` may
  remain where they name the current qualification schema, not the workbench.
- Temporary test directories use the `vendor-workbench-*` prefix.
