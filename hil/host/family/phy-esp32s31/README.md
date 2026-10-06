# oer-hil-family-phy-esp32s31

The ESP32-S31 composition of the [PHY HIL family](../phy/README.md): the
family's comparison port implemented with the chip's vendor calibration
comparison,
[`oer-esp32s31-phy-vendor-calibration`](../../../phy/esp32s31/calibration/README.md),
over the published radio-PHY partition and analog-I2C register images. The
runner's family registry (`hil/host/runner/src/scenario.rs`) registers its
`FAMILY` as the `[phy]` family; the family crate itself names no chip, and
its captures are the chip-neutral
[`oer-phy-calibration-capture`](../../../phy/capture).

```console
cargo test -p oer-hil-family-phy-esp32s31
```
