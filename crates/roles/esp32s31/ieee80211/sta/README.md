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
The FTM frontier consumes the portable session into its transmission handle,
inspects that handle's immutable body, then consumes a hardware-admission
rejection and its terminal. It never accepts another requester's TX identity
as a completion and still performs no FTM publication or timestamp capture.
Connected control consumes a fresh ordered `PmActions` batch for each power
input. A network report containing both TX start and TX completion applies
the start's batch before producing and applying the completion's batch.
It validates relative deadlines and the 32-bit TBTT interval projection for
the complete batch before executing effects. `PowerSchedule` reports an
unrepresentable request; it never truncates an interval or silently disarms
an overflowing timer. `Due` becomes ready timer work and `ArmAt` retains the
deadline computed by the portable power manager.
`ConnectedRxConfig` retains the validated `OwnAddress` and nonzero `Bssid`
roles. Construct `ConnectedRxDispatcher` from that association identity;
an unconfigured arena holds no dispatcher and has no placeholder BSSID.
Unprotected disconnects use MAC's common receiver/BSS parser; negotiated
robust-management protection still dispatches through the separate PMF path.
`crates/runtime/esp32s31/ieee80211` supplies concrete timers, DMA/TX
owners, IRQ wakeups and task/network composition. HIL may observe those public
boundaries but must not implement a second driver path.

Connected security uses `RsnConnectedSupplicant::on_message3` for exact and
newer authenticated repeats. Its M4 keeps the incoming MAC protection;
`start_eapol` takes that protection explicitly, so an open M3 with installed
PTK gets an open response without consuming a CCMP PN. Protected group M1
still gets a protected group M2.
