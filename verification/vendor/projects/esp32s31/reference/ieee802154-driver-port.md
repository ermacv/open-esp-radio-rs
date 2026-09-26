# ESP32-S31 IEEE 802.15.4 driver port map

This reference identifies the public ESP-IDF IEEE 802.15.4 driver that the
production crates port, pinned at commit
`7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe`. Unlike the Wi-Fi and Bluetooth
controllers, the complete MAC driver and its register layer are public source;
only the PHY, BTBB and coexistence libraries it calls are closed. The map
assigns every source unit to its owning production layer and states the
current coverage. Coverage here is source inventory; the
[IEEE 802.15.4 catalog](../../../../../qualification/catalog/esp32s31/ieee802154.toml)
and qualification remain the readiness authority.

The [lifecycle](ieee802154-lifecycle.md), [dataplane](ieee802154-dataplane.md)
and [control](ieee802154-control.md) references record the reviewed contracts
of individual paths in detail.

## Source ledger

The files were fetched from the pinned commit and hashed locally. Together they
are the complete open driver and register layer for this chip.

| Public ESP-IDF source | Lines | SHA-256 |
| --- | ---: | --- |
| [`components/esp_hal_ieee802154/include/hal/ieee802154_common_ll.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_hal_ieee802154/include/hal/ieee802154_common_ll.h) | 565 | `ba4ce294b402df311f25c4d0ce9cb33449e3eb41993aff94a25df5a66142d471` |
| [`components/esp_hal_ieee802154/esp32s31/include/hal/ieee802154_ll.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_hal_ieee802154/esp32s31/include/hal/ieee802154_ll.h) | 30 | `a66a3562ff6ef62ffa6dda90bc08e97f0d4509bcf3817cdc57795fd97daeaa30` |
| [`components/esp_hal_ieee802154/esp32s31/ieee802154_periph.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_hal_ieee802154/esp32s31/ieee802154_periph.c) | 12 | `56246b6b482752e0d217e2391acb4869ae02cd18068c5ee3b361a4b7ae110995` |
| [`components/ieee802154/driver/esp_ieee802154_dev.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/driver/esp_ieee802154_dev.c) | 1258 | `9aaccfa2832cb89bfdfd98086a984269e621400a272b02926c4e088d16222830` |
| [`components/ieee802154/driver/esp_ieee802154_pib.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/driver/esp_ieee802154_pib.c) | 302 | `4bc94779b0c29fdfc77dcdf0c6d3d66fad5d02324aa951d9f19877bc62532cf4` |
| [`components/ieee802154/driver/esp_ieee802154_ack.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/driver/esp_ieee802154_ack.c) | 178 | `525e1ac8b3e5bad9bfb74b7e786d72e23fd83405e3c09fbc85b8c2cb387b8a1c` |
| [`components/ieee802154/driver/esp_ieee802154_frame.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/driver/esp_ieee802154_frame.c) | 468 | `af84dc5dcc04d1ee20fd2f686ebc7220793791c7eb50bfd41107727a58d2df3d` |
| [`components/ieee802154/driver/esp_ieee802154_timer.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/driver/esp_ieee802154_timer.c) | 126 | `66a2b68bb718d8fa878cbf28488af810ecb902288fde26dc7a0e5b32dcbd1326` |
| [`components/ieee802154/driver/esp_ieee802154_event.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/driver/esp_ieee802154_event.c) | 94 | `c62a7bc108dce00e4b907c9521ad4de0c49080d6f6f5b091481f5c2f4af94063` |
| [`components/ieee802154/driver/esp_ieee802154_sec.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/driver/esp_ieee802154_sec.c) | 30 | `7f5cae45d08ec496b78fd18cfa788be8557ba91d40eb6fe77b42967961bbdc19` |
| [`components/ieee802154/driver/esp_ieee802154_util.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/driver/esp_ieee802154_util.c) | 92 | `e1a012d5f359e2445128977e82a304ba94c100c2994729e062e26586596df38a` |
| [`components/ieee802154/driver/esp_ieee802154_debug.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/driver/esp_ieee802154_debug.c) | 507 | `e4dd37b1ffc462c78a12cca7d57e4e8bd4e0e8984d542012b12ce964ee9a1812` |
| [`components/ieee802154/esp_ieee802154.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/esp_ieee802154.c) | 457 | `a83716d9944d4ffba1998cc64ebb635a605b60fc77c74ae6070e83a1c617f1bc` |
| [`components/ieee802154/esp_ieee802154_multipan.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/esp_ieee802154_multipan.c) | 141 | `4a82ff4a9b1eac7158ecc3c06e800ea1a844c6b2f87ff773fcd55c3cf9552aa3` |
| [`components/ieee802154/include/esp_ieee802154.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/include/esp_ieee802154.h) | 692 | `244f330affef9e4d4383275c678bfb2a0a5027725ffa8ebf1266897db2ca1e59` |
| [`components/ieee802154/include/esp_ieee802154_types.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/include/esp_ieee802154_types.h) | 157 | `5206f935bfaee354562e7ab87d499a196db251aa52f1d1bd9051bb32b4957424` |
| [`components/ieee802154/include/esp_ieee802154_multipan.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/include/esp_ieee802154_multipan.h) | 224 | `4c040a3b38769a2e697f65f21b04ea84d93f3de9048be27977b8e61b55777146` |
| [`components/ieee802154/private_include/esp_ieee802154_dev.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/private_include/esp_ieee802154_dev.h) | 275 | `afdb884e6dc78f19c9d72adedfc1ea7fdbbdebb86d1e7aaa80bb89e62a74bfd3` |
| [`components/ieee802154/private_include/esp_ieee802154_pib.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/private_include/esp_ieee802154_pib.h) | 281 | `cdad2d0fa07babe25f0174d69d551ca7acb6bd250ff9614961d0bff55638d835` |
| [`components/ieee802154/private_include/esp_ieee802154_frame.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/private_include/esp_ieee802154_frame.h) | 228 | `c4d326c59bd71a43db2de265ab3886064faf95ee018a541fc2deb0bc5609d1e1` |
| [`components/ieee802154/private_include/esp_ieee802154_ack.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/private_include/esp_ieee802154_ack.h) | 82 | `977df90ca0c0db4f2148c66d8956c49bdfd7418e7a33925b6495144be6db197d` |
| [`components/ieee802154/private_include/esp_ieee802154_timer.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/private_include/esp_ieee802154_timer.h) | 156 | `e5ab8062c0c4b9195bd597a79a6aabb692fdb686a9599a034db36a46d8f89bd6` |
| [`components/ieee802154/private_include/esp_ieee802154_util.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/private_include/esp_ieee802154_util.h) | 467 | `4ca86544b16248e1d66b85cc84df8ac37bde50727189b3b242513aab22e19017` |

## Interrupt-context decisions

The driver takes every time-critical decision inside the interrupt handler
([`ieee802154_isr`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/driver/esp_ieee802154_dev.c#L782-L895)),
not in a task:

- `RX_DONE` selects the ACK path before the acknowledgement leaves: it
  computes the frame-pending bit from the pending-address table
  (`ieee802154_ack_config_pending_bit`), and for a 2015 frame generates the
  enhanced ACK, publishes its address and notifies the hardware
  ([L538-L585](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/driver/esp_ieee802154_dev.c#L538-L585));
- `TX_DONE` of an ACK-requesting frame enters ACK reception and starts the
  timer-zero watchdog
  ([L507-L536](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/driver/esp_ieee802154_dev.c#L507-L536));
- a receive abort (CRC, filter, SFD timeout, ...) is not reported upward; the
  deferred `next_operation` re-enables receive when `rx_when_idle` is set, or
  enters idle/sleep
  ([L488-L505](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/driver/esp_ieee802154_dev.c#L488-L505),
  [L604-L640](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/driver/esp_ieee802154_dev.c#L604-L640)).

Task-side entry points (`ieee802154_transmit`, `receive`, `energy_detect`,
`cca`, `sleep`) run under the same critical section and always pass through
`stop_current_operation`, which applies the stop path of the current private
state
([L412-L461](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/driver/esp_ieee802154_dev.c#L412-L461)).
Consequently the owner of the MAC state machine must be reachable from both
interrupt and task context; an interrupt that only acknowledges events and
defers every decision to a task cannot meet these deadlines.

## Closed dependencies

| ESP-IDF call | Purpose in the driver | Open counterpart in this repository |
| --- | --- | --- |
| `esp_phy_enable` / `esp_phy_disable(PHY_MODEM_IEEE802154)` | RF client acquire and release around operations | PHY registration and RF close on `route::Ieee802154` |
| `esp_btbb_enable` / `esp_btbb_disable` | common BTBB initialization | PHY BTBB and IEEE 802.15.4 timing transition |
| `bt_bb_get_tx_pwr_table` | dBm-to-power-index table | HAL `tx_power` over an external level provider |
| `esp_coex_ieee802154_*` | PTI scenes, external-coexistence stages, coexistence break notice | coexistence driver; the route uses the disabled PTI baseline |
| `ieee802154_txon_delay_set` (called by `ieee802154_mac_init`) | TX-on delay | none |
| `bt_bb_get_cur_rx_info` | receive diagnostic | none |
| `modem_clock_module_*` | module clock, reset and MAC reset | HAL IEEE 802.15.4 lifecycle |
| `sleep_retention_*`, `esp_phy_modem_init` | register retention across light sleep | none |

## Ownership map

`Implemented` means production source owns the complete unit, `partial` that a
subset exists, and `absent` that no production owner exists. The owner column
is the layer that holds the unit under the PAC/HAL rule: the PAC performs
single register transactions, the HAL owns multi-step hardware sequences and
route state, the driver owns the IEEE 802.15.4 MAC state machine, and portable
frame semantics belong to the protocol crate.

| ESP-IDF unit | Owner | Coverage |
| --- | --- | --- |
| `ieee802154_common_ll.h`: command, event, abort, address, policy, ED, timer, security, pending-bit and enhanced-ACK accessors | PAC `ieee802154` | implemented, except pending mode, `is_current_rx_frame`, security-offset readback and the debug counters |
| `ieee802154_ll.h` (S31): `IEEE802154_RSSI_COMPENSATION_VALUE` 0, receive sensitivity -104 | driver | absent |
| `ieee802154_mac_init` / `deinit`, `ieee802154_enable` / `disable` | HAL IEEE 802.15.4 lifecycle | partial: clocks, reset and masked foundation; the driver instead enables its event set and interrupt at init |
| `ieee802154_pib_*`, `ieee802154_pib_update` | HAL policy | partial: fixed channel, CCA, ACK timeout, control flags and primary PAN identity; no mutable PIB, TX power, pending mode or `rx_when_idle` |
| `start_ed`, `tx_init`, `rx_init`, `ieee802154_transmit`, `receive`, `energy_detect`, `cca` | HAL operation starts, driver entry points | partial: single operations without `stop_current_operation` |
| `stop_rx` ... `stop_ed`, `stop_current_operation` | HAL | absent |
| `ieee802154_isr`, `isr_handle_*`, `next_operation`, private 12-state machine | driver MAC engine | partial: a per-operation actor handles RX without auto-ACK, TX with and without ACK, ED and CCA; no auto-ACK, enhanced ACK, `rx_when_idle` or stop |
| RX buffer ring, `set_next_rx_buffer`, frame info | driver DMA ownership | partial: pinned RX pool and TX buffer |
| `esp_ieee802154_timer.c` | HAL timers, driver callbacks | partial: ACK watchdog on timer zero |
| `ieee802154_transmit_at`, `receive_at` (timer and ETM) | driver | absent |
| `esp_ieee802154_ack.c` pending table and `ack_config_pending_bit` | protocol pending table, driver selection | absent |
| `esp_ieee802154_frame.c` | protocol frame parsing | partial: frame control and ACK request only |
| `esp_ieee802154_sec.c` | driver over PAC security | partial: PAC transaction only |
| `esp_ieee802154_multipan.c` | HAL policy, driver | partial: PAC multi-PAN fields |
| `esp_ieee802154_event.c` callbacks | runtime event handoff | partial: acknowledged-interrupt queue |
| `esp_ieee802154_util.c` coexistence scenes, channel conversion | coexistence driver, HAL | partial: channel conversion only |
| `ieee802154_sleep`, `rf_enable` / `rf_disable`, sleep retention | HAL and PHY | absent |
| `esp_ieee802154_debug.c` | not ported: optional statistics | absent |
| `esp_ieee802154.c` public API | role over the portable radio contract | absent |

The masked foundation with serialized polled ED and CCA has no ESP-IDF
counterpart. It is retained as a HIL diagnostic path only.
