# ESP32-S31 HE RX A-MPDU containment

Qualification ID: `HIL_ESP32S31_HE_RX_AMPDU_CONTAINMENT_2026_08_04`.

This cell qualifies the portable meaning and provenance of
`MacRxMetadata::ampdu` for an HE PPDU. The field means that the received MPDU
was carried in an A-MPDU container. It does not claim the number of MPDUs in
that container.

## Standard and hardware boundary

IEEE 802.11ax-2021 clause 26.6 defines A-MPDU operation in an HE PPDU. The
standard's VHT/HE single-MPDU definition likewise describes an S-MPDU as the
only MPDU in an A-MPDU carried by a VHT or HE PPDU. Therefore:

- the S31 `cur_bb_format` RXVECTOR field hardware-observes an HE PPDU;
- the standard-defined format then establishes A-MPDU containment as
  `MacRxEvidence::ProtocolValidated(true)`;
- this is not exposed as `HardwareObserved`, because S31 has no independent
  HE Aggregation status bit in the public RX prefix;
- `cur_single_mpdu` remains a separate hardware observation and is not
  inverted to produce `ampdu`;
- the value does not establish whether the A-MPDU contains one or several
  MPDUs.

The host gate requires one `ORXAG` record per qualified interval, exact
agreement between totals and provenance classes, at least one
protocol-validated HE A-MPDU observation, and zero hardware, false or
unavailable HE classifications.

Standard references:

- [IEEE 802.11ax-2021 preview, clause 26.6 table of contents](https://thewifiofthings.com/wp-content/uploads/2021/08/802.11ax-2021-Preview.pdf);
- [IEEE TGax S-MPDU definition and A-MPDU context change](https://ptacts.uspto.gov/ptacts/public-informations/petitions/1558071/download-documents?artifactId=sKXJWXaoI9zg3yliRLd8xehrnWoL9VSvRckcfyoidmo24wDLLuotK5Y).

## Cell

- Board: ESP32-S31 revision 0.0, MAC `30:ed:a0:f3:f6:d0`.
- Peer: FRITZ!Box 7530 FN, HE20 downlink.
- Useful RX vector: HE-SU MCS9, 2xLTF/0.8-us GI.
- Memory profile/scenario: `psram-code-psram-data` / `open-radio-hil`.
- Runtime CRC32: `29030389`.
- Repository base commit: `bbd9c9b7a5af23db951a6f03fa738f4223b99997` plus the
  working-tree implementation described by this record.

Credentials were supplied through runtime provisioning and are intentionally
absent from the command and artifacts:

```text
cargo hil flash radio
cargo hil traffic rx 192.168.178.130 \
  --phy he20 --rate 75M --seconds 15 --payload 1200 \
  --serial /dev/ttyACM0
```

## Result

- Result: `PASS`.
- Host offer/device receive: 75.000/75.143 Mbit/s.
- Exact delivery: 117,189 payload datagrams and 140,626,800 bytes.
- The extra negative terminal datagram crossed normalized metadata but was
  excluded from payload accounting.
- A-MPDU totals: 117,190 true, zero false and zero unavailable.
- Provenance: zero hardware true/false and 117,190/zero protocol
  true/false.
- S-MPDU remained independently zero/117,190/zero for
  S-MPDU/not-S-MPDU/unavailable.
- All 1,831 sampled useful RX vectors were HE-SU MCS9.
- Software queue drops, `BUFFER_FULL` and FIFO overflow were all zero.

Artifact hashes:

```text
application.bin  b456026c95de91b9b2b3e46acafe06d1bb197efa658a4659026f687d1e66f0dd
uart.log         d085f7e205f1ab62164caad9f68908bd809fe71bd6363cd5124df34fa9469151
report.md        f0cdfa5c3a0ecef7cbce8a5a0c764c930572a8393d24d0db657a48f9fbc5d270
```

## Remaining boundary

This closes A-MPDU containment for known legacy, HT, VHT and HE PPDU
formats. It deliberately does not publish A-MPDU cardinality, delimiter
position or first/last-subframe status. Those require another direct
hardware/parser source if a future HMAC consumer actually needs them. Unknown
future S31 baseband formats remain `Unavailable`.
