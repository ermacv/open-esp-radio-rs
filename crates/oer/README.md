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

`esp32s31` exposes `chips::esp32s31::{hal, driver}`. Raw PAC authority requires
an explicit dependency on the restricted hardware crate.

For a ready ESP32-S31 Embassy Wi-Fi composition, select one of
`upstream-xarxa`, `owned-xarxa`, or `embassy-smoltcp`. The composition is exposed
at `systems::esp32s31::embassy::wifi`. Network profiles are alternatives and
are checked separately. Stack dependencies and patch ownership are described
in [network implementations](../../docs/network-implementations.md).

Internal libraries depend on their specific contracts and on `oer-radio` for
radio lifecycle, using the corresponding `oer-*` Cargo package names. They must
never depend on this facade. Association types are shared with the underlying
IEEE 802.11 encoder; the facade introduces no duplicate enums.
