# ESP32-S31 Wi-Fi station composition

This crate specializes portable STA policy for ESP32-S31 while remaining
independent of Embassy, a network stack, board allocation and HIL protocols.
It owns chip transaction ordering and typed resource transitions, but not the
executor mechanisms used to wait for them.

Module map:

- `cold_start`: finite power, PHY registration and initial MAC/RX startup;
- `channel`: persistent PHY/channel owner retained across scan and reconnect;
- `association`: PHY/rate/power plan derived from a selected candidate;
- `peer`: associated-peer WMM/HT/HE/rate-control programming;
- `tx`: entropy, calibrated-power and monotonic-time ports plus TX resources;
- `tx_epoch`: unique pre-connected control-TX ownership state;
- `scan`: cold/running scan ordering and mandatory RX cleanup contract;
- `join`: executor-independent RX, TX and observation contracts for the
  ESP32-S31 Authentication/Association adapter;
- `attempt`: complete selected-candidate to connected-entry transaction;
- `wpa2`: chip handshake and atomic hardware-key publication ports.

`driver/wifi/sta` remains the owner of portable MLME and reconnect policy.
`driver/adapters/embassy/esp32s31-wifi` supplies concrete timers, DMA/TX
owners, IRQ wakeups and task/network composition. HIL may observe those public
boundaries but must not implement a second driver path.
