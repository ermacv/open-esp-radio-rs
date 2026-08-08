# Verification

Verification is the project workflow that compares source-qualified vendor
functions with compiled Rust implementations. It intentionally keeps four
concerns separate:

| Concern | Document |
| --- | --- |
| Source inventory, pairing, verdicts and gates | this document |
| Profiles, concrete scenarios and branch coverage | [Execution and profiles](execution-and-profiles.md) |
| Dispositions, bindings and effect contracts | [Verification contracts](verification-contracts.md) |
| Schema-v4 reports, baselines and evidence review | [Verification evidence](verification-evidence.md) |

## Commands

`verify source` analyzes one vendor source. `verify inventory` combines all
configured sources and is the authoritative project result. `verify evidence`
reviews a persisted inventory report without loading proprietary artifacts.

```console
cargo vendor-binary-workbench verify source \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --vendor-artifact "$ESP32S31_ROM_ELF" \
  --rust-artifact target/verification/esp32s31-probes/riscv32imafc-unknown-none-elf/release/open-esp-radio-verification-esp32s31-probes-elf

cargo vendor-binary-workbench verify inventory \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --run-spec /path/to/authenticated.run \
  --gate regression --match-floor 104 \
  --json-report /tmp/esp32s31-verification.json
```

Run-spec paths are relative to the run spec. Explicit CLI inputs override the
same run-spec role. Artifact revision and authenticity remain caller-owned;
the workbench reports content hashes but never substitutes its own trust
policy for the invoking CI job.

Symbol naming conventions are project data, not generic CLI defaults.
`verification.rust-prefix` supplies the project convention; `--rust-prefix`
overrides it for focused runs. Per-source vendor prefixes are derived from
unambiguous project IR profiles or supplied explicitly with `--source-prefix`.
Standalone verification must pass `--rust-prefix`; an omitted vendor prefix
selects all named vendor functions.

Function identity is `(source, symbol)`, not the symbol spelling alone. Thus a
ROM function and an archive function with the same name remain two independent
rows. Binding-v1 probes are selected by their exact declared name. Unbound
probes use the source-aware naming convention and `--rust-prefix` controls
their pairing and orphan accounting.

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

The project normally owns:

- the source and Rust artifact roles in an untracked local run spec;
- execution profiles;
- the disposition manifest;
- the accepted evidence baseline;
- the target/platform selection and register catalog.

The ESP32-S31 checked example is under
`verification/vendor/targets/esp32s31`. Private artifacts are deliberately not
part of the repository. Synthetic unit fixtures cover parsers, decoders,
policies and report contracts; protected CI supplies authenticated vendor
artifacts for the oracle regression.

No parity exceptions are implicit. A future exception must be a closed typed
rule with exact source, symbol and behavior scope plus tests.
