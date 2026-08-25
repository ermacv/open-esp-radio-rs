# Verification and qualification contract

This document defines the boundary between production code, host contracts,
Blobray verification, HIL execution and product qualification.

## Authorities

| Source | Question it answers | May decide readiness? |
| --- | --- | --- |
| `driver/` owners and tests | Does the current source contain the declared production and host contracts? | Required, not sufficient |
| Blobray `production-trace` | Does concrete vendor execution match the exact compiled production entry under the declared bounded contract? | Supplies vendor evidence only |
| Blobray `shared-core` or static analysis | Does supporting code or a model agree? | No |
| HIL sealed run | Did the current clean production composition pass the declared scenario on hardware? | Supplies HIL evidence only |
| Qualification v3 | Are all required axes and dependencies closed by acceptable current evidence? | **Sole readiness authority** |

No evidence producer imports product-readiness policy. Qualification consumes
their typed outputs and derives the verdict.

Production owners, general host tests and bounded-async contracts are separate
manifest declarations. Every `async-contracts` entry must also be a declared
host test; the evaluator never infers bounded scheduling from unrelated test
coverage.

## Vendor verification path

A vendor comparison is qualification-eligible only when all of these hold:

1. the vendor side is a concrete replay;
2. the Rust binding is `exact-production-entry`, not a generated reference,
   shared production core or verification projection;
3. the reviewed production component resolves to source and compiled symbols;
4. the compiled artifact is fresh relative to production source;
5. the comparison is `match` or an explicitly bounded match;
6. the accepted evidence baseline passes with a reproducible identity;
7. the evidence row is explicitly release eligible and has no blockers;
8. the evaluator worktree is clean and every named production source still
   matches its recorded hash.

`project verify` writes the compact
`verification/vendor/.../evidence/vendor-evidence.json` only for a complete
project run. Qualification follows the selected project's
`verification-addon`, requires the configured index to be that exact output,
checks its command and project identity, validates proof class/status/hash
shape and re-hashes every referenced production source. A partial suite run
cannot replace this index.

The qualification manifest names vendor roots and explicit evidence rows:

```toml
vendor-roots = [
  { source = "archive", symbol = "phy_chip_set_chan" },
]
vendor-evidence = [
  { suite = "phy", source = "archive", symbol = "phy_chip_set_chan" },
]
```

The evaluator derives:

- `qualified` only when every root has current release-eligible
  `production-trace` evidence and no source-only anchor remains;
- `mapped` when reviewed roots or anchors exist but strict evidence is absent;
- `unmapped` when neither exists;
- `not-applicable` only from an explicit reason and with no vendor references.

## HIL evidence path

Qualification manifests name exact scenario requirements rather than dated
narrative files:

```toml
hil-requirements = [
  { scenario = "station-reconnect", minimum-repetitions = 1 },
]
```

The evaluator first checks that every requirement exists in the typed HIL
scenario catalog and is achievable by its declared repetition count. It then
accepts a run only when:

1. `integrity.json` exactly seals the complete file inventory;
2. every size and SHA-256 digest matches;
3. manifest, suite, run directory and target identities agree;
4. the run and required scenarios passed;
5. every required repetition passed;
6. the run was created from a clean tree at the evaluator's current commit.

Historical Markdown records do not enter this decision. A changed commit needs
a new hardware run; no hand-edited `qualified` field exists.

## Normal workflow

1. Implement behavior and named host contracts in `driver/`.
2. Run workspace tests.
3. Produce complete Blobray verification evidence for declared vendor roots.
4. Run the exact HIL scenarios declared by the target qualification manifest
   from a clean commit.
5. Validate and inspect the derived report.
6. Use the strict gate for a release decision.

```console
tools/blobray/scripts/run-limited \
  project verify \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --run-spec verification/vendor/targets/esp32s31/local.toml

cargo qualification evaluate \
  --manifest qualification/targets/esp32s31/wifi-sta.toml \
  --json-report target/qualification/wifi-sta.json

cargo qualification gate \
  --manifest qualification/targets/esp32s31/wifi-sta.toml
```

`validate` is the normal source-tree consistency check. `gate` is intentionally
expected to fail while any required claim remains incomplete.
