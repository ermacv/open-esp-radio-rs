# oer-vendor-evidence-shard

The shared evidence shard format used by the qualification evaluator and
verification producers. It belongs to the root workspace in `qualification/`,
whose evaluator records HIL observations as shards and reads both vendor and
HIL evidence.

- `index`: the shard schema, validation, source and coverage-decision currency,
  and the cross-scenario views of a whole evidence directory.
- `store`: the shared shard reader and writer, names and stale-shard selection.

[`verification/evidence`](../../verification/evidence/README.md) re-exports
this API and owns verdict source policy, the producer contract and producer
orchestration. The verification scenario engine, reports, ESP32-S31 scenarios
and IEEE 802.15.4 host stand also link this format directly. Package metadata
keeps its shared verification role and foundation host layer.

```console
cargo test -p oer-vendor-evidence-shard
```
