# Bluetooth GATT application

`oer-example-bluetooth-gatt` is the portable Trouble Host GATT
application of the [Bluetooth controller example](../esp32s31/bluetooth-controller/README.md).
The example binary and the Bluetooth HIL firmware both compose it; it is a
library so that neither consumer depends on the other.

- `gatt` serves the plaintext profile: one writable value, a fixed legacy
  advertising payload and value-only observations.
- `secure` (feature) adds LE Secure Connections Numeric Comparison, a bounded
  asynchronous bond store and encrypted attribute access.

The library owns no platform, executor, controller or console. Callers supply
the `trouble-host` stack, the HCI transport and, for `secure`, the Numeric
Comparison console. Host tests exercise the attribute tables, pairing policy
and bond store against the portable HCI controller model:

```console
cargo test -p oer-example-bluetooth-gatt --all-features
```

The security scope and its limits are described in the example README's
[authenticated GATT profile](../esp32s31/bluetooth-controller/README.md#authenticated-gatt-profile).
