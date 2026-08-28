# Qualification v3

Qualification is the sole readiness authority for a supported product path.
The checked-in TOML manifests declare capability roots, dependencies, required
evidence and known blockers. They never declare proof outcomes.

The ESP32-S31 Wi-Fi and Bluetooth programs are independent:

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

## Derived axes

Every capability has five independent axes:

- `implementation` is complete only when all named public production owners
  resolve under `driver/*/src` and no implementation blocker remains;
- `host` is covered only when all named driver test functions resolve and no
  host blocker remains. Test execution is still enforced by the workspace test
  job; source discovery alone is not a test-run attestation;
- `vendor` is derived from Blobray's complete compact evidence index. Only a
  fresh, baseline-accepted, release-eligible `production-trace` for every
  declared root evaluated from a clean worktree can qualify the axis;
- `hil` is derived from immutable schema-2 HIL bundles. Every required scenario
  must pass with enough repetitions in a sealed run from the exact current
  clean commit;
- `async` is bounded only when at least one `async-contracts` reference names a
  declared host test and no async blocker remains, or is explicitly not
  applicable with a reason.

`proof-ready` means all five axes are terminal. `ready` additionally requires
every dependency to be ready. `required-capabilities` must exactly equal the
manifest capability set, so deleting a difficult node cannot improve the
summary.

Known `gaps` are planning facts, not editable outcomes. The evaluator also adds
a deterministic derived gap whenever machine evidence is absent.

## Evidence ownership

```text
driver owners/tests ───────────────┐
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
An unsealed bundle whose manifest is still `running` is mutable execution
state and is ignored as evidence; completed and interrupted bundles must have
a valid integrity seal and fail closed otherwise.

Use a JSON report for CI and downstream presentation:

```console
cargo qualification evaluate \
  --manifest qualification/targets/esp32s31/wifi-sta.toml \
  --json-report target/qualification/wifi-sta.json
```

The console `INPUT` row and JSON `evidence_inputs` object expose how many
verification rows and HIL bundles were observed, how many are current, and
whether a dirty evaluator worktree prevented otherwise valid evidence from
entering the verdict.

See the canonical
[verification and qualification contract](../docs/VERIFICATION_AND_QUALIFICATION.md)
for evidence strength and the release workflow.
