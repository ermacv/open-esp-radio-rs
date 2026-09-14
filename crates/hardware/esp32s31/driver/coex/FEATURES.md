# ESP32-S31 coexistence source capabilities

This page is the stable entry point for coexistence source coverage. The
canonical declarations, exact limitations, reviewed machine provenance and
vendor scenario classifications live in the
[coexistence catalog](../../../../../qualification/catalog/esp32s31/coex.toml).
They describe source ownership, not RF grants, concurrent connectivity or
qualification readiness.

Render the static coexistence inventory from the repository root:

```console
cargo qualification catalog render \
  --catalog qualification/catalog/esp32s31/coex.toml \
  --out target/qualification/catalog/coex-static
```

The static command does not load vendor evidence or HIL runs. There is no
standalone coexistence qualification target. The existing Bluetooth
`coexistence` capability remains the broader product requirement; its reused
diagnostic timer fact does not close missing live request/grant/release,
scheduler, ownership or hardware-evidence boundaries.

See the [whole-radio entry page](../FEATURES.md) for shared ownership, clock
and power lifecycle, and cross-protocol composition limits.

## Internal arbitration hardware and models

Canonical section: `coex-internal-arbitration-hardware-and-models`.

## Coexistence policy and scheduler

Canonical section: `coex-coexistence-policy-and-scheduler`.

## Protocol integration and lifetime

Canonical section: `coex-protocol-integration-and-lifetime`.

## External coexistence

Canonical section: `coex-external-coexistence`.

## Official coexistence scenario scope

Canonical section: `coex-official-coexistence-scenario-scope`. Its `Y`, `C1`,
`X` and `S` records are external vendor classifications, not open-driver source
statuses or HIL evidence.

## Readiness authority

Canonical section: `coex-readiness-authority`. Qualification remains owned by
the existing protocol programs.
