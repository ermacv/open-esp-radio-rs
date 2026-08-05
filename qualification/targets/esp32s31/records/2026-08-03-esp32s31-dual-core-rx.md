# ESP32-S31 dual-core RX scheduling qualification

Qualification ID: `HIL_OPEN_HE20_DUAL_CORE_RX_2026_08_03`.

## Scope

This qualification removes the rare 80-Mbit/s RX descriptor starvation seen
when every connected task shared one cooperative thread-mode Embassy executor.
It does not impose a per-wake frame limit. The DMA owner still drains the
complete contiguous hardware frontier that can be admitted by the staging
pool and queue credits.

The connected task affinity is:

- Core 0: MAC and power interrupts, PAC/MMIO, RX/TX DMA radio runner,
  `embassy-net`, network report and HIL benchmark;
- Core 1: staged 802.11 protocol processing, BlockAck reorder, decapsulation
  and publication of Ethernet frames to the pinned network queue.

The compiler enforces the boundary. `embassy_net::Runner` contains a
`RefCell` and is not `Send`, so it remains on Core 0. The staged protocol task
owns only cross-core `CriticalSectionRawMutex` queues and passes
`SendSpawner::spawn` without an unsafe trait implementation. MMIO ownership
never crosses cores.

Core 1 uses the existing scheduler-free runtime executor with software
interrupt 1 for remote wake-up. Its 16-KiB stack is explicitly placed in
internal `.critical.bss`; the futures and task storage remain static. In the
poll-profile ELF, `.critical.bss` is 125,760 bytes, `.dma.data` plus
`.dma.bss` is 110,916 bytes, and the remaining linker-owned Core 0 stack range
is 253,248 bytes. Placement and autonomous-source audits pass.

## Failure localization before the split

The ordinary single-core 80-Mbit/s, 1,472-byte UDP RX run delivered every host
datagram but recorded five hardware `BUFFER_FULL` increments. The increments
were observed in two RX services with a full 32/32 descriptor frontier while
the staging pool and queue both still had 64 credits. Sampled IRQ-to-service
latency reached 11,402 us. No A-MSDU or oversized raw unit was observed.

The single-core poll overlay localized the cooperative residence:

| Task | Average continuous poll residence | Boot maximum |
|---|---:|---:|
| `embassy-net` | 1,923 us | 2,451 us |
| staged RX protocol | 2,025 us | 2,743 us |
| radio runner | 328 us | 639 us |

One-frame network polling and explicit protocol yields reduced hardware
latency but caused staging backpressure and reduced delivered payload to
approximately 52 and 63 Mbit/s respectively. Both experiments were rejected.

## Ordinary-image qualification

All runs used HE20, an 80-Mbit/s paced host offer and 1,472-byte UDP payloads.
They were independent benchmark sessions on the same ordinary image.

| Duration | Device payload | UDP datagrams | Software drops | `BUFFER_FULL` / FIFO overflow | Maximum frontier | Sampled IRQ-to-service average / maximum |
|---:|---:|---:|---:|---:|---:|---:|
| 20 s | 80.001 Mbit/s | 135,871 | 0 | 0 / 0 | 14 | 81.55 / 866 us |
| 30 s | 79.963 Mbit/s | 203,806 | 0 | 0 / 0 | 3 | 85.63 / 223 us |
| 30 s | 80.001 Mbit/s | 203,806 | 0 | 0 / 0 | 13 | 83.89 / 1,242 us |
| 30 s, final cold boot | 79.999 Mbit/s | 203,806 | 0 | 0 / 0 | 13 | 83.93 / 748 us |

Every run reported zero UDP sequence gap, staging backpressure, pool-credit
limit and queue-credit limit. The aggregate qualified interval is 110 seconds
and 747,289 delivered UDP datagrams without a hardware buffer-full increment.

## Poll-overlay qualification

The diagnostic overlay adds timer reads and atomics and is not used to define
the production throughput ceiling. A reset-separated 20-second run still
delivered all 135,871 datagrams at 80.002 Mbit/s with zero hardware or software
drop, a maximum frontier of 12 and sampled IRQ-to-service latency of 91.96 us
average / 974 us maximum.

| Task | Average continuous poll residence | Boot maximum |
|---|---:|---:|
| `embassy-net`, Core 0 | 94.94 us | 1,693 us |
| staged RX protocol, Core 1 | 117.87 us | 1,273 us |
| radio runner, Core 0 | 38.44 us | 236 us |

One preceding diagnostic cold boot associated and completed WPA2 but reported
beacon loss before DHCP; the reset-separated retry above passed. This isolated
boot is retained as an association-stability observation, not hidden as a
throughput result. Repeated cold-boot coverage remains appropriate for the
permanent HIL matrix.

## Result

The split is retained. It removes the sequential cooperative latency that
caused descriptor starvation without adding an artificial 8/16-frame ceiling
or moving non-`Send`/MMIO state across cores. UDP RX at 80 Mbit/s is qualified
for this cell. TCP and RX/TX/bidirectional protocol matrices remain separate
follow-up qualifications.
