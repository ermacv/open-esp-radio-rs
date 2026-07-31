# Driver-repository HE20 bidirectional HIL

This cell proves that the ESP32-S31 HIL system no longer needs the neighboring
`esp32s31_rust` application to build, flash or qualify the normal open-radio
data path. Both firmware stages and the Rust host runner came from
`open-esp-radio-rs`. The final image resolved the published `esp-hal`
`esp32s31-async-platform` branch at commit `4d42ec36249a4f10a43cc5fb63f54eb24252feea`;
the local sibling checkout was explicitly disabled for the build and flash.

## Exact cell

- Board: ESP32-S31 revision 0, MAC `30:ed:a0:f3:f6:d0`.
- Peer: FRITZ!Box 7530, BSSID `dc:15:c8:54:bc:1e`.
- Memory profile: `psram-code-psram-data`.
- PHY: HE20 SU, NSS1, MCS9, LDPC, 2xLTF/0.8-us GI, nominal 114.7 Mbit/s.
- MAC: 32-member A-MPDU with BlockAck and concurrent RX servicing.
- Downlink offer: 10 Mbit/s, 1,200-byte UDP datagrams, 12 seconds.
- Uplink source: the existing HIL synthetic Ethernet/A-MPDU producer.

The image was built and flashed with:

```text
OPEN_RADIO_STA_SSID=<ssid> OPEN_RADIO_STA_PASSWORD=<password> \
  cargo hil flash bidirectional --port /dev/ttyACM0
```

The traffic and strict qualification were run without shell helpers or the
old application `xtask`:

```text
cargo hil traffic bidirectional 192.168.178.141 \
  --phy he20 --rate 10M --seconds 12 --serial /dev/ttyACM0
```

## Result

- Host offer: 10.001 Mbit/s, 15,001,200 bytes / 12,501 datagrams.
- Device direct-RX median: 10.012 Mbit/s.
- Concurrent open-radio TX floor: 71.361 Mbit/s.
- Conservative sum: 81.373 Mbit/s.
- RX baseband format remained HE (`format=4`).
- TX vector remained HE20 MCS9 at 114.7 Mbit/s.
- Both captured RX runtime intervals reported `buffer_full=0` and
  `fifo_overflow=0`.
- Runtime code markers were in PSRAM; build-time placement audit separately
  required ISR, DMA and stack ranges to remain in internal SRAM.
- The strict host result was `OPENRADIOHOST result=PASS`.

## Artifact identity

- UART qualification log SHA-256:
  `bde1b2b49459543c521b203c0018c85839643ae62d716a1311e8ad1d286fa9e2`.
- ESP application image SHA-256:
  `70d414f3741f194d2a64a235a826dd530730e0d3d244238863d96c0cdbe8cf02`.
- Packed stage-two runtime SHA-256:
  `9124aa778033cb4737d1d7b6c918a640e557be999043cfab27df73baaeadd133`.

The bulky UART log and binaries remain generated artifacts under
`target/hil/esp32s31`; this record preserves their hashes and the exact cell.

## Ownership boundary

Reusable rate, aggregate, retry, pinned-frame and Embassy network ownership is
already in the driver crates named in the feature ledger. The final run also
used the driver-owned `select_sta_association` channel/CBW decision and
`StaAssociationRetrySchedule`. It now also uses `StaPeerScanPolicy` and
`StaPeerAssociationPlan` as the sole post-response join for HT A-MPDU, WMM,
HE BSS color/capabilities, peer QoS, link metric and rate-control state; the
former HIL copies were deleted before this qualification. `StaTxRuntimePolicy`
now owns that negotiated TX state and all four EDCA contention windows, while
`UnicastRetryState` owns the bounded attempt count, exact legacy/HT rate
selection and success/failure CW transitions. The HIL retains only platform
entropy, DMA/IRQ waiting, PHY power application and Retry-bit publication.
Board/bootstrap setup, credentials, synthetic traffic generation and evidence
reporting also remain HIL concerns. The authentication/WPA2 executor
orchestration is tracked separately and must be extracted before the old
application HIL can be deleted.
