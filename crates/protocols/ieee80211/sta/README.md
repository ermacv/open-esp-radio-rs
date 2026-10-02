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
- `scan`: channel-plan progress values, the candidate-scan port and the
  primitive `StaScanPort` of one channel visit;
- `attempt`: the phases of one attempt, their `StaAttemptPort`, and the
  personal credentials and security material an attempt carries;
- `station`: outer attempt, reconnect, backoff, disconnect and stop policy and
  the lifecycle port;
- `modem_sleep`: the connected station's power manager, after the vendor's
  `pm.o` and `pm_coex.o`, with the vendor's timing constants;
- `sa_query`: the SA Query procedure of an association under management
  frame protection;
- `link_monitor`: beacon-loss decisions;
- `pmksa`: the SAE PMKSA cache a reconnect resumes, and its shared owner;
- `ftm`, `twt`: bounded requester state and deadlines; their presence does not
  establish a chip timestamp or wake-schedule implementation;
- `request`: caller-visible station configuration and selection values.

This is not a generic 802.11 frame crate and it is not an ESP32 backend.
Frame parsing/building belongs in `crates/protocols/ieee80211/mac`; ESP32-S31 ordering
and hardware ownership belong in `crates/roles/esp32s31/ieee80211/sta`; clocks, tasks,
DMA wakeups and network leases belong to the radio runtime; final resource
claims and board composition belong to integration.

