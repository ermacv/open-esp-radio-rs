# oer-vendor-evidence

The native vendor-comparison evidence index of a chip
(`verification/<chip>/evidence/scenarios`): one JSON shard per scenario, so
each scenario run rewrites only its own file. Producers write it, `cargo
verification evidence` keeps it current, and the qualification evaluator reads it.
The shared data contract and store live in
[`oer-vendor-evidence-shard`](../../qualification/evidence-shard/README.md), a root-workspace format
package. This verification workspace library owns comparison policy and
producer orchestration. Qualification reads the format directly.

| Module | Owns |
| --- | --- |
| `index` (re-exported at the root) | The shard format `Index` (schema, producer `command`, claims, coverage, observation, state, untriaged locations), `validate`, `currency`/`is_current` against the recorded source digests and coverage decisions, `digest_source`, `Evidence` (a whole directory, strict) and `untriaged` (locations no scenario covers) |
| `store` | The one reader and writer of shard files: `write`, `read`, `names`, `shards`, `stale` and `records_any` (whether a change touches a shard) |
| `policy` | The verdict source policy: `closure` (the path packages a shard may record, report packages apart), `report_packages`, `check_verdict_sources` and `reject_report_sources` for committed shards |
| `diff` | What two versions of one shard say differently: claims line by line, recorded sources counted |
| `producer` | The `Producer` contract and the host stands: `host_stand::stands` (a package below `verification/<chip>/host/` owns the shard `<directory>-host`) and `host_stand::shard`, the shard a stand writes from its own `shard` command |
| `run` (feature `producers`) | The producers' orchestration: `probes` (each chip's Rust comparison probe images, built and their catalogs validated: `cargo verification probes`), `scenario` (the typed Blobray scenarios built in the Blobray workspace and run: `cargo verification scenario`), `regenerate` (which producer reruns which stale or named shard, the rerun check, the untriaged listing: `cargo verification evidence`) and `phase` (the `phase <name>: <seconds> s` timing lines) |

The producers are the Blobray scenario engine
([`harness/scenarios`](../harness/scenarios), `command = "vendor-scenario"`)
and the host stands ([`esp32s31/host/ieee802154`](../esp32s31/host/ieee802154/README.md),
`command = "vendor-host"`). Which of them reruns which shard is decided by
`run::regenerate`, which `cargo verification evidence` ([xtask](../../tools/xtask/README.md)) calls.

Without `producers` this library exposes comparison policy and the index
API. `producers` adds toolchain, chip profiles and the probe orchestration.
The gate invokes verification through its command line; qualification reads
only the shared format.

```console
cargo test --manifest-path verification/Cargo.toml -p oer-vendor-evidence --features producers
```
