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
board journal. For each of `--boots` rounds (ten by default) it takes these
steps:

1. Flash the [vendor calibration firmware](../../hil-vendor/README.md) into
   `ota_0`, reset the board and keep the boot's `phy_param` report. Then
   request every readable register of the published radio-PHY ownership
   partition (`RadioPhyPeripherals` of `registers/esp32s31/policy/api.toml`,
   registers from the published SVD). A read that resets the chip, such as
   a register whose clock domain the Wi-Fi calibration leaves off, is
   recorded as unreadable and the reads continue after the new boot.
2. Flash the production image class (`--production-image`, `correctness` by
   default) and prepare one reset with a fresh startup artifact path, so the
   boot calibrates fully and publishes its retained calibration.

Alternating the sides exposes both to the same board temperature drift.
Captures stay in the ignored output directory.

## Compare

`compare` does the following:

- Decodes each production artifact with the target's own codec.
- Rebuilds the retained PHY state and projects it onto the relation's output
  words.
- Reads the same fields from each vendor `phy_param`.

Cold calibrations vary from boot to boot, so the vendor's own spread is the
reference. A field's margin is the widest range its vendor boots span in any
one element. An element passes when every production value lies within its
vendor range widened by that margin.

Every field carries a review in [`tolerances.toml`](tolerances.toml) with
these parts:

- whether its elements are signed;
- which bits the relation models (`mask`);
- whether it describes the environment rather than calibration (`excluded`);
- the reason.

A field without a review leaves the verdict INCOMPLETE.

The summary goes to
[`evidence/hardware/calibration.json`](../../evidence/hardware/calibration.json).
It records:

- the capture date and verdict;
- both images;
- each field's margin and its vendor and production ranges;
- the excluded fields with their reasons;
- the vendor byte ranges no compared field covers;
- the vendor register state: registers read, unreadable, and varying
  between vendor boots. Production does not publish its register state
  yet, so registers are not compared.

## Limitations

The relation covers these parts of the committed calibration, in the order
the tracking roots commit them:

- references, channel and bandwidth;
- TX DC rows;
- RX DC banks;
- DCODE codes, status bytes, RX-gain table last indices and sensor index.

Committed state outside `phy_param`, and `phy_param` bytes outside the
relation, are reported as uncovered, not compared.
