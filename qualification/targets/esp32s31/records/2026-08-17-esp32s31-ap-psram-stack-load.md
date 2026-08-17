# ESP32-S31 AP PSRAM-stack load characterization

Date: 2026-08-17

Evidence ID: `HIL_ESP32S31_AP_PSRAM_STACK_LOAD_2026_08_17`

## Cell

- target: ESP32-S31 revision 0.0 operating as a WPA2 access point;
- PHY: 2.4 GHz HT40; the UDP cells require observed MCS7 traffic;
- firmware base: `cdae775a`, plus the AP HIL evidence/report changes recorded
  in the same change set;
- runtime profile: `psram-code-psram-data-psram-stack`;
- runtime CRC-32: `28acdee1`;
- HIL protocol: v53;
- CPU0 ordinary stack: 192 KiB PSRAM;
- CPU1 ordinary stack: 16 KiB PSRAM;
- interrupt stacks: one 32-KiB internal-SRAM stack per hart;
- initial AP RX peer: the laboratory OpenWrt station;
- initial AP TX, bidirectional and ICMP peer: the controlled Linux station;
- controlled repeat: OpenWrt is the Wi-Fi peer for every AP and STA cell;
  the Linux host remains only the wired traffic generator/receiver.

The workload matrix used reset isolation, three repetitions and two requested
AP epochs per repetition. A repetition which missed a throughput criterion
failed closed after its first measured epoch. Credentials and fixture addresses
remained host-owned and are not retained in this record.

## Commands

```console
cargo hil run access-point-single-client-ceiling-rx-psram-stack
cargo hil run access-point-single-client-ceiling-tx-psram-stack
cargo hil run access-point-single-client-ceiling-bidirectional-psram-stack
cargo hil run access-point-icmp-psram-stack
```

## UDP results

| Cell | Offered load | Observed payload throughput | Result |
| --- | ---: | ---: | --- |
| AP RX | 120 Mbit/s | 75.931 / 75.883 / 77.650 Mbit/s; median 75.931 Mbit/s | 0/3 PASS; below the 95-Mbit/s floor |
| AP TX | 130 Mbit/s | 109.989..115.161 Mbit/s across six epochs; median 112.918 Mbit/s | 3/3 PASS |
| AP RX+TX | 100+100 Mbit/s | RX 75.552 / 75.799 / 73.936 Mbit/s; TX 2.516 / 3.270 / 3.292 Mbit/s | 0/3 PASS; TX is below the 40-Mbit/s floor |

The TX-only result proves that the PSRAM-stack image can sustain more than 100
Mbit/s from AP to station. Under simultaneous saturated RX, AP TX collapses to
2.5--3.3 Mbit/s while RX retains 73.9--75.8 Mbit/s. This is an asymmetric
full-duplex scheduling/backpressure defect, not a general inability to publish
HT40 MCS7 aggregates: the failed bidirectional runs still recorded 107, 139
and 140 MCS7 aggregates respectively.

RX-only remained below the required 95 Mbit/s despite receiving predominantly
HT40 MCS5--MCS7 A-MPDU traffic. The three OpenWrt measurements reported
20,891--28,411 station retries and 40--52 radio FCS errors. No target MIC
failure, quarantine, protocol rejection or radio rejection was observed.

## ICMP latency

The ICMP cell used 100 requests per AP epoch, a 56-byte payload, 20-ms
interval and 1-s timeout. All six epochs completed 100/100 replies with no
loss. Percentiles use the same nearest-rank definition as the station HIL.

| Repetition / epoch | p50 | p95 | p99 |
| --- | ---: | ---: | ---: |
| 1 / 1 | 5.410 ms | 7.820 ms | 12.600 ms |
| 1 / 2 | 5.330 ms | 9.240 ms | 13.800 ms |
| 2 / 1 | 5.290 ms | 9.190 ms | 12.000 ms |
| 2 / 2 | 5.280 ms | 9.380 ms | 21.400 ms |
| 3 / 1 | 5.360 ms | 8.310 ms | 14.700 ms |
| 3 / 2 | 5.420 ms | 8.020 ms | 12.300 ms |

The medians of the six per-epoch percentile values are p50 5.345 ms, p95
8.750 ms and p99 13.200 ms. They are not a percentile recomputation over one
merged 600-sample population. The p50 result is materially worse than the
2.573--2.831-ms station median recorded by
`HIL_ESP32S31_HT40_DATAPATH_2026_08_13`; a same-image station control is
required before attributing the difference solely to the AP path.

Short ICMP replies are not an A-MPDU/MCS7 qualification workload. The AP ICMP
contract therefore requires HT40 and latency/loss evidence but does not require
an MCS7 aggregate. UDP ceiling cells retain the MCS7 gate.

## Stack and control-plane observations

- CPU0 minimum free ordinary-stack space was 60,648 bytes out of 196,608;
- CPU1 minimum free space was 6,040 bytes during UDP and 7,344 bytes during
  ICMP, out of 16,384;
- all completed AP role transitions retained the stack policy minimums;
- the framed HIL control plane reported no checksum, decode or sequence error;
- no 64-KiB internal-SRAM task-stack reservation was restored.

This record characterizes the current defects; it does not qualify the AP RX
or bidirectional throughput claims and does not modify the qualification
ledger.

## Controlled OpenWrt RF-peer repeat

The complete AP matrix was repeated after making the peer topology uniform.
OpenWrt created the managed interface associated to the DUT AP. The Linux host
generated and received traffic over Ethernet through scoped OpenWrt
forwarding; the forwarding table and firewall chain were removed after every
AP epoch. OpenWrt, rather than the Linux WLAN, therefore owned every measured
802.11 exchange. This removes the peer implementation as a difference between
AP directions without making the router CPU the traffic generator.

| Cell | Offered load | Observed payload throughput | Result |
| --- | ---: | ---: | --- |
| AP RX | 120 Mbit/s | 74.364 / 74.567 / 73.861 Mbit/s; median 74.364 Mbit/s | 0/3 PASS; below 95 Mbit/s |
| AP TX | 130 Mbit/s | 114.145 / 114.068 / 99.809 / 107.754 / 114.786 / 114.396 Mbit/s; median 114.107 Mbit/s | 3/3 PASS |
| AP RX+TX | 100+100 Mbit/s | RX 73.606 / 71.931 / 73.785 Mbit/s; TX 2.434 / 3.346 / 1.893 Mbit/s | 0/3 PASS; TX below 40 Mbit/s |

The controlled repeat reproduces both material defects. RX-only remains near
74 Mbit/s and concurrent saturated RX still collapses AP TX to 1.9--3.3
Mbit/s, while TX-only reaches 99.8--114.8 Mbit/s. The Linux-WLAN peer used by
the initial cell was therefore not the cause.

All six controlled AP ICMP epochs delivered 100/100 replies:

| Repetition / epoch | p50 | p95 | p99 |
| --- | ---: | ---: | ---: |
| 1 / 1 | 4.610 ms | 6.380 ms | 8.370 ms |
| 1 / 2 | 4.630 ms | 6.430 ms | 7.990 ms |
| 2 / 1 | 4.600 ms | 6.390 ms | 7.370 ms |
| 2 / 2 | 4.560 ms | 5.910 ms | 8.640 ms |
| 3 / 1 | 4.670 ms | 7.830 ms | 13.400 ms |
| 3 / 2 | 4.480 ms | 6.050 ms | 6.430 ms |

The medians of the six per-epoch values are p50 4.605 ms, p95 6.385 ms and
p99 8.180 ms. This improves on the mixed-peer AP cell but remains slower than
the historical station baseline.

## Same-code station controls

The station controls use the inverse RF role with the same physical topology:
the DUT is an HT40 station, OpenWrt is its AP, and the Linux host is connected
to OpenWrt by Ethernet. Three reset-separated repetitions were run for each
cell.

| Runtime | Cell | Result |
| --- | --- | --- |
| PSRAM stack | RX-only, 120-Mbit/s offer | 73.608 / 74.730 / 75.557 Mbit/s; 0/3 PASS against the 100-Mbit/s floor |
| PSRAM stack | TX-only, 130-Mbit/s offer | 118.161 / 116.866 / 117.966 Mbit/s; 3/3 PASS, no missing/reordered/duplicate datagrams |
| PSRAM stack | RX+TX, 40+40 Mbit/s | RX 39.906 / 39.960 / 39.877; TX 40.025 / 40.036 / 40.036 Mbit/s; 3/3 PASS |
| PSRAM stack | 100 ICMP | 0 loss; p50 4.421 / 4.451 / 4.434 ms; p95 5.295 / 6.738 / 5.224 ms |
| ordinary stack | RX-only, 120-Mbit/s offer | 110.405 / 110.696 / 109.627 Mbit/s; only 1/3 PASS because attempts 2/3 reported DMA `buffer_full=1/6` |
| ordinary stack | 100 ICMP | 0 loss; p50 4.622 / 4.489 / 4.482 ms; p95 5.299 / 7.392 / 5.048 ms |

The ordinary-stack A/B recovers the historical RX throughput range, proving
that the stable 74--76-Mbit/s ceiling is introduced by ordinary execution on
the PSRAM task stack rather than by the RF peer. It does not clear the current
driver: two ordinary-stack attempts observed RX DMA buffer exhaustion,
sequence gaps and backward arrivals. This is a separate common RX ownership
regression. The current ordinary-stack ICMP median is also about 4.5 ms rather
than the 2.573--2.831-ms range recorded on 2026-08-13; this cell establishes
the regression but does not yet attribute it to one code change or fixture
timing change.

## AP RX protocol hot-text A/B

Task-poll evidence localized the PSRAM-stack AP RX ceiling above the common
DMA producer. In the unmodified diagnostic image, three reset-separated
epochs delivered 62.230--66.080 Mbit/s. DMA service consumed only
2.31--2.42 seconds of each 16-second interval while the AP radio future was
resident for 15.10--15.45 seconds. Unlike the station, AP protocol processing
currently runs inline in that radio future. The monomorphized
`service_staged_rx` routine occupied `0x3060` bytes of external executable
text.

An algorithm-preserving A/B placed only `service_staged_rx` in the semantic
`.hot.text.open_radio_ap_rx` class. The PSRAM-code board linker mapped that
12,384-byte routine to internal executable SRAM; descriptor geometry, staging
capacity, negotiated BA window, traffic offer and scheduler budget were
unchanged. The diagnostic task-poll instrumentation remained enabled.

| Repetition / epoch | Payload throughput | Hardware BUFFER_FULL | HT40 MCS7 share |
| --- | ---: | ---: | ---: |
| 1 / 1 | 102.958 Mbit/s | 13 | 99.018% |
| 1 / 2 | 100.950 Mbit/s | 2 | 99.172% |
| 2 / 1 | 104.359 Mbit/s | 0 | 98.780% |
| 2 / 2 | 102.447 Mbit/s | 0 | 99.220% |
| 3 / 1 | 103.452 Mbit/s | 0 | 98.985% |
| 3 / 2 | 102.867 Mbit/s | 0 | 98.838% |

All three repetitions passed the 95-Mbit/s criterion and all six epochs had
no beacon loss, MIC failure, quarantine, radio rejection or protocol
rejection. This isolates the prior MCS fallback as a consequence of AP
protocol execution latency: late descriptor return caused BUFFER_FULL and
missing BlockAck coverage, the peer retried and selected lower MCS values, and
the lower goodput increased backpressure. It was not a fixed PHY limit or a
need for a BA window larger than 16.

The remaining two non-zero BUFFER_FULL epochs mean this is not yet a
zero-loss qualification result. The A/B nevertheless explains why an earlier
isolated run could reach roughly 110 Mbit/s: the same datapath enters a fast
regime when the AP protocol working set remains serviceable quickly enough;
PSRAM instruction-fetch/cache pressure made that regime unstable.

```text
hot-text runtime CRC-32  8ffd0619
hot-text image SHA-256   e1e1c08176752c9519269e82b13356b4b49d234bea00ba44c32fad97cd43c575
hot-text result SHA-256  b9fad98bb7010a4d024f73ca7ea5a2944b9709da87318daf1430d5249c0835c7
```

## AP RX source-moderation A/B

The hot-text image was then changed to retain NAPI-style RX source
moderation. The first RX status masks `RX_SUCCESS` together with the two
RX-associated auxiliary sources observed under saturation (interrupt bits 5
and 24). DMA remains live. The WDEV task services bounded 64-frame turns,
keeps the group masked across `ProbePending` and capacity backpressure, and
restores it only after `Drained`. TX interrupt sources remain enabled
throughout. A status arriving while masked remains latched and asserts the S31
level CPU route after unmask.

Masking only `RX_SUCCESS` was insufficient: it reduced useful RX posts to
447--942 but left 119,820--122,908 auxiliary-only ISR entries and delivered
only 87.862--90.059 Mbit/s. Treating the three observed RX sources as one
moderated delivery group removed that ineffective interrupt load.

| Repetition / epoch | Payload throughput | MAC ISR entries | Hardware BUFFER_FULL |
| --- | ---: | ---: | ---: |
| 1 / 1 | 98.631 Mbit/s | 946 | 5 |
| 1 / 2 | 101.509 Mbit/s | 543 | 3 |
| 2 / 1 | 97.717 Mbit/s | 839 | 14 |
| 2 / 2 | 100.123 Mbit/s | 402 | 6 |
| 3 / 1 | 98.047 Mbit/s | 349 | 12 |
| 3 / 2 | 101.234 Mbit/s | 503 | 4 |

All three repetitions passed. Median payload throughput was 99.377 Mbit/s.
The unmoderated hot-text A/B entered the ISR roughly 147,000--151,000 times per
epoch; the final moderated image entered it 349--946 times, a reduction of
more than 99%, while retaining the 95-Mbit/s floor and bounded beacon timing.
Removing an unnecessary software probe after the final unmask reduced radio
future polls without relying on a fabricated completion edge.

Source moderation addresses CPU interrupt amplification, not the remaining
occasional descriptor exhaustion: BUFFER_FULL remained 3--14 in these cells.
The result is therefore still characterization rather than a zero-loss
qualification claim.

```text
moderated runtime CRC-32         53d5321b
moderated image SHA-256          a3ba690a7bfcbb9b2e3888b9db913995a9d33f2fe30201e39259b4a460c83e08
moderated attempt-1 protocol     e4d555d01046013cfcd9ccd693e77be110eac4a818c71081ccafa39a8838cb88
```

## Repeated moderation and stack control

A later same-day repeat used the final moderated image after the guarded
cursor-reclaim changes. All six PSRAM-stack epochs had zero hardware
BUFFER_FULL, 99.15--99.98% HT40 MCS7, and mean RSSI near -17.8 dBm.

| Stack / RX source policy | Six payload-throughput epochs | Median | MAC ISR entries / epoch |
| --- | --- | ---: | ---: |
| PSRAM / moderated | 98.940 / 100.633 / 97.026 / 101.020 / 96.925 / 99.055 Mbit/s | 99.055 Mbit/s | 543--1,476 |
| SRAM / moderated | 95.849 / 95.457 / 100.761 / 99.628 / 98.549 / 97.332 Mbit/s | 98.549 Mbit/s | 2,947--3,746 |
| SRAM / unmoderated A/B | 95.749 / 99.679 / 100.438 / 101.456 / 101.892 / 100.115 Mbit/s | 100.438 Mbit/s | 139,244--148,232 |

The SRAM A/B changed only whether AP startup activated RX source moderation.
It therefore measures about 1.9 Mbit/s of median cost for the current
moderation/scheduling policy while showing a greater than 97% reduction in
hard-IRQ entries. Removing moderation did not recover 110 Mbit/s and did not
change the zero-BUFFER_FULL result. The remaining difference is outside the
source-masking mechanism.

The repeat also corrects an easy comparison error: the reproducible
109.627--110.696-Mbit/s ordinary-stack values earlier in this record are the
station control, not an AP baseline. The AP hot-text result before moderation
was 100.950--104.359 Mbit/s; the approximately 110-Mbit/s AP observation was
an isolated earlier cell and is not a reproducible reference.

Blobray inspection of the complete vendor path shows a different moderation
strategy. `wDev_ProcessFiq` acknowledges the complete status snapshot, posts
RX signal `0x19`, and loops while interrupt state remains pending; it does not
mask the RX delivery group across task execution. `pp_post` coalesces/counts
the signal and wakes `ppTask`. The `wdevProcessRxSucDataAll` task then walks to
the frozen hardware descriptor frontier. Thus the vendor bounds duplicate
task work and drains descriptors, but static code alone cannot establish the
number of hardware IRQ entries under load.

```text
moderated SRAM runtime CRC-32     55ab5b61
unmoderated SRAM runtime CRC-32   9240ff3
restored PSRAM runtime CRC-32     53d5321b
```

## Artifact identities

```text
RX result                 717ca649824189600e215b608a5c0a544ff2263b84ae654f8045ae731e8c9a0f
RX attempt-1 protocol     1ef4b6f3c08bf2e63ae076c799dbef57955c13e1ff51938d600269ada68eafff
TX result                 a66ef853f8a34ddfe8740e6d5a65ca037ec412c58f5c93815c72d670046534a1
TX attempt-1 report       14741be87bf7641fa137add6daf6aab86949036ec23847725ad42478fbceb7c8
bidirectional result      f74c1278548f50424f9c1f3daf7cfac5e4b4b4c1fb647f864a77454ef515efd7
bidirectional protocol    9b9d685b6a5850b739d7194f8e00401d2e0a53358dba53478adfd0551a3c64b0
ICMP result               8955ff9525bd5b0b9f7b65c9d473713f33c9c31ca9bbf2e0cd8cb9ee4b51f906
ICMP attempt-1 report     0cc61e7552c1c35978e61035ccf36964a6e17ada681fe0ef5bf619f234975fcd

controlled AP RX result  dde609806ad390a541e67003cd6b9f359058d30051c9ae17460a220ddf2418db
controlled AP TX result  a66ef853f8a34ddfe8740e6d5a65ca037ec412c58f5c93815c72d670046534a1
controlled AP bidi result 97b62ec8166b259566f7dcf9e3911d6354179a67c90b6bb7c4384158da6749b5
controlled AP ICMP result 8955ff9525bd5b0b9f7b65c9d473713f33c9c31ca9bbf2e0cd8cb9ee4b51f906
PSRAM STA RX result       c8a118df6229850f0f37eed7172468c680db4f7adf76ccc8269b5e204a2ce028
PSRAM STA TX result       2ed0639f00869ed2c7bcb6f0e07de042116d41a063cacd10cd3e267fb66dc7d0
PSRAM STA bidi result     cc874e4b6080d2ce2aa2155d2b43a0ee98c35a9862d5b5b7af9a0efd7b38a847
PSRAM STA ICMP result     54eb298e57c973e3718c04bebd4cc61763353ff7c4e7b44e56ea9a82595cdb04
ordinary STA RX result    5076cb39d786f0f746dbecd1860e51883f76489387b9262b7560805a0de3b8c7
ordinary STA ICMP result  c5f09ff25bea3ac31d762844871ce53e81d55d62215d4a52e5c8f04ccf6a83f9
```
