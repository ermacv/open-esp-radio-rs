# ESP32-S31 PAC generator

`cargo pac-gen` has three deliberately separate inputs:

1. `verification/vendor/targets/esp32s31/registers/device.toml` and its
   peripheral fragments provide the portable register model.
2. `verification/vendor/targets/esp32s31/registers/pac-addon.xml` provides
   target-specific safe transactions, ownership helpers, evidence windows and
   provenance definitions.
3. `svd/esp32s31-platform-radio-deps.svd` is validated as the separate
   validator-only catalog for platform-owned dependencies.

The generator materializes `svd/esp32s31-radio.svd`, validates every add-on
reference against it, runs the pinned `svd2rust`, appends the reviewed helper
API, and writes `driver/chips/esp32s31/pac/src/lib.rs` plus
`svd/esp32s31-radio.bindings`.

Use `cargo pac-gen --check` in CI. It performs the full pipeline without
accepting changes to any generated output.

## Add-on vocabulary

| Group | Generated contract |
| --- | --- |
| `openEspRadioInterruptSnapshots` | Paired status sample and exact W1C acknowledgement |
| `openEspRadioFullRegisterWrites` | Safe complete-width field writes |
| `openEspRadioFixedRegisterWrites` | Writes of reviewed enumerated variants |
| `openEspRadioFixedRegisterImages` | Exact constant register images |
| `openEspRadioRegisterImageWrites` | Caller-supplied complete register images |
| `openEspRadioZeroBasedFieldWrites` | Register images composed from a zero baseline |
| `openEspRadioZeroRegisterWrites` | Explicit zero-image writes |
| `openEspRadioMaskedRegisterModifies` | Reviewed preserve/input/set-mask RMW operations |
| `openEspRadioProvenance` | Source definitions referenced by model review and helper records |
| `openEspRadioAddressWindows` / `EvidenceRanges` | Target validation boundaries, not register names |

Every referenced peripheral, register, field, enumeration, width, access rule
and write constraint is resolved against the clean model. A missing provenance
definition, stale identity or weakened write constraint fails generation.

## Editing workflow

1. Edit portable hardware semantics in the project TOML fragments.
2. Keep evidence IDs and confidence in the fragment's `[[review]]` records.
3. Define new evidence IDs, and only genuinely target-specific helper
   transactions, in `pac-addon.xml`.
4. Run `cargo vendor-code-validator registers validate`, then
   `cargo pac-gen` and `cargo pac-gen --check`.
