# Verification and qualification contract

This document is the canonical boundary between the production driver,
Vendor Binary Workbench evidence, HIL records and the qualification ledger.

## What answers which question

| Source | Question it answers | May make the driver ready? |
| --- | --- | --- |
| `driver/` tests | Does production Rust satisfy its local invariants? | Required, not sufficient |
| Workbench `production-trace` | Does a concrete vendor execution have the same ordered observable effects as the exact compiled production entry? | Yes, for the named bounded contract |
| Workbench `shared-core` | Does a compiled probe exercise code shared with production? | No; supporting evidence only |
| Environment model | Does an explicit environment make a comparison executable? | No; it is an input, not an implementation or verdict |
| Workbench `static-analysis` | Do lifted, symbolic or generated traces agree? | No; supporting evidence only |
| HIL record | Did the dated production composition work on hardware? | Required for hardware-qualified axes |
| Qualification ledger | Are all required capability axes and dependencies closed? | **Sole readiness authority** |

Workbench is intentionally broader than the qualification gate. Its analysis,
register discovery, environment models and focused investigations remain useful
for understanding vendor code. They must not be described as product
qualification unless their result reaches the ledger through the strict
production-trace path.

## Strict production-trace path

A vendor comparison is verification-eligible only when all of these hold:

1. the vendor side is a concrete replay;
2. the Rust binding is `exact-production-entry`, not a generated reference,
   shared production core or verification projection;
3. the reviewed production component resolves to source and compiled symbols;
4. the compiled artifact is fresh relative to the production source;
5. the comparison is `match` or an explicitly bounded match;
6. the accepted evidence baseline passes and has a reproducible identity.

`project audit bindings` checks the declaration-side trust boundary.
`project verify` checks behavior and, on a complete run only, writes the
compact `evidence/vendor-evidence.json` index. The index contains hashes and
repository-relative production sources, never private artifact paths or
vendor binaries. A partial `--suite` run updates focused reports but never
replaces this aggregate index.

## Ledger version 2

The ledger does not contain a manually editable `vendor-proof`. It names:

```text
vendor-root archive phy_chip_set_chan
vendor-evidence phy archive phy_chip_set_chan
```

The checker joins that reference to the compact index and derives:

- `qualified` when every declared root has verification-eligible production-trace
  evidence and no source-only anchors remain;
- `mapped` when roots or anchors exist but strict evidence is absent;
- `unmapped` when neither exists.

The checker also follows the project's `verification-addon` reference and
requires the evidence-index path declared by that add-on,
validates its proof class/status/hash shape and re-hashes every referenced
production source. Editing a status bit or changing driver source cannot keep
a capability qualified.

Other axes remain independent. A capability is `proof-ready` only when its own
five axes are terminal; it is `ready` only when all dependencies are also
ready. Workbench feature reports describe evidence closure inside Workbench;
they are not an alternative product-readiness ledger.

## Normal workflow

1. Implement and test behavior in `driver/`.
2. Add the smallest exact production entry that can be compiled and invoked.
3. Add a concrete vendor profile for the same operation and ordered MMIO/RAM
   effects. Do not copy production behavior into a shadow implementation.
4. Run the focused suite while iterating.
5. Run the complete resource-limited project verification to refresh the
   aggregate report and compact evidence index.
6. Review the index diff, then run the qualification checker and HIL cells.

```console
cargo build --profile workbench \
  -p open-radio-vendor-workbench-esp32s31-host \
  --bin vendor-binary-workbench

tools/vendor-binary-workbench/scripts/run-limited \
  project verify \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --run-spec verification/vendor/targets/esp32s31/local.toml

cargo qualification check \
  --manifest qualification/targets/esp32s31/wifi-sta.ledger
```

## Scope control

New Workbench features are frozen unless they directly unblock a named driver
capability, repair a false result, preserve reproducibility, or reduce the
maintenance cost of this workflow. Physical extraction into a separate
repository remains a later task after this contract is stable and the full
check is reproducible.
