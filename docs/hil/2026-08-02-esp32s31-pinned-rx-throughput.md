# ESP32-S31 pinned-RX throughput qualification

Qualification ID: `HIL_OPEN_HE20_PINNED_RX_2026_08_02`.

## Change under test

The former high-throughput network boundary queued
`EthernetFrame<1600>` by value. Every publication initialized and filled a
complete local frame and then moved the large value into the Embassy channel.
The measured `copy+publish` phase was the largest controllable RX phase.

`PinnedResources` now owns final RX slots plus bounded free/ready index
channels. The protocol publisher reserves one slot, copies the Ethernet header
and payload directly into it, and publishes only its `u8` index. A unique
`PinnedReceiveToken` returns the slot after `embassy-net` consumes or drops it.
The Wi-Fi DMA buffer still does not escape the radio owner: DMA-to-staging copy
and descriptor recycle retain the recovered vendor ownership order.

The RX resources remain ordinary CPU-owned PSRAM. Only the separate TX pool
is placed in DMA-visible internal SRAM. The HIL placement and autonomous source
audits both passed.

## Exact cell

- Board: ESP32-S31 revision 0.
- Peer: FRITZ!Box 7530 on the ordinary LAN.
- Memory profile: `psram-code-psram-data`.
- PHY: HE20 SU, NSS1, MCS9, 2xLTF/0.8-us GI, nominal 114.7 Mbit/s.
- RX descriptor ring and vendor-equivalent large-RX staging pool: 32 entries.
- Host UDP payload: 1,200 bytes.

Firmware was built and flashed with credentials supplied only through the
environment:

```text
OPEN_RADIO_STA_SSID=<ssid> OPEN_RADIO_STA_PASSWORD=<password> \
  cargo hil flash radio --port /dev/ttyACM0
```

The stable RX run was:

```text
cargo hil traffic rx <device-ipv4> \
  --phy he20 --rate 75M --seconds 30 --payload 1200 \
  --serial /dev/ttyACM0
```

## RX result

- Result: `PASS`.
- Host offer/device receive: 75.000/74.999 Mbit/s for 30 seconds.
- Delivered payload: 281,251,200 bytes in 234,376 UDP datagrams; no missing
  terminal count.
- Network publications/software drops: 234,502/0.
- Hardware `BUFFER_FULL`/FIFO overflow: 0/0.
- HE useful-frame histogram: 234,377 frames at MCS9 and zero at every other
  MCS.
- DMA frontier/admitted maximum: 31/31, with no pool or queue credit limit.
- DMA-to-staging service: 16.74 us/frame average.
- Complete protocol dispatch: 24.78 us/frame average.
- Final network `copy+publish`: 12.96 us/frame average.
- Network-ready wait: 2.19 us average.

The earlier by-value boundary took 24.60 us/frame in `copy+publish` at
50 Mbit/s and 26.30 us/frame during a requested 60-Mbit/s saturated run. The
new boundary measured 12.25 us/frame at 50 Mbit/s and 12.27 us/frame at
60 Mbit/s. The former 60-Mbit/s run reached only 56.369 Mbit/s and recorded
17 `BUFFER_FULL` observations; the replacement delivered 60.001 Mbit/s with
zero starvation and all useful frames at MCS9.

Additional reset-separated steps passed cleanly at 50, 60, 70 and 75 Mbit/s.
At an 80-Mbit/s offer the application still received all 83,334 datagrams at
79.980 Mbit/s, but strict qualification rejected three `BUFFER_FULL`
observations when the frozen DMA frontier reached exactly 32/32. Therefore
75 Mbit/s is the current demonstrated lossless floor; 80 Mbit/s is a measured
descriptor-burst boundary, not a claimed stable rate.

## TX and simultaneous regression

The neighboring modes were rebuilt and flashed after the ownership change.

- TX-only: `PASS`; host/device floors 90.892/91.534 Mbit/s, 77,771 datagrams,
  zero missing or reordered. A-MPDU averaged 30.99 MPDUs; 3,764 aggregates had
  31 members and one had 32. Preparation/publication averaged 303.37/23.64 us.
- Bidirectional: `PASS`; 9.999-Mbit/s RX plus an 80.571-Mbit/s concurrent TX
  floor, for a conservative 90.570-Mbit/s sum. RX and TX had no terminal
  hardware failure. The report now includes both RX phase telemetry and TX
  A-MPDU preparation/publication/exchange timing.

## Artifact identity

- RX UART/report SHA-256:
  `e22f1f8cc6d899bccbaa0e7c3d90ebc740c294d64ff0c81ef4a1f0607ac278ae` /
  `c576a1995eddd6e5b14ec4d2b0655d93a02d3698cf414b0766e45697bf73065b`.
- TX UART/report SHA-256:
  `4925df4140940adc07adda99cf46ee2b55c7178cc2e6f7f127778300614211b4` /
  `58b794e2d5b1a046f9a5bc9fc5501a6a3468a83474d5fe13e2736adc33df9a68`.
- Bidirectional UART/report SHA-256:
  `d6ed63188a903349631435d4e6abaadae100745d335a50d238abfb1516154ba6` /
  `885992a0caeab77a7ade85c55917fb8067be9fdb0187a1a94f72392120b88833`.

Generated UART logs and reports remain under `target/hil/esp32s31/qualification`;
this record preserves their exact identity.

## Remaining boundary

The next performance question is no longer the Embassy queue depth. No RX
network queue or staging credit was exhausted in any successful run. The
80-Mbit/s edge coincides with the complete 32-descriptor frontier and the
recovered 32-object large-RX profile. Increasing that geometry is not a safe
generic tuning knob: it needs separate vendor-oracle, SRAM-placement and HIL
qualification. A lower-risk next experiment is direct protocol decode into a
reserved final RX slot, followed by executor/frontier latency measurement;
the DMA-to-staging ownership copy must remain until an alternative is proven
against the vendor recycle boundary.
