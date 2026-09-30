# Portable Wi-Fi station policy

This crate contains STA MLME and lifecycle policy that is independent of a
chip, executor and network stack. It is sans-IO: its state machines take
received frames and elapsed milliseconds as values, and it declares the port
traits whose drivers, in
[`oer-ieee80211-sta-service`](../../../services/ieee80211/sta/README.md), wait
on hardware and time and return the exact caller-owned radio state at every
success, retry, stop and failure edge.

Module map:

- `join`: Open System and SAE authentication and Association state machines,
  retry policy and the `StaJoinBackend` port;
- `association`: association capability selection from the scan record;
- `scan`: channel-plan progress values and the candidate-scan port;
- `station`: outer attempt, reconnect, backoff, disconnect and stop policy and
  the lifecycle port;
- `link_monitor`: beacon-loss decisions;
- `pmksa`: the SAE PMKSA cache a reconnect resumes;
- `ftm`, `twt`: bounded requester state and deadlines; their presence does not
  establish a chip timestamp or wake-schedule implementation;
- `request`: caller-visible station configuration and selection values.

This is not a generic 802.11 frame crate and it is not an ESP32 backend.
Frame parsing/building belongs in `crates/protocols/ieee80211/mac`; ESP32-S31 ordering
and hardware ownership belong in `crates/roles/esp32s31/ieee80211/sta`; clocks, tasks,
DMA wakeups and network leases belong to the radio runtime; final resource
claims and board composition belong to integration.

