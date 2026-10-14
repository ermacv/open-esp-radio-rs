# ESP32-S31 Wi-Fi station composition

The station's scan, join and attempt orchestration on ESP32-S31 was removed
with the old station path (tag `s31-station-old-path`); the station is the
portable `oer_ieee80211_sta::machine` on the lower-MAC port. The connected
modules below are the sources of their move into layer 2, each removed in the
commit of its move ([#563](https://github.com/ermacv/open-esp-radio-rs/issues/563)).

This crate specializes portable STA policy for ESP32-S31 while remaining
independent of Embassy, a network stack, board allocation and HIL protocols.
It owns chip transaction ordering and typed resource transitions, but not the
executor mechanisms used to wait for them.

Module map:

- `hardware/channel`: persistent PHY/channel owner retained across scan and reconnect;
- `hardware/control`: concrete receive and control hardware operations;
- `peer`: associated-peer WMM/HT/HE/rate-control programming;
- `peer_policy`: value-only Scan/Association/WMM/HE plan consumed by `peer`;
- `control_tx`, `single_mpdu_tx`: bounded management/data TX transactions
  and their entropy, power, time and completion ports;
- `tx_epoch`: unique pre-connected control-TX ownership state;
- `wpa2`: chip handshake and atomic hardware-key publication ports;
- `connected`, `connected_rx`, `connected_control`: association-scoped data,
  security, receive admission and control ownership;
- `profile`: local capability advertisement and channel lowering;
- `modem_sleep`, `connection_coex`: re-exports of the power manager of
  `oer-ieee80211-sta` and of the reconnect coexistence requests of
  `oer-espressif-ieee80211-policy`;
- `standalone_esp_now_rx`: ESP-NOW reception while no station is connected;
- `ftm`, `hardware/beacon_monitor`: bounded hardware admission frontiers whose
  physical activation limitations are stated in [FEATURES.md](../../../../hardware/esp32s31/driver/ieee80211/FEATURES.md).

`crates/protocols/ieee80211/sta` owns portable MLME, reconnect and association
preference policy. The chip profile determines eligible modes and lowers the
selected mode to S31 PHY parameters.
Connected control consumes a fresh ordered `PmActions` batch for each power
input. A network report containing both TX start and TX completion applies
the start's batch before producing and applying the completion's batch.
`ConnectedRxConfig` retains the validated `OwnAddress` and `Bssid` roles.
Unprotected disconnects use MAC's common receiver/BSS parser; negotiated
robust-management protection still dispatches through the separate PMF path.
`crates/runtime/esp32s31/ieee80211` supplies concrete timers, DMA/TX
owners, IRQ wakeups and task/network composition. HIL may observe those public
boundaries but must not implement a second driver path.
