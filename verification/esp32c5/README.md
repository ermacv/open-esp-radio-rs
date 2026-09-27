# ESP32-C5 vendor verification

[`artifacts.toml`](artifacts.toml) is the single pin of every ESP32-C5 vendor
archive and ROM ELF: upstream repository, revision, path and SHA-256 of each.
Fetch and verify them into `target/vendor/<source>/<revision>/` with

```console
cargo xtask vendor-fetch esp32c5
```

The PHY, Wi-Fi and coexistence revisions are the submodules of the ESP-IDF
revision pinned for the [ESP32-S31](../esp32s31/README.md), so both chips are
compared against the same vendor release. The Controller archive is the
`esp32c5-bt-lib` submodule of that ESP-IDF revision. The ROM ELFs of chip
revisions v0.x (`rom-rev0`) and v1.0 (`rom-rev100`) are pinned separately.
The ESP-IDF files are the IEEE 802.15.4 driver sources and the ESP32-C5
register, SoC and LL headers they include, the counterparts of the ESP32-S31
pin.

No scenario, probe or production crate uses these artifacts yet.
