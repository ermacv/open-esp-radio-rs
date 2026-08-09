# ESP32-S31 Wi-Fi status

The [qualification ledger](../qualification/targets/esp32s31/wifi-sta.ledger)
is authoritative for measured behavior and dated evidence.

Implemented:

- PAC-backed RF/PHY start, retained calibration and channel selection;
- WPA2 STA scan/join/reconnect and an application-owned `embassy-net` stack;
- finite application scan with a bounded report and `Idle -> Scan -> Idle`
  ownership round trip;
- QoS, BlockAck, bounded TX A-MPDU and RX reorder/handoff;
- exclusive normalized monitor capture with bounded, non-blocking overflow;
- one public `Radio -> Wifi -> STA|Monitor` lifecycle and one runner owning
  PAC, DMA and ISR until cooperative quiescence;
- feature-gated qualification events; production builds contain no logging.

Not implemented:

- AP/AP+STA, power save, raw injection, BLE/BT/802.15.4/coexistence;
- ESP32-C5 backend.

The typed STA -> idle -> scan -> monitor -> idle -> STA ownership round trip is
qualified. UDP/TCP/ICMP throughput and faulted-quiescence remain independent
qualification cells; a role transition does not imply datapath performance.
