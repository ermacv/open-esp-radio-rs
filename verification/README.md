# Vendor references

This tree pins the vendor code the production drivers follow and holds the
reviewed facts read from it. The [ESP32-S31](esp32s31/README.md) and the
[ESP32-C5](esp32c5/README.md) have the same layout. It contains no production
behavior and no private vendor artifacts.

```text
verification/
  <chip>/
    artifacts.toml   the pin of every vendor archive, ROM ELF, ESP-IDF file
                     and vendor firmware output
    facts/           reviewed fingerprints of cited vendor functions
                     (provenance.toml); recovered name maps (names/, ESP32-S31)
    hil-vendor/      ESP-IDF firmware that runs vendor code on the HIL stand
```

## Pins

`<chip>/artifacts.toml` is the only pin: each git source at a revision, each
release asset, and each artifact with its path and SHA-256. Firmware outputs
are pinned by the tracked catalog image whose recipe builds them, with no
digest. `cargo xtask vendor fetch <chip>` downloads and verifies every
fetchable artifact into the host-wide vendor store, linked from each
checkout's `target/vendor/<source>/<revision>/`; `--artifact ID` fetches one.
[`oer-vendor-pins`](../tools/vendor-pins) parses the manifest and
[`oer-vendor-artifacts`](../tools/vendor-artifacts/README.md) is its only
reader of fetched bytes. The pins are read by the image pipeline's interrupt
stack check (the pinned ROM ELF), by the register inventory, and by
`cargo hil firmware build`, which builds the `hil-vendor` catalog images
against the pinned ESP-IDF and archives.

Changing a pin means changing the manifest, then following the pinned
behavior in production. Hashes outside `artifacts.toml`, in reviewed register
evidence and reference notes, record the artifact a fact was observed in;
they are not pins.

## Facts

A recovered constant, table or register fact names the vendor function it
was read from: a `SOURCE:` block in production code or a
[register](../registers/README.md) evidence record, under the
[source policy](../docs/source-policy.md).
`<chip>/facts/provenance.toml` is reviewed data: the relocation-normalized
fingerprint of each vendor function those citations name, at the pinned
revision. The ESP32-S31 [name maps](esp32s31/facts/names/README.md) give
source names for generated function names of the pinned Controller archive;
[`oer-symbol-lineage`](../tools/symbol-lineage/README.md) regenerates them
after a new archive revision.

## Vendor firmware

`<chip>/hil-vendor/` holds ESP-IDF applications, each an entry of the
stand's firmware catalog, that run the pinned vendor code on a board so HIL
cross-checks compare vendor and production behavior on the same device.

Private inputs never enter the repository: `_oracles/` and fetched artifacts
stay ignored, and checked configuration never names machine-local paths.
