# ESP32-S31 HT40 datapath regression

Qualification ID: `HIL_ESP32S31_HT40_DATAPATH_2026_08_13`.

Board: ESP32-S31 revision 0.0. Peer: laboratory ASUS RT-AX52 running
OpenWrt, 2.4 GHz channel 6, HT40. Base: `54a84e33`, plus the implementation
and record in the same change set.

## Cause and correction

The refactor introduced two independent RX regressions:

- staged SRAM frames were copied into a second network pool instead of lending
  their linear staging lease to the network token;
- split placement put Wi-Fi protocol/reorder and `embassy-net` together on
  CPU1, while CPU0 owned only radio/IRQ service.

The corrected path lends the staging lease through the network `RxToken`; token
release recycles the exact SRAM slot and wakes RX capacity. CPU0 owns
radio/IRQ plus Wi-Fi protocol/reorder. CPU1, when selected, owns
`embassy-net` plus the application. No direction-dependent runtime policy is
present.

## Qualification

All cells used reset isolation and the production qualification image:

- RX-only: 3/3 PASS at `108.752..116.172 Mbit/s`; no software, DMA
  `BUFFER_FULL`, or FIFO-overflow drops.
- TX-only: 3/3 PASS at `114.184..115.892 Mbit/s`; no missing, reordered, or
  duplicate host datagrams; A-MPDU maximum 32.
- split-core 40+40 Mbit/s: 5/5 PASS at `79.856..79.959 Mbit/s` combined;
  no software/DMA drops or beacon loss.
- single-core 32+32 Mbit/s: 5/5 PASS at `63.920..63.969 Mbit/s` combined;
  no software/DMA drops or beacon loss.
- station lifecycle: 10 cold boots with three complete reconnects each, PASS.
  The host gate requires the ordered lifecycle edges
  `Disconnected(N, ReconnectRequested) -> Connected(N+1)` as well as complete
  owner-return evidence. The earlier completion-only gate could report PASS
  before the new connected generation became observable and is not retained
  as evidence.

One earlier reset produced beacon loss immediately after the first association
and before network readiness; the reconnect succeeded. A complete five-reset
repeat did not reproduce it. It is a startup lifecycle observation, not a
datapath-load failure, and remains fail-closed in scenarios that require no
beacon loss.

## Single-core boundary

Diagnostic instrumentation showed why 32+32 is the maintained single-core
baseline. At 32+32 the network poll averaged `882 us`, while TX IRQ-to-service
averaged about `1.93..2.08 ms`. At 36+36 the measured result was
`35.956+35.895 Mbit/s`, but TX-credit-blocked polls rose to 1275 and sampled
IRQ-to-service to about `3.78 ms`. At 40+40 the result became asymmetric
(`39.975+35.711 Mbit/s`), 5854/7400 polls were TX-credit blocked, and sampled
IRQ-to-service reached `7.27 ms`.

The radio still used 32-member A-MPDUs and reported no DMA/FIFO loss. The
single-core limit is cooperative executor residence: a work-conserving network
poll can publish a complete 32-frame aggregate before the radio task services
the pending TX interrupt. Lowering the A-MPDU limit or adding workload-specific
yields would trade radio efficiency for a benchmark and is not accepted.
Split-core placement is therefore the production high-throughput topology;
single-core remains the simpler bounded-latency topology.

## TCP and latency

The TCP socket capacities are 256 KiB RX and 128 KiB TX. The original 64 KiB
TX capacity exceeded the link BDP but retained only about 44 full-size TCP
segments, fewer than the active plus standby 32-frame A-MPDU arenas. Raising
it to 128 KiB removed that artificial producer boundary; 256 KiB improved
throughput by only about one percent and was rejected as a poor PSRAM trade.
A HIL-only byte-at-a-time stream pattern generator and validator was replaced
by block operations over its exact 256-byte mathematical period. This
preserves absolute-offset byte equality while removing benchmark work
unrelated to the driver.

- TCP RX reached `79.741..79.774 Mbit/s` at an 80 Mbit/s offer. Two measured
  transfers passed; the third reset was rejected because the initial connected
  generation reported beacon loss before network readiness.
- TCP TX with a 120 Mbit/s offer: 3/3 PASS at `70.626..70.807 Mbit/s`, exact
  byte count and pattern. This is up from `54.878..55.604 Mbit/s` with the
  64-KiB producer buffer. Full 32-member aggregates rose from single digits to
  about 510 per 16-second run.
- TCP 30+30: 3/3 PASS at `59.988..59.994 Mbit/s` combined, exact in both
  directions.
- ICMP: 3/3 PASS, 100/100 replies. Median RTT was `2.573..2.831 ms`; p95 was
  `4.506..8.546 ms`; the largest observed sample was `41.292 ms`.

TCP TX is no longer socket-capacity limited at 128 KiB. It publishes full-size
MPDUs with zero DMA/FIFO drops and immediate TX interrupt service, while the
host advertises roughly 1.15 MiB of receive window and ACKs about every two
segments. The residual roughly 71 Mbit/s ceiling remains in TCP/network
scheduling rather than the Wi-Fi driver or its 32-member A-MPDU contract.

## Cold-start beacon observation

Qualification images now record a finite associated-RX policy snapshot at
connected start and exit. Across 10 cold boots and 30 genuine reassociations,
all 40 starts had the same policy: queue zero `0x0001c387`, queue three
`0x0001c38f`, the selected BSSID, address checking and RX policy enabled, STA
role selected, and hardware beacon-filter control zero. No beacon loss or
diagnostic text loss occurred in that series. This excludes a persistent RX
filter programming difference between initial and reconnected epochs, but
does not yet explain the earlier rare cold-start event; the no-beacon-loss
gate remains fail-closed.

Artifact SHA-256 values (first repetition):

```text
RX report          6ef3995332c2bdf8760dcc60eecefd755630aad91172ec1477fd2ec5288c726c
TX report          74b56e84019de1770f68b82a89aaab32ae74b0798687a436be871165d6341242
TCP TX report      868f6a28766ef2161ee887d84aaf97879ac5eff2170d9e484fcc3dfd7010ecdf
split bidi report  f1749f6998f0c3c7092987df3f9dbf696eef803179b8f5850c6ff2cb567f66c0
single bidi report e4d9f47f9f56714316ffe2075bdc992675966ead2c436530c397fb5e77b635a5
```
