# Portable Wi-Fi station policy

This crate contains the station, independent of a chip, executor and network
stack. It is sans-IO: `machine` is one state machine for the whole lifecycle
(scan, join, WPA2 four-way handshake, connected, background scan, group
rekey, leave) that takes the port's events, the channel arbiter's answers and
notices, commands and time as values and leaves the port's and the arbiter's
actions as values. A thin service loop over the lower-MAC port feeds it
([owner decision on #563](https://github.com/ermacv/open-esp-radio-rs/issues/563#issuecomment-6128926225)).

Module map:

- `machine`: the station, its tokens (`StaTokens`) and its host slice tests
  beside the real arbiter;
- `join`: Open System and SAE authentication and Association state machines
  and their retry policy;
- `association`: association capability selection from the scan record;
- `power_save`: what the access point knows of the station's power state and
  the TIM's decision;
- `station`: the application's vocabulary (lifecycle stages, fault reasons,
  reconnect policy) that `oer-radio` publishes;
- `modem_sleep`: the connected station's power manager, after the vendor's
  `pm.o` and `pm_coex.o`, with the vendor's timing constants;
- `sa_query`: the SA Query procedure of an association under management
  frame protection;
- `link_monitor`: beacon-loss decisions;
- `pmksa`: the SAE PMKSA cache a reconnect resumes, and its shared owner;
- `ftm`, `twt`: bounded requester state and deadlines; their presence does not
  establish a chip timestamp or wake-schedule implementation. The TWT wake
  plan is computed in `TsfInstant`s of one TSF generation the caller tracks;
  its intervals and guards are `TsfDuration`s. A guard before the epoch is
  `BeforeTsfEpoch`, and an endpoint beyond the epoch is `BeyondTsfRange`;
- `request`: caller-visible station configuration and selection values.

This is not a generic 802.11 frame crate and it is not an ESP32 backend.
Frame parsing/building belongs in `crates/protocols/ieee80211/mac`; ESP32-S31 ordering
and hardware ownership belong in `crates/roles/esp32s31/ieee80211/sta`; clocks, tasks,
DMA wakeups and network leases belong to the radio runtime; final resource
claims and board composition belong to integration.

