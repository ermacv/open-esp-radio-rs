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
| `<chip>/hil-vendor/`, `<chip>/hardware/` | Vendor firmware and board cross-checks |
| `harness/` | Probe generation and the chip-neutral scenario engine |
| `phy-calibration-capture/` | The chip-neutral capture of the PHY calibration cross-check the HIL `phy` family records |
| `evidence/` | `oer-vendor-evidence`: the shard format, the one shard reader and writer, currency, the verdict source policy and the `Producer` contract (Blobray engine, host stands) |

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
