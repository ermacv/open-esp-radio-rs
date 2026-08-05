# ESP32-S31 direct HT RX aggregation metadata

Qualification ID: `HIL_ESP32S31_HT_RX_AGGREGATION_METADATA_2026_08_04`.

This cell qualifies the portable RX `ampdu` field against a direct physical
source. It does not infer aggregation from BlockAck state, reorder activity,
traffic rate or the separate Espressif `cur_single_mpdu` status.

## Direct source and contract

Complete `_oracles/libpp.a[hal_debug.o]::dbg_dump_rx_ppdu` decodes the HT
format-two prefix and prints word bit 27 as `Aggregation`. The S31 RX decoder
exposes that exact HT-SIG bit as `MacRxMetadata::ampdu` with
`MacRxEvidence::HardwareObserved`. Other physical formats remain
`Unavailable` until an equally direct source is recovered.

The HIL observer consumes the normalized metadata after the production RX
handoff. It does not re-read the S31 prefix. One `ORXAG` record is required
for every qualified RX interval. An HT cell fails unless:

- at least one benchmark datagram has direct hardware provenance;
- no benchmark datagram has unavailable provenance;
- at least one benchmark MPDU has the Aggregation bit set.

The RX host selector now distinguishes HT20 from HT40 even though both use
baseband format two. This cell therefore proves HT aggregation on the
actually negotiated 20-MHz channel and makes no HT40 claim.

## Cell

- Board: ESP32-S31 revision 0.0, MAC `30:ed:a0:f3:f6:d0`.
- Peer: FRITZ!Box 7530 FN, forced HT20 association.
- Rate profile: HT MCS7, 400-ns GI, 72.2-Mbit/s nominal TX rate.
- Memory profile/scenario: `psram-code-psram-data` / `open-radio-hil`.
- Runtime CRC32: `ee02231c`.
- Repository base commit: `bbd9c9b7a5af23db951a6f03fa738f4223b99997` plus the
  working-tree implementation described by this record.

Credentials were supplied through runtime provisioning and are intentionally
absent from the commands and artifacts:

```text
OPEN_RADIO_FORCE_HT20=1 OPEN_RADIO_HT_SGI=1 cargo hil flash radio
cargo hil traffic rx 192.168.178.130 \
  --phy ht20 --rate 50M --seconds 15 --payload 1200 \
  --serial /dev/ttyACM0
```

## Result

- Result: `PASS`.
- The connected runtime reported `phy=ht20`, `bandwidth_mhz=20`, rate code
  `0x21` and 72,200-kbit/s nominal rate.
- All 1,220 sampled benchmark PHY records were baseband format two.
- Host offered 50.001 Mbit/s. The target measured 46.314 Mbit/s while
  receiving all 78,126 payload datagrams and 93,751,200 bytes exactly.
- The extra negative terminal datagram crossed normalized RX metadata but was
  excluded from payload accounting.
- Direct aggregation evidence was 78,127 A-MPDU, zero not-A-MPDU and zero
  unavailable observations.
- S-MPDU remained independently zero/78,127/zero for
  S-MPDU/not-S-MPDU/unavailable.
- Software queue drops, `BUFFER_FULL` and FIFO overflow were all zero.

Artifact hashes:

```text
application.bin  a0f621a2fdead9789ae30ac3b79295fe661dab886aa490256fbd5e19c808e4bf
uart.log         a5c514a8ead4db56c6284844b534621268d245fdbb995828830b5cc272b6c0e2
report.md        e90ed2dba4272dbc6b35e624d301c6d76218b732ed477c46c41911ce8f0353eb
```

## Remaining boundary

This closes only the direct HT RX Aggregation-bit boundary. HE RX `ampdu`
correctly remains `Unavailable`: HE data cannot be classified by negating
S-MPDU, by observing an active BA agreement or by assuming aggregation from
throughput. HT40 and the complete HT MCS/width/GI matrix also remain
independent qualification cells.
