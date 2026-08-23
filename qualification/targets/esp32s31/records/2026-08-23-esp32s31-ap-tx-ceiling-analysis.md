# ESP32-S31 AP TX ceiling analysis

Date: 2026-08-23

Evidence ID: `HIL_ESP32S31_AP_TX_CEILING_ANALYSIS_2026_08_23`

This is a diagnostic research record, not a qualification claim. The driver
base is `feda7cb6cd56`; the HIL-only link and timing observations described
below are part of the same uncommitted analysis change set. No production
scheduler, retry, aggregation, or register-publication behavior was changed.

## Question

The current AP must use the PSRAM code/data/task-stack profile and retain a
sixteen-MPDU TX BlockAck window. A-MSDU is outside the current scope. The goal
was to identify why observer-free AP UDP TX remains near 95--102 Mbit/s at an
HT40 MCS7 PHY rate of 150 Mbit/s, and what separates that result from stable
120+ Mbit/s payload throughput.

Historical records establish a different aggregation contract:

- `HIL_ESP32S31_HT40_DATAPATH_2026_08_13` measured STA RX at
  108.752--116.172 Mbit/s and STA TX at 114.184--115.892 Mbit/s with a maximum
  A-MPDU size of 32;
- `HIL_ESP32S31_AP_PSRAM_STACK_LOAD_2026_08_17` measured AP TX at
  109.989--115.161 Mbit/s, also before the AP window was reduced to 16;
- `caa0ac09` deliberately changed `AP_TX_BLOCK_ACK_WINDOW` from 32 to 16 so a
  saturated downlink could not monopolize the medium against concurrent
  uplink traffic.

The earlier observation that TX could exceed RX is therefore real, but it is
not evidence of a separate faster TX implementation. It was measured with
twice the current aggregate depth.

## Observer-free RF evidence

The production `performance` image was exercised with the OpenWrt router as
the sole wireless AP client. The wired host remained only the traffic
generator. The host runner now snapshots both TX and RX sides of OpenWrt
`iw station dump`, including RX bytes, packets, duration, and current bitrate.

```console
cargo hil --lab-config hil/local.toml run access-point-single-client-ceiling-tx
```

Representative complete cycles were:

| Payload throughput | OpenWrt RX duration | OpenWrt RX bitrate | Retries / failed |
| ---: | ---: | --- | ---: |
| 101.044 Mbit/s | 12.249165 s | 150.0 Mbit/s, MCS7, 40 MHz, short GI | 0 / 0 |
| 98.552 Mbit/s | 12.074176 s | 150.0 Mbit/s, MCS7, 40 MHz, short GI | 0 / 0 |
| 99.514 Mbit/s | 11.933308 s | 150.0 Mbit/s, MCS7, 40 MHz, short GI | 0 / 0 |
| 99.369 Mbit/s | 12.107795 s | 150.0 Mbit/s, MCS7, 40 MHz, short GI | 0 / 0 |

One additional cycle delivered 94.593 Mbit/s and failed the existing
95-Mbit/s floor. It also reported MCS7/HT40/short-GI with zero OpenWrt retry or
failure. The peer receives the intended PHY vector cleanly, but records only
about 75% RX airtime during each 16-second traffic interval. The missing
payload throughput is inter-exchange idle/turnaround time, not rate fallback,
FCS loss, or retry traffic.

## Diagnostic phase measurements

The two AP ceiling diagnostic scenarios incorrectly selected the single-core
topology. They now use the same split radio/network topology as the production
ceiling cell. The corrected MAC-IRQ diagnostic delivered 95.091--96.971
Mbit/s. Its two cycles observed:

- 8,076--8,235 completed aggregates and 129,212--131,758 acknowledged
  subframes;
- all but one terminal aggregate contained 16 MPDUs;
- 0 timeout, collision, hardware failure, or missing terminal subframe;
- standby prepared and published for every continuing exchange;
- publication programming averaged 44.6--44.8 us;
- sampled IRQ-to-service latency averaged about 32.9 us;
- one-publication exchange time averaged about 1.76--1.79 ms;
- no batch-collection deadline: the prepared count and preferred count were
  both 16.

A diagnostic-only terminal-completion correlation was then added. It records
the interval after `release_completed` and the terminal AP observation, but
before the next `publish_standby` begins. The task-poll image
(`runtime_crc32=3432dcbc`) measured:

| Cycle | Payload throughput | Completion to next publication | Publication programming |
| --- | ---: | ---: | ---: |
| 1 | 95.916 Mbit/s | 1,583,065 us / 8,145 = 194.36 us mean | 44.38 us mean |
| 2 | 99.113 Mbit/s | 1,620,056 us / 8,416 = 192.50 us mean | 43.51 us mean |

The second interval excludes one explicit 9.400413-second inter-cycle idle
sample retained by the lifetime correlation. This exclusion is stated rather
than silently normalized. The observer also showed that nearly every active
radio poll takes 0.5--1.0 ms. The wrapper measures one actual `Future::poll`,
not time spent pending. Much of that residence prepares the following standby
while the current PPDU is on air and is therefore overlapped; the non-overlapped
terminal-to-publication interval above is the relevant throughput loss.

## Source-level cause

The common DATAPATH scheduler unconditionally calls `service_control()` at
the top of every transaction boundary. For standalone AP this performs the
following work after every A-MPDU:

- beacon due check;
- TX BlockAck expiry;
- WPA2 retry/close checks;
- peer-close check;
- nearest-deadline traversal;
- a second nearest-deadline traversal in the AP DATAPATH adapter.

The current wait contract stores only a relative
`Timer::after_millis(next_control_delay_millis)`. Reconstructing that future
after every network wake moves the timer forward, so the scheduler cannot
simply omit the unconditional service call without first changing the
contract to an absolute deadline. About 8,200 full data aggregates and only
about 212 transmitted beacons occur in one 16-second diagnostic epoch. The AP
control plane is therefore polled orders of magnitude more often than its
timer work becomes due.

STA control already owns absolute alarm deadlines and an independently
published command channel. The AP custom adapter is the remaining relative
timer/polling exception at this boundary.

## Vendor comparison

Focused Blobray inspection retains complete instruction bodies but reports
semantic blockers for unresolved vendor context fields and input-dependent
branches in this slice. The facts that are currently recoverable are:

- `ppProcessTxQ` checks `lmacIsIdle` before selecting and publishing the next
  frame exchange;
- `lmacSetTxFrame` calls `hal_mac_tx_config_timeout`,
  `hal_mac_tx_set_ppdu`, TXOP-queue request/release, and lifetime processing;
- `hal_mac_tx_set_ppdu` calls the PLCP0, PLCP1, length, TXOP-queue, and PTI
  producers on each invocation;
- completion is posted asynchronously to the PP task, which recycles the
  completed exchange before the next idle selection.

These observations do not prove an AP-specific multi-PPDU hardware burst, and
they do not justify deleting the per-PPDU register sequence. The exact
context-dependent `hal_mac_tx_set_ppdu` effect remains `PARTIAL` in Blobray;
stable-field caching must therefore remain blocked until a reviewed concrete
profile proves which writes can be omitted.

## Throughput budget

Sixteen 1,472-byte UDP payloads contain 188,416 useful bits. Required
aggregate start-to-start periods are:

| Payload target | Maximum period |
| ---: | ---: |
| 100 Mbit/s | 1,884.16 us |
| 110 Mbit/s | 1,712.87 us |
| 115 Mbit/s | 1,638.40 us |
| 120 Mbit/s | 1,570.13 us |

The current observer-free 99--101-Mbit/s path spends approximately 1.86--1.90
ms per full aggregate. The directly measured post-completion scheduler gap is
about 0.193 ms and publication programming about 0.044 ms. Removing the
unconditional control polling can plausibly recover roughly 8--11 Mbit/s, but
does not by itself establish a stable 120-Mbit/s path.

OpenWrt records roughly 1.40--1.43 ms of received PPDU airtime per full
aggregate. At 120 Mbit/s only about 0.14--0.17 ms remains for SIFS, BlockAck,
AIFS, random backoff, software completion, and the next publication. This is
at the physical edge of a BA16/no-A-MSDU contract. Stable 120+ Mbit/s cannot
be treated as a scheduler-only acceptance criterion under that contract.

## Required cutover order

1. Replace AP relative control delay with one absolute control deadline.
   The DATAPATH scheduler must latch control readiness and call
   `service_control` only at initial entry, an explicit control wake, after an
   RX turn that may publish control work, or after a control-owned TX terminal
   edge. A network-data TX completion must not force a full AP control scan.
   Delete the unconditional polling path in the same change.
2. Add scheduler regressions for saturated data with a future beacon deadline,
   a due beacon that wins before another data publication, RX-generated
   control TX, external station control, stop, and paired STA+AP ownership.
3. Repeat observer-free AP TX, AP RX+TX, and beacon-timing HIL. The immediate
   BA16 target is a stable 108--112 Mbit/s TX path with no throughput or beacon
   fairness regression.
4. If terminal-to-publication remains above 50 us, measure completion/release,
   control probe, owner accounting, and publication as separate phases. Move
   only the proven hot synchronous TX leaf call graph into `.hot.text`; do not
   move the whole async radio future into SRAM.
5. Only after a concrete Blobray profile makes `hal_mac_tx_set_ppdu` complete,
   compare its ordered MMIO effects with compiled production publication.
   Remove or cache stable writes only when that comparison proves it.
6. Treat stable 120+ Mbit/s as a separate aggregation/medium-access design
   decision. The available choices are a larger BA window, A-MSDU, or a
   reviewed standards-valid multi-PPDU TXOP mechanism. A larger window was
   previously measured; A-MSDU is explicitly out of scope; Blobray currently
   provides no evidence for the TXOP alternative.

This order fixes the demonstrated polling defect first without changing the
agreed BA16 or A-MSDU scope and without hiding the physical throughput budget.
