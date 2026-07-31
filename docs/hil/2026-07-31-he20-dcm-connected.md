# HE20 connected DCM MCS0 HIL — 2026-07-31

Qualification ID: `HIL_OPEN_HE20_DCM_CONNECTED_2026_07_31`.

## Contract

- ESP32-S31 revision 0;
- source-only open PHY/MAC, no vendor radio initialization;
- `psram-code-psram-data`;
- runtime text in PSRAM, 10,976-byte internal-SRAM ISR frontier;
- controlled Linux AX211 HE20 AP on channel 11;
- peer advertises BPSK DCM receive and LDPC;
- BCC DCM MCS0, HE GI/LTF selector 1 (`2xLTF/0.8 us`);
- nominal RU242 rate 4.3 Mbit/s;
- protected HE A-MPDU with negotiated BlockAck;
- simultaneous 1-Mbit/s AP-to-device traffic;
- application TX producer paced at 750 kbit/s so this low-rate certification
  cell does not intentionally consume almost all channel airtime.

The pacing is HIL traffic policy. It does not alter EDCA, PHY selection,
descriptor format, retry behavior or the driver's nominal rate.

## Result

The strict Rust host qualifier passed:

```text
OPENRADIOHOST result=PASS mode=he20-bidirectional
offered_kbps=1000 rx_median_kbps=1002
concurrent_tx_floor_kbps=749 combined_floor_sum_kbps=1751
```

The corresponding device evidence contained:

```text
tx_rate=0x1a tx_rate_kbps=4300 he_gi_ltf=1 he_dcm=1 he_ldpc=0
subframes_avg=1 individual_retry=0 spill=0
buffer_full=0 fifo_overflow=0
```

Every one of fifteen consecutive independent Linux station-statistics samples
reported:

```text
rx bitrate: 4.3 MBit/s HE-MCS 0 HE-NSS 1 HE-GI 0 HE-DCM 1
```

This closes the ambiguity that an internal rate label alone could not close:
the peer decoded the actual over-the-air uplink as HE DCM.

Artifact hashes from
`target/xtask/esp32s31/open-radio-bidirectional`:

```text
report.md  54747254bdc66cc7c89ba12cd786a070fce0690e5fc2584c318768bad576bc26
uart.log   a7199629ccffcaa3f1c010cc4a4fab978aa5ad8d875a3d706a2d7f1a1d32fefa
```

The command was:

```text
cargo xtask open-radio bidirectional 10.42.0.138 \
  --rate 1M --seconds 12 --serial /dev/ttyACM0 --phy he20 \
  --expect-tx-kbps 4300 --expect-he-gi-ltf 1 \
  --expect-he-dcm 1 --expect-he-ldpc 0
```

## Defect found before qualification

The first connected run preclaimed two pinned network frames before applying
the rate-dependent HE APEP limit. ROM `he_max_apep_length` and complete
`libpp.a[pp_he.o]::ppCheckTxHEAMPDUlength` limit MCS0 DCM to 1,850 bytes:
one full-size protected Ethernet MPDU fits, two do not.

The rejected second lease was transmitted by the application's old generic
spill path at legacy OFDM 54 Mbit/s. This produced a misleading 5.371-Mbit/s
reported uplink while Linux alternated between `HE-MCS0 DCM1` and legacy
54-Mbit/s receive vectors.

`open_esp_radio::esp32s31::embassy_tx::ReferencedAmpduIngressPolicy` now owns
the distinction:

- HT preclaims the pair required by this aggregate adapter;
- HE starts with one lease;
- each further HE lease is removed from the network queue only after
  `ReferencedHtAmpduBatch::can_push_he` validates the exact APEP/TXOP and
  allocation capacity.

No legacy fallback remains for an HE spill. A regression fails closed instead
of silently changing the requested PHY.

An unpaced, saturated pure-DCM run then correctly reached about
3.2–3.3 Mbit/s payload throughput but starved the simultaneous downlink: a
4.3-Mbit/s station occupied most channel airtime with approximately 3.7-ms
PPDUs. That is a traffic-shape issue in this fixed-low-rate certification
scenario, not justification for restoring the incorrect 54-Mbit/s spill.
The final bounded 750-kbit/s producer proved bidirectional operation and zero
RX starvation without mixing PHY formats.
