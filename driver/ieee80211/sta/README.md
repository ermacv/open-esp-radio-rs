# Portable Wi-Fi station policy

This crate contains STA MLME and lifecycle policy that is independent of a
chip, executor and network stack. It consumes explicit port traits and returns
the exact caller-owned radio state at every success, retry, stop and failure
edge.

Module map:

- `join`: Open Authentication and Association retry/deadline transactions;
- `scan`: channel-plan progress and candidate-selection lifecycle;
- `station`: outer attempt, reconnect, backoff, disconnect and stop policy;
- `link_monitor`: beacon-loss decisions;
- `power_save`: STA power-state decisions and their safety preconditions;
- `ftm`, `twt`: bounded requester state and deadlines; their presence does not
  establish a chip timestamp or wake-schedule implementation;
- `request`: caller-visible station configuration and selection values.

This is not a generic 802.11 frame crate and it is not an ESP32 backend.
Frame parsing/building belongs in `driver/ieee80211/mac`; ESP32-S31 ordering
and hardware ownership belong in `driver/chips/esp32s31/ieee80211/sta`; clocks, tasks,
DMA wakeups and network leases belong to the radio runtime; final resource
claims and board composition belong to integration.

