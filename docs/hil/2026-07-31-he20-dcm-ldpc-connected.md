# HE20 connected LDPC DCM MCS0 HIL — 2026-07-31

Qualification ID: `HIL_OPEN_HE20_DCM_LDPC_CONNECTED_2026_07_31`.

## Contract

- ESP32-S31 revision 0;
- source-only open PHY/MAC, no vendor radio initialization;
- `psram-code-psram-data`;
- runtime text in PSRAM, 10,976-byte internal-SRAM ISR frontier;
- controlled Linux AX211 HE20 AP on channel 11;
- peer advertises BPSK DCM receive and LDPC;
- LDPC DCM MCS0, HE GI/LTF selector 1 (`2xLTF/0.8 us`);
- nominal RU242 rate 4.3 Mbit/s;
- protected HE A-MPDU with negotiated BlockAck;
- simultaneous 1-Mbit/s AP-to-device traffic;
- application TX producer paced at 750 kbit/s.

Pacing bounds only the HIL producer's offered load. It does not change EDCA,
PHY selection, descriptor format, retry behavior or the nominal rate.

## Result

The strict Rust host qualifier required the exact rate, GI/LTF, DCM and LDPC
selectors and passed:

```text
OPENRADIOHOST result=PASS mode=he20-bidirectional
offered_kbps=1000 rx_median_kbps=1001
concurrent_tx_floor_kbps=749 combined_floor_sum_kbps=1750
```

The device evidence included repeated complete samples:

```text
tx_rate=0x1a tx_rate_kbps=4300 he_gi_ltf=1 he_dcm=1 he_ldpc=1
subframes_avg=1 individual_retry=0 spill=0
buffer_full=0 fifo_overflow=0
```

Every one of fifteen consecutive Linux station-statistics samples reported:

```text
rx bitrate: 4.3 MBit/s HE-MCS 0 HE-NSS 1 HE-GI 0 HE-DCM 1
```

Linux does not expose the decoded payload-coding selector in this station
summary. LDPC is therefore established by the driver's typed
`HeDcmRate::ldpc` construction, the peer-LDPC capability gate and the strict
device telemetry; the AP observation independently establishes that the
over-the-air PHY remained HE DCM rather than a legacy fallback.

Artifact hashes from
`target/xtask/esp32s31/open-radio-bidirectional`:

```text
report.md  9a087acacbc53e2869688de1b2da375903f266bbf705ae08f8709562f4611fca
uart.log   a2e0babaa50368b3c21f794028c1cb6a452e33f1856b569e29b28d07f043dc40
```

The command was:

```text
cargo xtask open-radio bidirectional 10.42.0.138 \
  --rate 1M --seconds 12 --serial /dev/ttyACM0 --phy he20 \
  --expect-tx-kbps 4300 --expect-he-gi-ltf 1 \
  --expect-he-dcm 1 --expect-he-ldpc 1
```

## Driver ownership

No reusable behavior remained in the HIL application after this cell:

- `HeDcmRate::ldpc` constructs only standard-valid LDPC/DCM MCS values;
- `HeDcmRate::is_supported_by` requires both the peer DCM constellation and
  peer LDPC capability;
- `StaTxRatePolicy` preserves the explicitly selected LDPC coding instead of
  replacing it with the ordinary peer-derived HE coding;
- `ReferencedAmpduIngressPolicy` and `ReferencedHtAmpduBatch::can_push_he`
  retain the already qualified HE lease/APEP ownership boundary.

The application owns only compile-time vector selection, paced test traffic,
telemetry and the acceptance gate.
