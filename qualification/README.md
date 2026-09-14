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

The first migrated slice is
[`channel-selection-switch`](catalog/esp32s31/wifi-phy.toml), including its
`rf-bb-initialization` dependency, in the ESP32-S31
[Wi-Fi STA program](targets/esp32s31/wifi-sta.toml). Both declarations retain
their previous implementation, host, async, vendor and HIL obligations.

Catalog validation and inventory generation use the same resolver and
evaluator as normal qualification:

```console
cargo qualification catalog check \
  --manifest qualification/targets/esp32s31/wifi-sta.toml

cargo qualification catalog render \
  --manifest qualification/targets/esp32s31/wifi-sta.toml \
  --out target/qualification/catalog/wifi-sta
```

The render writes `capabilities.md` under the selected ignored output
directory. It records the program and catalog schema identities and SHA-256
source identities, then shows declaration origin, reviewed source coverage,
program membership, evidence-derived state and readiness in separate columns.
It is a view of the evaluator result, not another readiness decision or a
tracked snapshot.

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

`catalog check` additionally requires at least one selected catalog, while
`catalog render` writes the ignored derived view after the same validation and
evaluation succeeds.

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
manifest keeps SRAM/PSRAM DMA, descriptor chaining, scatter/gather and cache
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
