# verification/

The pins of the vendor code production follows and the reviewed facts read
from it: [README](README.md). No production behavior and no private vendor
artifacts live here.

| Path | Owns |
| --- | --- |
| `<chip>/artifacts.toml` | The single pin (repository, revision, SHA-256) of every vendor archive, ROM ELF, ESP-IDF file and vendor firmware output |
| `<chip>/facts/` | Reviewed fingerprints of cited vendor functions (`provenance.toml`) and recovered name maps (`names/`) |
| `<chip>/hil-vendor/` | ESP-IDF firmware that runs vendor code on the HIL stand |

## Rules

- `_oracles/` is private input: never commit it, vendor binaries or dumps.
- Hashes outside `artifacts.toml` record where a reviewed fact was observed;
  they are not pins.
- A recovered value carries its source: a `SOURCE:` block naming the pinned
  revision and symbol, or a register evidence record
  ([source policy](../docs/source-policy.md)).
- `facts/*.toml` are large and partly generated: grep them by explicit path.

## Commands

`<chip>` is `esp32s31` or `esp32c5`.

- `cargo xtask vendor fetch <chip> [--artifact ID]` downloads and verifies the pins into the vendor store behind `target/vendor/`.
- `cargo hil firmware build <image>` builds a `hil-vendor` catalog image against the pins.
- `oer-symbol-lineage` regenerates `esp32s31/facts/names` ([its README](../tools/symbol-lineage/README.md)).
