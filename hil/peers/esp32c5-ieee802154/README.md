# ESP32-C5 IEEE 802.15.4 reference peer

This ESP-IDF application turns an ESP32-C5 into the reference IEEE 802.15.4
peer of the HIL cell. The vendor driver (`esp_ieee802154_*`) owns the radio;
the application only drives it through a line protocol on the console, so
exchanges with the device under test run against the vendor implementation.

## Build and flash

The peer is a [firmware catalog](../../host/README.md#share-the-stand) entry
(`firmware.toml`). It builds against the one ESP-IDF revision pinned in
[`verification/esp32s31/artifacts.toml`](../../../verification/esp32s31/artifacts.toml),
installed below `target/` apart from any user installation:

```console
cargo hil --budget 5m firmware flash ieee802154-peer --board esp32c5
```

This builds the image, flashes it under a lease of that board only and records
it in the board journal as `ieee802154-peer`. The tracked
`sdkconfig.defaults` selects the USB Serial/JTAG console the stand uses and a
reproducible build.

Name the peer's board or serial port in the `[ieee802154_peer]` table of the
lab configuration. The board is shared with other consumers. Before an
IEEE 802.15.4 peer scenario `cargo hil run` reads the board journal; when
the board's newest recorded flash is another image, it flashes this image from
the catalog within the run's lease, which holds the peer board, and journals
it. The first such flash on a host builds the image against the shared ESP-IDF
cache inside the lease. `doctor` and `fixture check` do not flash: they report
the other image and who flashed it. A board without a recorded flash is
accepted; the peer's `@READY` handshake then decides.

## Line protocol

Lines end with `\n`. Every protocol line from the peer starts with `@`; the
host ignores any other output, such as boot messages. Hexadecimal bytes are
lowercase on output and accepted in either case on input.

At start the peer prints:

```text
@READY protocol=1 target=esp32c5
```

Each command is answered by `@OK <command>` or `@ERR <command> <reason>`.

| Command | Effect |
| --- | --- |
| `CFG <channel> <panid> <short> <ext> <promiscuous> <power>` | Channel 11–26; PAN ID and short address as four hex digits, big-endian; extended address as sixteen hex digits in over-the-air (little-endian) order; promiscuous `0`/`1`; transmit power in dBm. Receive-when-idle is enabled. |
| `RX` | Enter receive mode. |
| `SLEEP` | Leave receive mode. |
| `TX <cca> <mac>` | Transmit the MAC bytes (without FCS) given in hex, after a CCA when `cca` is `1`. The frame's own acknowledgement-request bit selects whether an ACK is awaited. |
| `TXAT <delay_us> <mac>` | Transmit without CCA `delay_us` (at most 1000000) microseconds from now through `esp_ieee802154_transmit_at` on the `esp_timer` clock. |
| `BURST <mac>` | Transmit the MAC bytes back to back without CCA until `BURST STOP`; the ends of these transmissions are counted, not reported. |
| `BURST STOP` | Stop the burst, wait for its last transmission and print `@BURST sent=<n> failed=<n>` before the reply. |
| `PENDING <mode>` | Set the automatic frame-pending mode: `0` disabled, `1` enabled, `2` enhanced, `3` Zigbee. |
| `PENDING ADD <short>` | Add a short address (big-endian hex) to the pending table. |
| `PENDING CLEAR` | Clear the short-address pending table. |
| `ED <symbols>` | Run one energy detection of the given number of 16 µs symbols. |
| `OFF` | Disable the driver (`esp_ieee802154_disable`): the MAC, BTBB and the PHY client are released, and RF closes after the last client. |
| `ON` | Enable the driver again (`esp_ieee802154_enable`); send `CFG` before the next operation. |

Events, printed when the driver reports them:

| Event | Meaning |
| --- | --- |
| `@RX <mac> rssi=<dBm> lqi=<n> pending=<0/1> ch=<n>` | A frame was received; `<mac>` excludes PHR and FCS; `pending` reports whether the automatic ACK carried frame pending. |
| `@TXDONE ack=-` | A frame without acknowledgement request was sent. |
| `@TXDONE ack=<mac> pending=<0/1> rssi=<dBm> lqi=<n>` | A frame was acknowledged; `<mac>` is the ACK without FCS. |
| `@TXFAIL <code>` | The driver reported `esp_ieee802154_tx_error_t` `<code>`. |
| `@ED <dBm>` | The energy detection result. |

Driver callbacks run in interrupt context; the application copies each
report into a bounded queue and prints it from a task.

## Enable bisection

The `ieee802154-peer-enable-bisect` catalog image
([project](../esp32c5-ieee802154-enable-bisect)) builds the same
application with the radio enable deferred: after the console is up it prints
`@BOOT enable-deferred` and `@READY` without calling `esp_ieee802154_enable`.
Three more commands then run the steps of that enable in its order, so a fault
can be placed at one step; `ON` runs the whole enable afterwards.

| Command | Step |
| --- | --- |
| `MCLK` | `modem_clock_module_enable(PERIPH_IEEE802154_MODULE)`, the MAC's modem clock. |
| `PHY` | `esp_phy_enable(PHY_MODEM_IEEE802154)`, the shared PHY and RF. |
| `BTBB` | `esp_btbb_enable()`, the baseband. |

Build or flash it like the peer:
`cargo hil firmware flash ieee802154-peer-enable-bisect --board esp32c5`.
