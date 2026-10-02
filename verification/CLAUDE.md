# verification/

Evidence that production drivers follow the pinned vendor code, by level
(L0 pins … L3 hardware): [README](README.md). No production behavior and no
private vendor artifacts live here.

| Path | Owns |
| --- | --- |
| `<chip>/artifacts.toml` | The single pin (repository, revision, SHA-256) of every vendor archive, ROM ELF and SDK firmware |
| `<chip>/facts/` | Cited-function fingerprints (`provenance.toml`) and recovered name maps |
| `<chip>/probes/`, `<chip>/scenarios/` | Compiled production entries and typed Blobray comparisons |
| `<chip>/evidence/` | Generated scenario shards and hardware cross-check summaries |
| `<chip>/hil-vendor/`, `<chip>/hardware/` | Vendor firmware and board cross-checks |
| `harness/`, `schema/` | Probe generation, the chip-neutral scenario engine, the shard schema |

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

- `cargo xtask vendor-fetch esp32s31` downloads and verifies the pins into `target/vendor/`.
- `cargo xtask check provenance --chip esp32s31` after touching a `SOURCE:` citation or a pin.
- `cargo xtask vendor-diff --chip esp32s31 --old A --new B` after a pin change.

Workflow: the `vendor-evidence` skill.
