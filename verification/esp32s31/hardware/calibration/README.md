# Hardware calibration cross-check

Compares the cold PHY calibration of the pinned vendor firmware with the
production calibration on the same ESP32-S31 board. The comparison uses the
reviewed `phy_param` relation that the Blobray tracking scenario also uses
([`committed.rs`](../../scenarios/src/phy/committed.rs)). The production
side goes through the same projection functions as the comparison probes
([`calibration_projection.rs`](../../probes/radio/library/src/calibration_projection.rs)).
It does not depend on Blobray peripheral models.

```console
cargo xtask vendor-firmware esp32s31 calibration
cargo build -p oer-esp32s31-phy-vendor-calibration
cargo hil --owner <name> lease --budget 15m -- \
    target/debug/oer-esp32s31-phy-vendor-calibration capture --output <new directory>
cargo run -p oer-esp32s31-phy-vendor-calibration -- compare --captures <directory>
```

## Capture

`capture` holds the board's device lease and records every flash in the
board journal. It takes these steps:

1. Flash the [vendor calibration firmware](../../hil-vendor/README.md) into
   `ota_0`.
2. Reset the board `--boots` times and keep each boot's `phy_param` report.
3. Flash the production image class (`--production-image`, `correctness` by
   default) and prepare each of `--boots` resets with a fresh startup
   artifact path. Every production boot therefore calibrates fully and
   publishes its retained calibration.

Captures stay in the ignored output directory.

## Compare

`compare` does the following:

- Decodes each production artifact with the target's own codec.
- Rebuilds the retained PHY state and projects it onto the relation's output
  words.
- Reads the same fields from each vendor `phy_param`.

A field element passes when every production value lies within the range the
vendor boots span, widened by the field's reviewed margin in
[`tolerances.toml`](tolerances.toml). If a compared field has no reviewed
tolerance, the verdict is INCOMPLETE.

The summary goes to
[`evidence/hardware/calibration.json`](../../evidence/hardware/calibration.json).
It records:

- the capture date and verdict;
- both images;
- each field's vendor and production ranges;
- the relation field excluded as tracking progress;
- the vendor byte ranges no compared field covers.

## Limitations

The relation covers these parts of the committed calibration, in the order
the tracking roots commit them:

- references, channel and bandwidth;
- TX DC rows;
- RX DC banks;
- DCODE codes, status bytes, RX-gain table last indices and sensor index.

Committed state outside `phy_param`, and `phy_param` bytes outside the
relation, are reported as uncovered, not compared.
