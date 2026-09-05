# Qualification v4

Qualification is the sole readiness authority for a supported product path.
The checked-in TOML manifests declare capability roots, dependencies, required
evidence and known blockers. The evaluator lives in `evaluator/`. Programs
explicitly declare implementation, host and async states; vendor and HIL
states are derived from independent evidence.

The ESP32-S31 Wi-Fi, Bluetooth LE and IEEE 802.15.4 programs are independent:

```console
cargo qualification validate \
  --manifest qualification/targets/esp32s31/wifi-sta.toml

cargo qualification validate \
  --manifest qualification/targets/esp32s31/bluetooth-le.toml
```

Three commands have deliberately different contracts:

- `validate` rejects malformed manifests, unsafe or stale references, invalid
  dependency graphs, mismatched verification inputs and corrupt HIL bundles;
  an incomplete target is still a valid development state;
- `evaluate` emits the same derived verdict and optionally a complete JSON
  report through `--json-report PATH`;
- `gate` returns non-zero unless every required capability and dependency is
  ready.

There is no `check` compatibility command and no `.ledger` parser.

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

Known `gaps` are planning facts, not editable outcomes. The evaluator also adds
a deterministic derived gap whenever machine evidence is absent.

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
scenario outcome and repetition count. A Markdown narrative under
`targets/esp32s31/records/` remains useful review history but is not proof.
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
[verification and qualification contract](../docs/VERIFICATION_AND_QUALIFICATION.md)
for evidence strength and the release workflow.
