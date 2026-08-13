# ESP32-S31 Wi-Fi status

The [qualification ledger](../qualification/targets/esp32s31/wifi-sta.ledger)
is authoritative for measured behavior and dated evidence.

Implemented:

- PAC-backed RF/PHY start, full calibration and channel selection;
- WPA2 STA scan/join/reconnect and an application-owned `embassy-net` stack;
- finite application scan with a bounded report and `Idle -> Scan -> Idle`
  ownership round trip;
- QoS, BlockAck, bounded TX A-MPDU and RX reorder/handoff;
- exclusive normalized monitor capture with bounded, non-blocking overflow;
- one single-client WPA2-Personal ERP AP with pairwise unicast data;
- one public `Radio -> Wifi -> STA|AP|Monitor` lifecycle and one runner owning
  PAC, DMA and ISR until cooperative quiescence;
- an always-awake station profile; power-save policy is not exposed by the
  production API;
- feature-gated qualification events; production builds contain no logging.

The internal PAC/HAL/LMAC register layer now contains exact, typed Wi-Fi MAC
COEX leaves for receive-beacon PTI, individual-TWT PTI/clear and all four TX
queue PTI vectors. `wifi-mac-coex-register-programming` closes all five
vendor-to-Rust transactions. This does not make coexistence a public runtime
feature: scheduler integration, lifecycle ownership and a joint Wi-Fi/BLE
hardware run are still absent.

The AP profile is intentionally limited to one client, 20 MHz, no HT/A-MPDU,
no AP+STA, no group-data TX and no power save.

At the register boundary, STA interface zero and SoftAP interface one can now
be published together through one closed `MacStaApReceivePlan`; the plan also
requires an explicit closed `MacStaPolicyMode` rather than guessing from AP
state. The reviewed
CCMP connection-context proof also distinguishes STA=0 from AP=1. The focused
`wifi-interface-context` suite independently executes the complete
`hal_mac_set_addr` and `hal_mac_set_bssid` leaves for both selectors: 2/2
whole-function matches, no mismatch and no incomplete case. The
`wifi-ap-sta-interface-identity` feature therefore qualifies the disjoint
identity banks. A separate cross-archive oracle executes the sparse
`wifi_set_rx_policy` cases 6 and 8 and matches both case-six register submodes
and the case-eight AP transaction against the production PAC/HAL probe. The
selector at `g_ic + 0x74` remains semantically untyped, so this is bounded
RX-policy register evidence, not proof that mode two means AP+STA. Public
AP+STA remains disabled because the shared one-channel PHY owner, beacon/TX/RX
scheduling and a combined lifecycle have not been implemented or qualified.

Internal Wi-Fi/BLE coexistence is also only partially closed. The Wi-Fi MAC
PTI leaves and all five COEX hardware-timer control banks are qualified. The
focused `coex-core` suite also proves the complete 48-entry PTI query, the
five accepted event-duration selectors and the release transaction. The
request transaction matches all 100 concrete valid-clock cases; its overall
result remains incomplete only for five Rust fail-closed clock-error branches
outside the vendor valid-clock precondition. The remaining low-level blockers
are the semaphore-backed scheduler environment, indirect adapter calls and
the BLE-side PTI/request path. External coexistence pins are a separate
platform boundary and are not required for the internal Wi-Fi/BLE target.

Not implemented:

- AP+STA, AP HT, power save and raw injection;
- public BLE/BT/802.15.4 or coexistence runtime ownership (the Wi-Fi-side
  register leaves and COEX timer-control sub-boundaries are implemented, but
  the joint scheduler/lifecycle and hardware evidence are not);
- cold hardware replay of the caller-owned calibration artifact; supplied
  artifacts currently force full calibration and are replaced;
- ESP32-C5 backend.

The typed STA -> idle -> scan -> monitor -> idle -> STA ownership round trip is
qualified. UDP/TCP/ICMP throughput and faulted-quiescence remain independent
qualification cells; a role transition does not imply datapath performance.
AP lifecycle, ICMP, UDP and TCP scenarios exist, but require a dated successful
hardware record before this revision is called AP-qualified.
