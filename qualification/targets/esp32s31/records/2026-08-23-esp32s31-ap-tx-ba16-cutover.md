# ESP32-S31 AP TX BA16 datapath cutover

Date: 2026-08-23

Evidence ID: `HIL_ESP32S31_AP_TX_BA16_CUTOVER_2026_08_23`

This record closes the implementation sequence proposed by
`HIL_ESP32S31_AP_TX_CEILING_ANALYSIS_2026_08_23`. It is a performance and
architecture characterization, not a new capability-ledger promotion. The
source revision is the commit containing this record; the exact HIL images are
bound below by runtime CRC and application SHA-256.

## Contract held constant

- PSRAM code, data and ordinary task stacks;
- internal SRAM only for interrupt/critical storage and measured hot text;
- HT40, MCS7 and short guard interval;
- TX BlockAck target of sixteen MPDUs;
- A-MSDU disabled and outside this change;
- OpenWrt as the sole wireless AP client and the laptop as wired traffic
  generator;
- one physical RX producer, TX owner and IRQ epoch;
- no shortening of the reviewed `hal_mac_tx_set_ppdu` MMIO sequence.

## Implemented cutover

The old relative AP control polling path was removed. AP control now exposes
one absolute deadline; DATAPATH owns a readiness latch and a typed active TX
origin (`Network` or `Control`). A terminal network-data TX does not re-arm
control service, while a control-owned TX and an RX protocol turn do.

AP status publication checks the revision before copying the status snapshot,
and link state is derived from that same revision. The AP service caches its
minimum operational TX BlockAck window when reviewed peer state changes.

The A-MPDU terminal path now separates classification/detach from retained
backing release. DMA clears only the descriptor prefix published by the
terminal transaction. Returning a complete BA16 batch creates one custom
readiness wake rather than sixteen equivalent wakes.

Aggregate admission performs one reviewed peer lookup and then carries a
generational portable binding plus an AID-derived hardware-key binding.
Per-MPDU encoding validates those bindings in O(1); peer removal,
authentication changes and slot reuse invalidate stale bindings.

Prepared publication is a synchronous ownership operation throughout the
role/DATAPATH traits; the former immediate `async` wrappers were deleted. A
complete standby batch uses a direct post-control publication path without
re-entering generic burst/deadline discovery.

Only the measured 584-byte
`Esp32s31ApEngine::encode_aggregate_ethernet_in_place` leaf was placed in
`.hot.text.open_radio_ap_tx_encode`. An attempted hot prepared-continuation
predicate occupied another 1,252 bytes without reducing latency and was
removed in the same cutover. The complete async scheduler remains in PSRAM.

The production HT40/BA16 PPDU publication transaction is now bound to an
exact, ordered 36-event vendor MMIO slice. Blobray compares the reviewed
queue-zero, two-MPDU MCS7/HT40/short-GI case against the compiled production
`RadioRegisters::program_ht_mac_tx_ppdu` implementation. This is a bounded
production-trace result, not a whole-function equivalence claim.

## Diagnostic result

The initial diagnostic task-poll image, before the compiled-production PPDU
binding, had:

- runtime CRC-32 `10519253`;
- application SHA-256
  `2746e1f568b4f1e0735e35215a44242faa50af2b596f26d0b781c1f161183690`;
- placement, stack-frame, autonomous-source-graph and serialized-log-writer
  audits all `PASS`;
- `.hot.text` size 39,856 bytes, including the 584-byte AP aggregate encoder.

```console
cargo hil --lab-config hil/local.toml run \
  access-point-single-client-ceiling-tx-task-poll
```

| Cycle | Payload | BA16 preparation | Completion core | Backing release | Terminal to publication | Publication program |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 109.574 Mbit/s | 1,954,531 / 9,305 = 210.05 us | 17.80 us | 48.66 us | 497,563 / 9,304 = 53.48 us | 36.08 us |
| 2 | 108.924 Mbit/s | 1,879,198 / 9,251 = 203.13 us | 17.69 us | 48.62 us | 55.91 us, excluding one explicit 9.457021-s inter-cycle idle sample | 36.54 us |

The path split showed approximately 51--54 us from terminal service boundary
to prepared scheduler entry and about 2.2 us from that entry to MMIO
publication. The mean remains close to, but does not strictly satisfy, a
less-than-50-us terminal-to-publication research target. Moving the complete
async scheduler into SRAM was rejected: the measured unit is too large and
the remaining gap does not justify broad internal-memory residency.

Both cycles retained MCS7/HT40/short-GI, acknowledged every prepared subframe,
and reported zero individual retry, timeout, collision, FIFO overflow or
`BUFFER_FULL`. Almost every continuing aggregate contained exactly sixteen
MPDUs; one terminal partial batch at a workload boundary is expected and does
not change the negotiated BA16 target.

## Observer-free performance result

The final performance image had:

- runtime CRC-32 `d9369d09`;
- application SHA-256
  `401b793d1441216b507fb31ad5fbf4bda37cb44465862a881a2781a5bce679ea`;
- placement, stack-frame, autonomous-source-graph and serialized-log-writer
  audits all `PASS`.

```console
cargo hil --lab-config hil/local.toml run \
  access-point-single-client-ceiling-tx
```

The required three attempts all passed. Their six cycles were:

| Attempt / cycle | Payload throughput | OpenWrt active RX throughput | PHY | Retry / failed | AQM drops |
| --- | ---: | ---: | --- | ---: | ---: |
| 1 / 1 | 113.107 Mbit/s | 139.898 Mbit/s | MCS7, 40 MHz, SGI | 0 / 0 | 0 |
| 1 / 2 | 114.231 Mbit/s | 139.979 Mbit/s | MCS7, 40 MHz, SGI | 1 / 1 | 0 |
| 2 / 1 | 113.489 Mbit/s | 139.918 Mbit/s | MCS7, 40 MHz, SGI | 0 / 0 | 0 |
| 2 / 2 | 114.589 Mbit/s | 139.359 Mbit/s | MCS7, 40 MHz, SGI | 0 / 0 | 0 |
| 3 / 1 | 115.262 Mbit/s | 140.472 Mbit/s | MCS7, 40 MHz, SGI | 0 / 0 | 0 |
| 3 / 2 | 115.162 Mbit/s | 140.128 Mbit/s | MCS7, 40 MHz, SGI | 0 / 0 | 0 |

The previous observer-free baseline in the same laboratory cell was
96.727--99.344 Mbit/s. The cutover therefore recovered about 14--18 Mbit/s
without increasing BA depth or enabling A-MSDU.

One OpenWrt retry and failed frame occurred in one of six cycles. It did not
cause rate fallback, AQM loss or scenario failure, but it means a strict
zero-retry/zero-failure research criterion is not claimed by this record.

An independent repeat after the complete workspace and source-only gates used
the same performance image. All three required attempts passed; their six
payload results were 109.874, 111.604, 113.481, 114.196, 112.504 and
113.650 Mbit/s. Every cycle retained MCS7/HT40/short-GI and zero AQM drops.
The final cycle again contained exactly one OpenWrt retry and one failed frame.
The rare event is therefore reproducible in two independent six-cycle runs and
remains an explicit investigation item rather than being dismissed as an RF
outlier.

### Final source-bound repeat

After adding the compiled-production Blobray binding, the current performance
image had:

- runtime CRC-32 `b26f213b`;
- application SHA-256
  `fb659a56ce8eb543b9be80b6db9f14e145deaeb8c55373dc56d016aea50e3f8c`;
- placement, stack-frame, autonomous-source-graph and serialized-log-writer
  audits all `PASS`.

All three required attempts passed. Their six cycles were:

| Attempt / cycle | Payload throughput | OpenWrt active RX throughput | PHY | Retry / failed | AQM drops |
| --- | ---: | ---: | --- | ---: | ---: |
| 1 / 1 | 113.319 Mbit/s | 138.872 Mbit/s | MCS7, 40 MHz, SGI | 0 / 0 | 0 |
| 1 / 2 | 111.862 Mbit/s | 139.404 Mbit/s | MCS7, 40 MHz, SGI | 0 / 0 | 0 |
| 2 / 1 | 111.958 Mbit/s | 137.918 Mbit/s | MCS7, 40 MHz, SGI | 0 / 0 | 0 |
| 2 / 2 | 114.004 Mbit/s | 139.960 Mbit/s | MCS7, 40 MHz, SGI | 0 / 0 | 0 |
| 3 / 1 | 113.491 Mbit/s | 139.912 Mbit/s | MCS7, 40 MHz, SGI | 0 / 0 | 0 |
| 3 / 2 | 111.645 Mbit/s | 138.440 Mbit/s | MCS7, 40 MHz, SGI | 0 / 0 | 0 |

The median payload throughput was 112.639 Mbit/s. Every cycle retained
MCS7/HT40/short-GI and reported zero OpenWrt retry, failed frames and AQM
drops. The performance image is observer-free and therefore cannot itself
establish terminal-to-publication timing. That criterion is measured
separately below.

### Final source-bound diagnostic repeat

The current diagnostic task-poll image had:

- runtime CRC-32 `b4f01fa7`;
- application SHA-256
  `7ea871eca49304e1f3e6fc05099a5b7c6a6e52154c502a0bb970989c5f8b1cd7`;
- placement, stack-frame, autonomous-source-graph and serialized-log-writer
  audits all `PASS`.

The HIL observer now timestamps terminal completion after its own counter
bookkeeping. Its atomics are therefore not charged to the production
scheduler boundary being measured. The two cycles passed at 107.300 and
106.636 Mbit/s under the diagnostic overhead. They retained MCS7/HT40/SGI and
reported zero retry, failed frames, AQM drops, timeout, collision, FIFO
overflow and `BUFFER_FULL`.

| Cycle | Completion to publication | Completion core | Backing release | Publication program |
| ---: | ---: | ---: | ---: | ---: |
| 1 | 515,601 / 9,114 = 56.57 us | 156,009 / 9,114 = 17.12 us | 442,843 / 9,114 = 48.59 us | 340,563 / 9,152 = 37.21 us |
| 2 | 550,394 / 9,055 = 60.78 us, excluding one explicit 9.373528-s inter-cycle idle sample | 160,213 / 9,056 = 17.69 us | 440,843 / 9,056 = 48.68 us | 336,523 / 9,096 = 36.99 us |

The prepared-entry-to-publication part remained only 2.37--2.46 us. The
remaining 54.20--58.32 us is between the terminal service boundary and the
prepared scheduler entry. A previous experiment moving a 1,252-byte
continuation predicate to SRAM did not reduce it and was removed; broad
scheduler placement in SRAM is not justified by this result. The strict
less-than-50-us research target is therefore explicitly not closed.

## Remaining boundary

The scheduler/DMA/peer-binding defects identified by the ceiling analysis are
closed. The current PPDU programming sequence is covered by an exact bounded
Blobray comparison against compiled production code. Further work toward
stable 120+ Mbit/s at BA16 and without A-MSDU is a separate
medium-access/CPU-budget investigation. Any proposed register-write reduction
or extension outside the reviewed finite domain still requires new vendor
evidence and a matching compiled-production comparison.
