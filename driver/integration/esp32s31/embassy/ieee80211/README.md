# ESP32-S31 Wi-Fi product integration

`open-esp-radio-esp32s31-embassy-wifi` composes the public radio lifecycle,
static resources, IRQ bindings and selected network adapter. Applications own
board identity, credentials, IP configuration and sockets. The
[network implementation guide](../../../../../docs/network-implementations.md)
explains the external crates, reasons for patches and complete build commands.

Select exactly one network feature with `default-features = false` when
replacing the default:

| Feature | Product contract |
| --- | --- |
| `owned-network` (default) | Maintained Embassy/Xarxa contract with explicit packet pools |
| `upstream-network` | Original Xarxa driver contract; the application supplies its stack |
| `compat-network` | Released Embassy/smoltcp token contract |

Both `--network upstream-xarxa` and `--network patched-xarxa` in repository
builders select `upstream-network`. Their difference is the application graph's
Xarxa source, not a second product feature. Source overrides belong to the
consumer workspace; selecting this library feature alone does not apply a patch.

The [packet ownership contract](../../../../../docs/wifi-egress.md) defines
adapter and physical scheduler boundaries. Static dimensions and the one-time
resource claim belong to this crate; reusable adapters supply storage types.
All network selections share the ESP32-S31 hardware dependencies. Example and
HIL support is listed separately in the implementation guide: a library feature
does not establish that every application role has been qualified.

Applications that construct their own stack can consume `Esp32s31WifiDevice`
through `into_upstream()`, `into_compat()` or `into_owned()`, according to the
selected contract. The owned transfer returns its matching packet allocator
alongside the unique device. This permits application-owned stack composition
and observation without exposing hardware authority or cloning an endpoint.
