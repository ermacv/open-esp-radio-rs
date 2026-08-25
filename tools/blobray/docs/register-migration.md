# Register-model migration

The migration goal is a reproducible generated/imported base plus small
reviewed overlays. A controller project should grow with accepted facts, not
with copied MMIO inventories or regenerated SVD-shaped TOML.

## Read-only plan

Run the target-neutral planner against the reusable model and every reviewed
pack selected for the migration:

```console
cargo run -p open-esp-radio-register-model \
  --bin register-model-migration -- \
  verification/vendor/chips/esp32s31/registers/device.toml \
  verification/vendor/targets/esp32s31/reviewed/ieee802154.toml
```

The default output is bounded to the summary, sparse assertions and blocking
diagnostics. Add `--details` before the model path to include the deterministic
classification of every base entity. Redirecting the report is optional; the
tool itself never writes the model or review packs.

Semantic diagnostics are intentionally scoped to physical subjects already
selected by a sparse register assertion. They answer whether that proposed
migration is complete; they are not a claim that every other base property has
been reviewed. The base classification and its `review-required` count provide
the broader migration backlog.

The plan distinguishes:

- `imported-base`: exact provenance imported from a named source; its
  completeness still limits which properties that source proves;
- `generated-base-candidate`: exact observations suitable for regenerating
  geometry only;
- `review-required`: derived, approximate, hinted, reviewed-in-base or missing
  provenance that cannot be moved automatically;
- `embedded-reviewed-fact`: a sparse assertion whose value is still duplicated
  by the base;
- `sparse-reviewed-fact`: an assertion that already changes or materializes the
  reusable base.

Every sparse record in the plan retains pack ID, classification, effective
applicability, evidence and note. The review-knowledge fingerprint makes two
plans comparable and prevents silently planning from a different pack set.

`embedded-reviewed-fact` is a migration candidate, not permission to rewrite
the model. Remove or replace one base property, reapply the overlay, and accept
the change only if the effective SVD remains byte-identical. Declarations,
arrays, address-space mismatches and conflicting applicability continue to fail
closed through normal model composition.

## Properties that remain human decisions

The planner never derives any of the following from vendor reads, writes, RMW
sequences, symbol spelling or neighboring registers:

- peripheral/register/field names and descriptions;
- read/write access, reset state or concurrency behavior;
- W1C, W1S, self-clear, trigger or hardware-owned-bit semantics;
- region ownership or missing geometry.

Those properties need exact sparse assertions with evidence. A vendor helper
that writes a status mask is compatible with W1C, but is not proof of W1C.
Until authoritative documentation or controlled HIL establishes the contract,
use `hardware-write-semantics = "unknown"` and keep access as explicit review
debt.

## Safe iteration

1. Generate the migration plan and save the current effective SVD.
2. Choose one `embedded-reviewed-fact`; do not batch unrelated semantics.
3. Replace only its base representation with a deterministic non-semantic
   placeholder, or remove the optional property.
4. Re-run the plan. The fact must become `sparse-reviewed-fact` and its evidence
   and applicability must be unchanged.
5. Run the target's SVD `--check`, register validation, unit tests, formatting
   and clippy. Reject the migration if effective output changes.

For ESP32-S31 the first isolated migration moves the name at
`mmio:cpu:0x20103064/32` out of the reusable base: the base now calls the word
`WORD_064_BASE`, while the project overlay supplies reviewed `EVENT_STATUS`.
The checked-in SVD remains byte-identical after composition. The planner still
reports the base `read-only` access and prose description as review-required;
neither was moved or reinterpreted, and W1C remains explicitly unknown.
