# Hardware calibration cross-check

Compares the cold PHY calibration of the pinned vendor firmware with the
production calibration on the same ESP32-S31 board. The comparison uses the
reviewed `phy_param` relation that the Blobray tracking scenario also uses
([`committed.rs`](../../scenarios/src/phy/committed.rs)). The production
side goes through the same projection functions as the comparison probes
([`calibration_projection.rs`](../../probes/radio/library/src/calibration_projection.rs)).
It does not depend on Blobray peripheral models.

```console
cargo xtask vendor-firmware --chip esp32s31 calibration
cargo build -p oer-esp32s31-phy-vendor-calibration
cargo hil --owner <name> lease --board esp32s31 --air exclusive --budget 30m -- \
    target/debug/oer-esp32s31-phy-vendor-calibration capture --output <new directory>
cargo run -p oer-esp32s31-phy-vendor-calibration -- compare --captures <directory>
```

## Capture

`capture` runs under a lease of the esp32s31 board with exclusive air,
since cold calibration measures TX and RX DC and IQ that another
transmission would bias. It holds the board's device lease and records
every flash in the
board journal. For each of `--boots` rounds (ten by default) it takes these
steps:

1. Flash the [vendor calibration firmware](../../hil-vendor/README.md) into
   `ota_0`, reset the board and keep the boot's `phy_param` report. Then
   request every readable register of the published radio-PHY ownership
   partition (`RadioPhyPeripherals` of `registers/esp32s31/policy/api.toml`,
   registers from the published SVD). A read that resets the chip, such as
   a register whose clock domain the Wi-Fi calibration leaves off, is
   recorded as unreadable and the reads continue after the new boot. Then
   request every register of the analog image: each analog-I2C register a
   reviewed `PhyI2cField` of the PAC API policy occupies (its
   `register-image`), read through the ESP-IDF analog-I2C driver.
2. Flash the production image class (`--production-image`, `correctness` by
   default) and prepare one reset with a fresh startup artifact path, so the
   boot calibrates fully and publishes its retained calibration. Then read
   the radio-PHY register image (HIL `PhyRegisterImage`) at the indices the
   vendor boot of the round read, and the whole analog image (HIL
   `PhyAnalogImage`, one production analog-I2C read per register).

`--lifecycle` selects when both sides report their registers. `cold`, the
default, reads them after the cold calibration and the Wi-Fi bring-up.
`restart` first restarts the Wi-Fi radio once: the vendor firmware stops
and starts its Wi-Fi client, and requires that stopping it released every
PHY modem, which closes RF; production runs its idle radio restart
(`RestartRadio`) and requires that RF was closed and woken. A vendor read
that resets the chip is followed by another restart before the reads
continue.

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
- the one lifecycle point it applies to (`lifecycle`), when the difference
  comes from the point rather than the state;
- the reason.

A field without a review leaves the verdict INCOMPLETE.

The summary of a `cold` capture goes to
[`evidence/hardware/calibration.json`](../../evidence/hardware/calibration.json),
that of a `restart` capture to
[`evidence/hardware/calibration-restart.json`](../../evidence/hardware/calibration-restart.json),
unless `--output` names a path. It records:

- the capture date, lifecycle point and verdict;
- both images;
- each field's margin and its vendor and production ranges;
- the excluded fields with their reasons;
- the vendor byte ranges no compared field covers;
- the register image: registers compared and matched, the registers
  outside their vendor range widened by their own vendor spread with both
  ranges, reviewed exclusions, registers whose read reset the chip, and
  vendor-readable registers production did not report;
- the analog image, under the same rule and reviews.

## Limitations

Both sides are read at the same lifecycle point: the vendor firmware starts
its Wi-Fi client without a connection, with power save off and promiscuous
RX on its home channel, since production's role-neutral Wi-Fi bring-up
enables RX on its initial channel and has no modem sleep. Production's missing modem sleep is a behavior difference
outside this calibration comparison.


The relation covers these parts of the committed calibration, in the order
the tracking roots commit them:

- references, channel and bandwidth;
- TX DC rows;
- RX DC banks;
- DCODE codes, status bytes, RX-gain table last indices and sensor index.

Committed state outside `phy_param`, and `phy_param` bytes outside the
relation, are reported as uncovered, not compared.
