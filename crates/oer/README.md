# Open ESP Radio

`open-esp-radio` exposes the Rust library `oer`. It reexports existing types;
it owns no radio resources, packet queues or executor state.

```rust
use oer::wifi::WifiConfig;
use oer::ieee80211::{mac, softmac};
use oer::ieee80211::sta::association::{PhyMode, Preference};
```

The default `wifi` feature exposes portable Wi-Fi control and IEEE 802.11
protocols. `bluetooth` exposes `bluetooth::hci` and `bluetooth::le::ll`;
`ieee802154` exposes the IEEE 802.15.4 contracts. These features build without
a chip or executor. Memory and network value contracts are always available.

Chip selection and protocol backends are separate:

| Feature | Public API |
| --- | --- |
| `esp32s31` | `chips::esp32s31::hal`; no Wi-Fi STA/AP or Bluetooth driver selection |
| `esp32s31-wifi` | Wi-Fi protocols and `chips::esp32s31::driver::ieee80211::{mac,sta,ap}` |
| `esp32s31-bluetooth` | Bluetooth protocols and `chips::esp32s31::driver::bluetooth` |
| `upstream-xarxa`, `owned-xarxa` or `embassy-smoltcp` | Wi-Fi backend, `embassy::radio` service mailbox and `systems::esp32s31::embassy::wifi` with the selected network stack |
| `embassy-esp32s31-bluetooth` | Bluetooth backend and `systems::esp32s31::embassy::bluetooth` |

Each backend feature includes its chip and portable protocol feature. Selecting
`wifi,esp32s31` exposes portable Wi-Fi and HAL; select `esp32s31-wifi` to add the
Wi-Fi backend. Raw PAC authority requires an explicit restricted dependency.

Disable defaults when selecting a Bluetooth-only application:

```toml
# From an application under examples/; adjust the path for another checkout.
oer = { package = "open-esp-radio", path = "../../crates/oer", default-features = false, features = ["embassy-esp32s31-bluetooth"] }
```

That composition does not require Wi-Fi STA/AP, Embassy networking or Xarxa.
`systems::esp32s31::embassy::bluetooth` also reexports the cold-start inputs
that the driver path lacks: scheduler allocation and default TX power values,
`DtmRecheckPeriod`, the esp-hal radio platform, the deadline watchdog and SoC
`entropy::Entropy`. The [Bluetooth controller example](../../examples/esp32s31-bluetooth-controller/)
uses only the facade besides its board runtime and executor.
Cargo features are additive: another dependency enabling `wifi` on the same
facade also enables portable Wi-Fi in the final feature union.

For a ready ESP32-S31 Embassy Wi-Fi composition, select one of
`upstream-xarxa`, `owned-xarxa`, or `embassy-smoltcp`. The composition is exposed
at `systems::esp32s31::embassy::wifi`, together with every input needed to
construct it; the [station](../../examples/esp32s31-station/),
[access-point](../../examples/esp32s31-access-point/) and
[monitor](../../examples/esp32s31-monitor/) examples use only this path. Network profiles are alternatives and
are checked separately. Stack dependencies and patch ownership are described
in [network implementations](../../docs/network-implementations.md).

`composition/` owns the source-level binding of components; `systems::` exposes
those ready-to-construct owners in the public API. Stack and executor adapters
are separate integrations. Availability through the facade does not establish
hardware readiness or concurrent-radio operation. Bluetooth cold start, HCI
and RF support retain the limitations of the [Bluetooth capability map](../hardware/esp32s31/driver/bluetooth/FEATURES.md)
and [qualification contract](../../qualification/targets/esp32s31/bluetooth-le.toml).
The facade does not promise successful Bluetooth startup on hardware.

Architecture checks resolve isolated consumers to catch unwanted dependencies,
compile no-default/default and declared feature profiles, and test the public
type identities. Bluetooth and IEEE 802.15.4 are checked independently of Wi-Fi.

Internal libraries depend on their specific contracts and on `oer-radio` for
radio lifecycle, using the corresponding `oer-*` Cargo package names. They must
never depend on this facade. Association types are shared with the underlying
IEEE 802.11 encoder; the facade introduces no duplicate enums.
