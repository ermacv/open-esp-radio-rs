# ESP32-C5 radio register ownership

The editable hardware source is the schema-3 [`model/device.toml`](model/device.toml)
and its schema-2 peripheral fragments. It follows the layout of the
[ESP32-S31 register project](../esp32s31/README.md); the generator is
[`tools/registers/model`](../../tools/registers/model/README.md).

| Path | Owner and purpose |
| --- | --- |
| `model/device.toml`, `model/peripherals/` | Reviewed hardware register geometry and semantics |
| `model/memory.toml` | MMIO regions |
| `model/reviewed.toml` | Typed reviewed assertions with their applicability and evidence |
| `policy/api.toml` | Production PAC ownership partitions and typed transactions |
| `policy/ownership.toml` | Shared publication scope of named MMIO ranges |
| `policy/lints.toml` | Reviewed register-model lint policy |
| `evidence/` | Source identities of the reviewed facts |
| `published/radio.svd`, `published/radio.bindings.toml` | Generated SVD and binding index |
| `publication/registers.toml` | Source-only publication composition |

The IEEE 802.15.4 MAC aperture at `0x600A3000` applies to chip revision
v1.0. Its geometry comes from the ESP32-C5
`ieee802154_struct.h` and the common LL pinned in
[`verification/esp32c5/artifacts.toml`](../../verification/esp32c5/artifacts.toml),
not from the ESP32-S31 model: the register offsets agree, but the channel and
power codes, the event set, the PTI fields, the diagnostic counters and the
transmit-on delays differ.

The Wi-Fi MAC fragments (`model/peripherals/wifi-mac-*.toml`) keep the
peripheral, register and field names of the ESP32-S31 model. Their addresses
and reviews are derived from it: each `C5_*` source in
`evidence/vendor-wifi-libraries.toml` names the ESP32-C5 archive member of
every function the ESP32-S31 source describes and records that its full body
is identical to the ESP32-S31 one apart from modem addresses (and, where
stated, software structure offsets). Modem offsets are equal except where the
evidence lists a shift; the eight TX queue banks are `0x78` bytes apart
instead of `0x7C`. Such reviews use `provenance = "derived"` and
`completeness = "partial"`. Functions whose C5 bodies differ structurally are
never evidence for these facts, and ESP32-S31 HIL observations are not
carried over. The fragments publish register views only; no ESP32-C5 driver
owns them.

```console
cargo registers validate --manifest registers/esp32c5/publication/registers.toml
cargo registers generate --check --manifest registers/esp32c5/publication/registers.toml
```

The generated outputs are this directory's `published/` files,
[`pac/raw/src/lib.rs`](../../crates/hardware/esp32c5/pac/raw/src/lib.rs) and
[`pac/src/generated.rs`](../../crates/hardware/esp32c5/pac/src/generated.rs).
Do not edit them directly.
