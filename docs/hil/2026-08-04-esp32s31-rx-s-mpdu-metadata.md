# ESP32-S31 RX S-MPDU metadata

Qualification ID: `HIL_ESP32S31_RX_S_MPDU_METADATA_2026_08_04`.

This cell qualifies exact transport and provenance of the S31
`cur_single_mpdu` status. It also records why that bit must not be exposed as
the inverse of physical A-MPDU membership.

## Source and corrected meaning

Complete `_oracles/libpp.a[hal_debug.o]::dbg_dump_rx_ppdu` loads RX-prefix
byte `0x1f`, masks bit zero and prints it as `cur_single_mpdu`. The pinned
Espressif `esp_wifi_he_types.h` defines the field as indicating whether the
received MPDU is an **S-MPDU**. IEEE uses S-MPDU for the VHT/HE single-MPDU
A-MPDU form whose delimiter EOF bit is set; it is not a generic ordinary-MPDU
flag.

The portable metadata therefore carries independent fields:

- `s_mpdu` preserves the exact hardware status and its provenance;
- `ampdu` remains unavailable for HE rather than being inferred by negating
  `s_mpdu`;
- for HT only, the decoder obtains `ampdu` directly from HT-SIG Aggregation
  bit 27, independently proved by the format-two branch of
  `dbg_dump_rx_ppdu`.

The same S-MPDU record crosses `NetworkRxFrame`, connected protected-data and
Beacon parsing, and `ConnectedRxEvent`. HIL only counts this already-normalized
value. The host gate requires one record per accepted interval, observations
for both benchmark data and connected Beacon classes, and zero unavailable
provenance.

## Cell

- Board: ESP32-S31 revision 0.0, MAC `30:ed:a0:f3:f6:d0`.
- Peer: FRITZ!Box 7530 FN, HE20 downlink, channel 1 during this run.
- Memory profile/scenario: `psram-code-psram-data` / `open-radio-hil`.
- Runtime CRC32: `ae2f0d15`.
- Repository base commit: `bbd9c9b7a5af23db951a6f03fa738f4223b99997` plus the
  working-tree implementation described by this record.

Credentials were supplied through the typed runtime provisioning protocol and
are intentionally absent from the command and artifacts:

```text
cargo hil flash radio --port /dev/ttyACM0
cargo hil traffic rx 192.168.178.130 \
  --phy he20 --rate 75M --seconds 15 --payload 1200 \
  --serial /dev/ttyACM0
```

## Result

- Result: `PASS`.
- Host offer/device receive: 75.000/75.001 Mbit/s.
- Exact delivery: 117,189 payload datagrams and 140,626,800 bytes, with no
  software drop, `BUFFER_FULL` or FIFO overflow.
- Benchmark data: zero S-MPDU, 117,190 not-S-MPDU and zero unavailable. The
  extra observation is the negative terminal datagram, which deliberately
  crosses RX before throughput accounting excludes it.
- Connected Beacon: zero S-MPDU, 147 not-S-MPDU and zero unavailable.
- The independent ARP-prime reply also reported `rx_s_mpdu=0`.
- All 1,831 sampled useful PHY records were HE-SU MCS9.

Artifact hashes:

```text
application.bin  c81346858df6a5d6f5cfccbbc4bf06e462e608cf0266a5879d8980641c1c337a
uart.log         501ae7611f0891eed220017b2d1e12fe59758073cd947625c1dc6dfd8a355ef3
report.md        99265cb7d0dc262569761fb867dc5f9ae899d80b96ee34dd95ea5881ebe258b6
```

## Remaining boundary

This cell intentionally makes no HE A-MPDU claim. The earlier tentative
`!cur_single_mpdu => ampdu` interpretation was rejected when real Beacon
management frames produced the same clear value as sustained HE data. The
next direct cells are an HT downlink qualification of the independently
decoded HT-SIG Aggregation bit and recovery of an equally direct HE physical
aggregation source. BA agreement state, reorder activity and traffic shape
are useful context but are not substitutes for that source.
