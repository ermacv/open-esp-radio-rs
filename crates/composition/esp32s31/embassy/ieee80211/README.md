# ESP32-S31 Wi-Fi product integration

`oer-esp32s31-ieee80211-system` is the ESP32-S31 Wi-Fi composition on the
shared radio. Applications own board identity, credentials, IP configuration
and sockets. The
[network implementation guide](../../../../../docs/network-implementations.md)
explains the external crates, reasons for patches and complete build commands.

## Current state

The roles' orchestration (the station, the access point, the monitor, ESP-NOW
and STA+AP, with the application facade `new`, `WifiSystem` and the radio
supervisor's role dispatch over them) was removed with the old station path
(tag `s31-station-old-path`). It returns in
[#563](https://github.com/ermacv/open-esp-radio-rs/issues/563)'s step 2 as the
roles' bodies on the lower-MAC port (`run_port`), the station being the
portable `oer_ieee80211_sta::machine` beside the channel arbiter
(`oer-ieee80211-arbiter`). The ESP32-S31 HIL agent's Wi-Fi workloads and every
Wi-Fi HIL scenario wait for it; the scenarios are marked `unsupported`.

`cargo xtask check network` audits this crate as `owned-composition`: its
`owned-network` feature reaches the owned adapter and the pinned Xarxa driver
contract. Every reachable network API must retain its full revision pin and
agree with the other packages from that source. Applications and HIL keep the
`owned-product` check, which also requires the Xarxa and Embassy stacks. This
crate returns to `owned-product` when its product runner is restored.

What stays here:

- the interrupt bindings (`mac_interrupt`, `power_interrupt`);
- the deadline watchdog binding (`WatchdogConfig`);
- the interface addresses the chip was programmed with (`efuse`);
- value-only diagnostic observations;
- the [`resources`](src/resources.rs) profile of named static memory, also
  available on the host for product resource and ownership validation.
