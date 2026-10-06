# verification/

Evidence that production drivers follow the pinned vendor code, by level
(L0 pins … L3 hardware): [README](README.md). No production behavior and no
private vendor artifacts live here.

| Path | Owns |
| --- | --- |
| `<chip>/artifacts.toml` | The single pin (repository, revision, SHA-256) of every vendor archive, ROM ELF and SDK firmware |
| `<chip>/facts/` | Cited-function fingerprints (`provenance.toml`) and recovered name maps |
| `<chip>/probes/`, `<chip>/scenarios/` | Compiled production entries (built with the chip's image compiler, in the shared firmware cache) and typed Blobray comparisons (the verdict library; `scenarios/cli` its binary) |
| `<chip>/evidence/` | Generated scenario shards (`evidence/scenarios`, written only through `oer-vendor-evidence`); hardware cross-checks are HIL run bundles |
| `<chip>/hil-vendor/`, `<chip>/hardware/` | Vendor firmware and isolated firmware probes of board state |
| `harness/` | Probe generation and the chip-neutral scenario engine |
| `evidence/` | `oer-vendor-evidence`: evidence producers, orchestration, the verdict source policy and the `Producer` contract; re-exports the shared shard format |

The host CLI, harness, evidence producers and chip scenarios belong to this
workspace; firmware probes and host stands have their own workspaces below
the chip directories. Root-workspace shared formats live in
[`../hil/evidence-shard/`](../hil/evidence-shard/) and
[`../hil/phy-calibration-capture/`](../hil/phy-calibration-capture/README.md);
chip calibration relations and comparisons live in `../crates/hardware/<chip>/`.

## Rules

- `_oracles/` is private input: never commit it, vendor binaries or dumps.
- Hashes outside `artifacts.toml` record where a reviewed fact was observed;
  they are not pins.
- A comparison fails closed with `MATCH`, `DIFF` or `INCOMPLETE` and exercises
  compiled production code, never a shadow implementation.
- Only the check's owner regenerates `evidence/scenarios` shards, in commits
  of their own; never edit or merge them by hand.
- `facts/*.toml` are large and partly generated: grep them by explicit path.
- Production behavior never moves into a probe.

## Commands (in the background)

- `cargo verification fetch esp32s31` downloads and verifies the pins into `target/vendor/`.
- `cargo verification check provenance --chip esp32s31` after touching a `SOURCE:` citation or a pin.
- `cargo verification diff --chip esp32s31 --old A --new B` after a pin change.

Workflow: the `vendor-evidence` skill.
