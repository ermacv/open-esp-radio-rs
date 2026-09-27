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

The model currently publishes the IEEE 802.15.4 MAC aperture at `0x600A3000`
for chip revision v1.0. Its geometry comes from the ESP32-C5
`ieee802154_struct.h` and the common LL pinned in
[`verification/esp32c5/artifacts.toml`](../../verification/esp32c5/artifacts.toml),
not from the ESP32-S31 model: the register offsets agree, but the channel and
power codes, the event set, the PTI fields, the diagnostic counters and the
transmit-on delays differ.

```console
cargo registers validate --manifest registers/esp32c5/publication/registers.toml
cargo registers generate --check --manifest registers/esp32c5/publication/registers.toml
```

The generated outputs are this directory's `published/` files,
[`pac/raw/src/lib.rs`](../../crates/hardware/esp32c5/pac/raw/src/lib.rs) and
[`pac/src/generated.rs`](../../crates/hardware/esp32c5/pac/src/generated.rs).
Do not edit them directly.
