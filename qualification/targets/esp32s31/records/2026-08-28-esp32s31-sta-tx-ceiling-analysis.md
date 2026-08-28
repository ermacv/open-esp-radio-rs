# ESP32-S31 station TX ceiling analysis

Date: 2026-08-28

This record separates station TX CPU cost, radio delivery and laboratory
state. It does not infer a bottleneck from throughput alone. Every ceiling run
below required a managed OpenWrt fixture, 40 MHz HT operation, AP receive rate
`150.0 MBit/s MCS 7 40MHz short GI`, and pre-workload channel utilization no
higher than 64/255.

## Measurement boundary

Three distinct image classes were used:

- `performance` contains no driver or scheduler observer and is the production
  throughput result;
- `diagnostic-task-poll` keeps the production split-core topology and adds
  A-MPDU/BlockAck plus executor-poll residence;
- `diagnostic-core0-rx-coarse` was reused for a 12-second TX interval because
  its observer measures the complete Core0 radio future with `mcycle` and
  `minstret`, independently of RX work. Twelve seconds keeps the u32 cycle
  delta below one wrap.

The TX host runner now records the pre-workload airtime sample, AP link vector,
task residence, and Core0 cycles/instructions in its immutable report. The
same guard is also applied to the observer-free production ceiling.

## Baseline before the hot-path fix

The observer-free run `1787931120316-00022369` produced 116.016, 116.477 and
116.170 Mbit/s (mean 116.221 Mbit/s), with zero missing, reordered or duplicate
host datagrams. Channel utilization was below the 64/255 gate and AP RX stayed
at HT40 MCS7 SGI.

The three-repeat task-poll run `1787930927615-00021ee1`, runtime CRC
`6f8a17d1`, produced 115.424--115.874 Mbit/s. It averaged approximately:

| Measurement | Baseline |
|---|---:|
| A-MPDU subframes | 30.10 |
| aggregate preparation | 820.75 us/aggregate |
| Core1 network residence | 97.21 us/datagram |
| Core0 radio residence | 92.94 us/datagram |

All but one received BlockAck were full across the three repetitions. There
were no target hardware timeout/collision outcomes and no host delivery loss.

The coarse run `1787931839601-000249f5`, runtime CRC `ae17cf62`, measured
117.619 Mbit/s and established the actual Core0 boundary:

| Measurement | Baseline |
|---|---:|
| Core0 radio cycles | 2,913,605,855 |
| Core0 radio retired instructions | 588,156,226 |
| Core0 cycle occupancy | 75.87% |
| IPC | 0.202 |
| cycles/datagram | 24,291.79 |
| instructions/datagram | 4,903.67 |
| Core1 network residence | 93.59 us/datagram |
| Core1 network + UDP task residence | 96.38% of interval |

Thus Core0 was not 95--100% compute-saturated. Core1 was much closer to
saturation, while the low Core0 IPC showed a stall-heavy path without by
itself identifying cache as the cause.

## Localized defect

`PinnedDmaTxPool::claim_network()` performed a whole-pool DMA guard scan before
every TX token. The production Wi-Fi geometry has 66 slots and a 32-byte guard
per slot, so every Ethernet frame read and compared 2,112 unrelated guard
bytes before writing its selected slot. At about 120 thousand datagrams per
12-second interval this was an O(queue depth) diagnostic audit in the Core1
packet hot path.

The scan was not an ownership requirement. The selected slot is already
checked at `FREE -> NETWORK`, and the exact hardware-owned slot is checked
again before `RADIO -> FREE`. The whole-pool check was moved to the explicit
`claimed_slots()` lifecycle/diagnostic observation. Per-slot transitions and
DMA-overrun failure remain fail closed.

## Result after the fix

The same coarse image class in run `1787932018630-000251e2`, runtime CRC
`4ebb8cdf`, measured:

| Measurement | Before | After | Change |
|---|---:|---:|---:|
| host TX | 117.619 Mbit/s | 120.638 Mbit/s | +2.57% |
| Core1 network | 93.59 us/datagram | 69.73 us/datagram | -25.50% |
| Core0 cycle occupancy | 75.87% | 69.44% | -6.43 points |
| Core0 cycles/datagram | 24,291.79 | 21,672.62 | -10.78% |
| Core0 instructions/datagram | 4,903.67 | 4,252.43 | -13.28% |
| Core0 radio polls/datagram | 0.516 | 0.443 | -14.1% |

The after-fix task-poll run `1787932184459-000257bd`, runtime CRC `e5b54a48`,
produced 118.885--119.036 Mbit/s across three repetitions with zero delivery
loss, reordering or duplication. A-MPDU size became a stable 31.50 frames,
aggregate preparation fell to approximately 697.40 us, every received
BlockAck was full, and there were no hardware timeout/collision outcomes.

The final observer-free production run `1787932467261-0002620b`, runtime CRC
`d3c6dd0d`, produced 120.406, 119.676 and 119.937 Mbit/s (mean 120.006
Mbit/s). Every repetition had zero missing/reordered/duplicate host datagrams,
AP RX stayed at HT40 MCS7 SGI, and pre-workload utilization was 13--16/255.
The production ceiling gate is consequently raised from 100 to 115 Mbit/s.

## Current blocker and next work

At the after-fix coarse point, Core1 network plus UDP task residence is about
74.4% and Core0 radio cycle occupancy is 69.4%. Neither core is the current
hard ceiling. In the driver-observed image, a 31.5-frame aggregate with 1,472
useful payload bytes and an approximately 3.002 ms exchange corresponds to an
empirical in-exchange payload rate near 123.6 Mbit/s. The 120.0 Mbit/s
production mean is about 97% of that bound. The remaining few Mbit/s are
therefore primarily the aggregate-to-aggregate air/publication gap at the
current HT40 MCS7 SGI geometry, not evidence of a CPU wall.

Further CPU reduction is still useful for duplex and concurrent roles, but it
must be measured independently from the ceiling:

1. add TX-specific Core0 cycle/instruction bins around frame preparation,
   CCMP/header encode, descriptor publication, terminal IRQ service and
   scheduler tail;
2. measure Core1 `embassy-net` egress phases separately (socket traversal,
   IPv4/UDP checksum, TX-token claim/publication) before changing checksum or
   copy policy;
3. retain per-slot DMA guard checks and keep whole-pool audits outside packet
   admission;
4. use same-policy HIL A/B and require lower cycles/datagram in addition to
   throughput, BA health and the idle-channel/link gates.

No SRAM relocation, manual cache coloring, replacement-buffer architecture or
IRQ polling change is justified by this TX result.
