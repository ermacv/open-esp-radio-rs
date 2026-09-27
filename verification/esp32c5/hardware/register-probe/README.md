# ESP32-C5 register probe

A standalone image that checks the published
[ESP32-C5 register model](../../../../registers/esp32c5/README.md) on the
board. It opens the IEEE 802.15.4 MAC clocks as esp-radio does, writes all
ones to selected read-write MAC registers through the generated raw PAC and
reads each word back. The bits that stay set are the implemented ones, so
every `PROBE` line is `MATCH` when the silicon has the field widths the model
declares and `DIFF` otherwise. The MAC stays idle; no command is issued.

| Register | Expected implemented bits |
| --- | --- |
| `CHANNEL` | frequency code 6:0 and the unclassified bit 7 |
| `TX_POWER` | power code 4:0 |
| `EVENT_ENABLE` | thirteen events 12:0 |
| `COEX_PTI` | TX/RX PTI 3:0, ACK PTI 7:4, bit 8 |

Build it here and flash it through the stand:

```console
cargo build --release
cargo hil flash --board esp32c5 --image esp32c5-register-probe \
  --monitor 30s --until PROBE-DONE \
  target/riscv32imac-unknown-none-elf/release/oer-esp32c5-register-probe
```

The image boots through the ESP-IDF second-stage bootloader that `espflash`
writes and uses esp-hal's standard linker script.

The first run on the stand's ESP32-C5 v1.0 (2026-09-27) matched every width
the struct declares except `CHANNEL`, whose bit 7 is also implemented; the
register model records that bit as unclassified with this observation as its
evidence.
