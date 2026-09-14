# Qualification v4

Qualification is the sole readiness authority for a supported product path.
The checked-in TOML manifests declare capability roots, dependencies, required
evidence and known blockers. The evaluator lives in `evaluator/`. Programs
explicitly declare implementation, host and async states; vendor and HIL
states are derived from independent evidence.

The ESP32-S31 Wi-Fi, Bluetooth LE and IEEE 802.15.4 programs are independent.

## Capability catalogs and program resolution

Canonical capability declarations may live below `catalog/`. A qualification
program names one or more catalog files with `catalogs` and selects stable IDs
with `catalog-capabilities`. Resolution adds the selected declaration and its
catalog-owned dependency closure to the program before the schema-4 evaluator
runs. Inline capabilities remain supported while domains migrate; a capability
ID cannot be declared both inline and in a loaded catalog.

Every catalog declaration keeps the existing reviewed/evidence axes and may
carry the existing source contracts. Its required `catalog-scope` identifies
the chip, role, PHY, security set, composition, native/lower/composed level,
activation boundary and limitations. This metadata is validated and rendered,
but it is not a readiness axis.

The ESP32-S31 [Wi-Fi/PHY catalog](catalog/esp32s31/wifi-phy.toml) owns all 14
Wi-Fi STA qualification declarations and the wider Wi-Fi/shared-PHY source
inventory. Direct roots and their dependency closure resolve to the same exact
14 required IDs. Source-facet
inventory status and native/lower/composed level are separate from evaluator
axes; unselected inventory entries have no readiness value.

The [Bluetooth catalog](catalog/esp32s31/bluetooth.toml) owns all 68 existing
Bluetooth LE qualification declarations and the wider LE, Classic and Host-only
source inventory. The LE program loads it together with the Wi-Fi/PHY and
[coexistence](catalog/esp32s31/coex.toml) catalogs to reuse the exact
`bluetooth-initial-phy-handoff` and diagnostic
`coex-timer-validation-bridge` source facts. Neither lower fact promotes the
incomplete common-PHY or coexistence lifetime.

The coexistence catalog and the
[whole-radio catalog](catalog/esp32s31/whole-radio.toml) are source-inventory
owners, not qualification programs. They add no capability declaration or
readiness authority. Whole-radio views load the Wi-Fi/PHY and coexistence
catalogs explicitly because they project facts owned there; a missing fact
owner is an error rather than an omitted row.

The [IEEE 802.15.4 catalog](catalog/esp32s31/ieee802154.toml) owns the existing
six-capability radio/MAC program and its wider PHY, CCA, dataplane, filtering,
timing, security, power, coexistence and Host-only source inventory. Its
`ieee802154-registered-timing-entry` and
`ieee802154-mac-operation-subset` facts expose implemented lower operations
without promoting their incomplete RF-ready and public-dataplane parents.

Static catalog validation checks every declaration, dependency graph, source
contract, disposition mapping, HIL scenario reference and inventory path
without loading a vendor evidence index or HIL runs:

```console
cargo qualification catalog check \
  --catalog qualification/catalog/esp32s31/wifi-phy.toml

cargo qualification catalog render \
  --catalog qualification/catalog/esp32s31/wifi-phy.toml \
  --out target/qualification/catalog/wifi-phy-static

cargo qualification catalog render \
  --manifest qualification/targets/esp32s31/wifi-sta.toml \
  --out target/qualification/catalog/wifi-sta

cargo qualification catalog check \
  --catalog qualification/catalog/esp32s31/wifi-phy.toml \
  --catalog qualification/catalog/esp32s31/coex.toml \
  --catalog qualification/catalog/esp32s31/bluetooth.toml

cargo qualification catalog render \
  --manifest qualification/targets/esp32s31/bluetooth-le.toml \
  --out target/qualification/catalog/bluetooth-le

cargo qualification catalog check \
  --catalog qualification/catalog/esp32s31/ieee802154.toml

cargo qualification catalog render \
  --manifest qualification/targets/esp32s31/ieee802154.toml \
  --out target/qualification/catalog/ieee802154

cargo qualification catalog render \
  --catalog qualification/catalog/esp32s31/coex.toml \
  --out target/qualification/catalog/coex-static

cargo qualification catalog render \
  --catalog qualification/catalog/esp32s31/wifi-phy.toml \
  --catalog qualification/catalog/esp32s31/coex.toml \
  --catalog qualification/catalog/esp32s31/whole-radio.toml \
  --out target/qualification/catalog/whole-radio-static

cargo qualification catalog render \
  --catalog qualification/catalog/esp32s31/wifi-phy.toml \
  --catalog qualification/catalog/esp32s31/coex.toml \
  --catalog qualification/catalog/esp32s31/bluetooth.toml \
  --catalog qualification/catalog/esp32s31/whole-radio.toml \
  --catalog qualification/catalog/esp32s31/ieee802154.toml \
  --out target/qualification/catalog/esp32s31-radio-static
```

Static render writes `domain-inventory.md`, `capability-catalog.md`, and
`migration-map.md`. Manifest render additionally writes
`program-inventory.md` after evaluation. The program view records repository
commit/dirty state and configured evidence provenance; a catalog or manifest
hash is source identity, never firmware identity. All outputs are ignored
views, not another readiness decision or tracked snapshot.

The Bluetooth LE program includes legacy and extended roles, connected PHY and
control procedures, security/privacy, periodic advertising and PAwR, Direction
Finding, ISO in both connected and broadcast roles, LE Audio and the named LE
Host integrations. It also requires capacity admission, power lifecycle and
coexistence. Each new scope retains explicit incomplete axes until its own
production composition and evidence exist. The chip's
[feature inventory](../crates/hardware/esp32s31/driver/bluetooth/FEATURES.md#qualification-scope-mapping)
maps these requirements to current source boundaries.

Validate each program from the repository root:

```console
cargo qualification validate \
  --manifest qualification/targets/esp32s31/wifi-sta.toml

cargo qualification validate \
  --manifest qualification/targets/esp32s31/bluetooth-le.toml

cargo qualification validate \
  --manifest qualification/targets/esp32s31/ieee802154.toml
```

Three commands have deliberately different contracts:

- `validate` rejects malformed manifests, unsafe references, invalid
  dependency graphs, mismatched verification inputs and corrupt HIL bundles;
  an incomplete target is still a valid development state;
- `evaluate` emits the same derived verdict and optionally a complete JSON
  report through `--json-report PATH`;
- `gate` returns non-zero unless every required capability and dependency is
  ready.

`catalog check --catalog` and `catalog render --catalog` are static operations
over the complete explicitly loaded catalog set. The `--manifest` check form
also resolves selected IDs and dependencies and enforces the program's exact
`required-capabilities` set, still without reading vendor evidence or HIL runs.
Manifest render then evaluates that same statically validated program and
writes the separate readiness view.

## Declared and derived axes

Every capability has five independent axes:

- `implementation` is the reviewed declaration `complete` or `incomplete`;
- `host` is the reviewed declaration `covered` or `incomplete`. The evaluator
  checks consistency with declared gaps; it does not find or run Rust tests.
  Workspace testing remains a separate repository check;
- `vendor` is derived from Blobray's complete compact evidence index. Only a
  fresh, baseline-accepted, release-eligible `production-trace` for every
  declared root evaluated from a clean worktree can qualify the axis;
- `hil` is derived from immutable schema-2 HIL bundles. Every required scenario
  must pass with enough repetitions in a sealed run from the exact current
  clean commit;
- `async` is the reviewed declaration `bounded`, `incomplete`, or explicitly
  `not-applicable` with a reason. Consistency with gaps is checked; the
  evaluator does not infer executor behavior from source names.

`proof-ready` means all five axes are terminal. `ready` additionally requires
every dependency to be ready. `required-capabilities` must exactly equal the
manifest capability set, preventing a mismatch between required roots and declared capabilities.
Changing the declared program still requires review; validation cannot prove
that a removed capability was unnecessary.

Known `gaps` declare reviewed blockers rather than override outcomes. The
evaluator also derives a gap whenever required machine evidence is absent.

## Source contracts

A capability may attach `[[capabilities.source-contracts]]` reference entries
to describe its hardware-facing implementation paths. The ESP32-S31 Wi-Fi
catalog keeps SRAM/PSRAM DMA, descriptor chaining, scatter/gather and cache
handoff boundaries under `rx-tx-dma`. Cold calibration/cache, Bluetooth DTM and
PHY lifetime, coexistence, and IEEE 802.15.4 lower operation contracts likewise
make implemented subsets visible without promoting their broader parents.
The [inventory agreement contract](../docs/verification-and-qualification.md#agreement-with-feature-inventories)
requires matching implementation descriptions for the same scope.

Each entry has a unique `id`, an explicit `scope`, `limits`, repository-relative
`source-paths`, and a `composition`:

- `production`: the production owner composes the operation in the stated scope;
- `diagnostic`: an executable diagnostic composition exists;
- `unimplemented`: the stated composition has no implementation. This says
  nothing about whether the hardware could support it.

These declarations describe reviewed source coverage. Unknown hardware
reachability and ownership limits stay explicit in `limits`; source references
do not establish hardware support or measured performance. The evaluator checks
unique IDs, required descriptions and unique regular source files within the
repository, and preserves the entries in each JSON capability's
`source_contracts` array. It does not infer implementation from file contents.
Entries are optional reference metadata and do not change the five readiness
axes, evidence requirements or capability dependency graph.

## Evidence ownership

```text
reviewed capability declarations ─┐
Blobray vendor evidence index ─────┼─> qualification evaluator ─> JSON/verdict
sealed HIL run bundles ────────────┘
```

Blobray and the HIL runner never decide product readiness. Blobray owns vendor
comparison truth; the HIL runner owns hardware execution truth; qualification
maps both into the declared capability graph.

The HIL runner writes bundles below `target/hil/<target>/runs/<run-id>/`.
Qualification independently checks `integrity.json`, every indexed file hash,
manifest/suite identity, clean repository provenance, commit equality,
scenario outcome and repetition count. Markdown reports are not proof inputs.
A generated run directory without a manifest and an unsealed bundle whose
manifest is still `running` are incomplete mutable execution state and are
ignored as evidence. The former is counted as `hil-incomplete` in console
output and `evidence_inputs.hil.incomplete` in schema-4 JSON reports;
`hil-directories` counts every entry while `hil-bundles` counts only entries
that have published a manifest. An existing malformed manifest still fails
validation, and completed or interrupted bundles must have a valid integrity
seal and fail closed otherwise.

Use a JSON report for CI and downstream presentation:

```console
cargo qualification evaluate \
  --manifest qualification/targets/esp32s31/wifi-sta.toml \
  --json-report target/qualification/wifi-sta.json
```

The console `INPUT` row and JSON `evidence_inputs` object expose how many
verification rows and HIL directories were observed, how many are incomplete
or current, and whether a dirty evaluator worktree prevented otherwise valid
evidence from entering the verdict.

See the canonical
[verification and qualification contract](../docs/verification-and-qualification.md)
for evidence strength and the release workflow.
