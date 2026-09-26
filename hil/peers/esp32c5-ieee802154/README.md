# ESP32-C5 IEEE 802.15.4 reference peer

This ESP-IDF application turns an ESP32-C5 into the reference IEEE 802.15.4
peer of the HIL cell. The vendor driver (`esp_ieee802154_*`) owns the radio;
the application only drives it through a line protocol on the console, so
exchanges with the device under test run against the vendor implementation.

## Build and flash

The peer is built with a system-installed ESP-IDF release (currently v6.1);
the repository does not provision ESP-IDF. From this directory:

```console
. ~/.espressif/v6.1/esp-idf/export.sh
idf.py set-target esp32c5
idf.py build
idf.py -p /dev/ttyUSB0 flash
```

The console is the default UART. To use the USB Serial/JTAG port instead,
select it under *Component config → ESP System Settings → Channel for console
output*; the application follows that choice.

Name the peer's serial port in the `[ieee802154_peer]` table of
`hil/local.toml`.

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
| `PENDING <mode>` | Set the automatic frame-pending mode: `0` disabled, `1` enabled, `2` enhanced, `3` Zigbee. |
| `PENDING ADD <short>` | Add a short address (big-endian hex) to the pending table. |
| `PENDING CLEAR` | Clear the short-address pending table. |
| `ED <symbols>` | Run one energy detection of the given number of 16 µs symbols. |

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
