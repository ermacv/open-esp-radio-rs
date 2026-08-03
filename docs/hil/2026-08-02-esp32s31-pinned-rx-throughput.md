# ESP32-S31 pinned-RX throughput qualification

Qualification ID: `HIL_OPEN_HE20_PINNED_RX_2026_08_02`.

## Change under test

The former high-throughput network boundary queued
`EthernetFrame<1600>` by value. Every publication initialized and filled a
complete local frame and then moved the large value into the Embassy channel.
The measured `copy+publish` phase was the largest controllable RX phase.

`PinnedResources` now owns final RX slots plus bounded free/ready index
channels. The protocol publisher reserves one slot, copies the Ethernet header
and payload directly into it, and publishes only its `u8` index. A unique
`PinnedReceiveToken` returns the slot after `embassy-net` consumes or drops it.
The Wi-Fi DMA buffer still does not escape the radio owner: DMA-to-staging copy
and descriptor recycle retain the recovered vendor ownership order.

The RX resources remain ordinary CPU-owned PSRAM. Only the separate TX pool
is placed in DMA-visible internal SRAM. The HIL placement and autonomous source
audits both passed.

## Exact cell

- Board: ESP32-S31 revision 0.
- Peer: FRITZ!Box 7530 on the ordinary LAN.
- Memory profile: `psram-code-psram-data`.
- PHY: HE20 SU, NSS1, MCS9, 2xLTF/0.8-us GI, nominal 114.7 Mbit/s.
- RX descriptor ring and vendor-equivalent large-RX staging pool: 32 entries.
- Host UDP payload: 1,200 bytes.

Firmware was built and flashed with credentials supplied only through the
environment:

```text
OPEN_RADIO_STA_SSID=<ssid> OPEN_RADIO_STA_PASSWORD=<password> \
  cargo hil flash radio --port /dev/ttyACM0
```

The stable RX run was:

```text
cargo hil traffic rx <device-ipv4> \
  --phy he20 --rate 75M --seconds 30 --payload 1200 \
  --serial /dev/ttyACM0
```

## RX result

- Result: `PASS`.
- Host offer/device receive: 75.000/74.999 Mbit/s for 30 seconds.
- Delivered payload: 281,251,200 bytes in 234,376 UDP datagrams; no missing
  terminal count.
- Network publications/software drops: 234,502/0.
- Hardware `BUFFER_FULL`/FIFO overflow: 0/0.
- HE useful-frame histogram: 234,377 frames at MCS9 and zero at every other
  MCS.
- DMA frontier/admitted maximum: 31/31, with no pool or queue credit limit.
- DMA-to-staging service: 16.74 us/frame average.
- Complete protocol dispatch: 24.78 us/frame average.
- Final network `copy+publish`: 12.96 us/frame average.
- Network-ready wait: 2.19 us average.

The earlier by-value boundary took 24.60 us/frame in `copy+publish` at
50 Mbit/s and 26.30 us/frame during a requested 60-Mbit/s saturated run. The
new boundary measured 12.25 us/frame at 50 Mbit/s and 12.27 us/frame at
60 Mbit/s. The former 60-Mbit/s run reached only 56.369 Mbit/s and recorded
17 `BUFFER_FULL` observations; the replacement delivered 60.001 Mbit/s with
zero starvation and all useful frames at MCS9.

Additional reset-separated steps passed cleanly at 50, 60, 70 and 75 Mbit/s.
At an 80-Mbit/s offer the application still received all 83,334 datagrams at
79.980 Mbit/s, but strict qualification rejected three `BUFFER_FULL`
observations when the frozen DMA frontier reached exactly 32/32. At this stage
75 Mbit/s was the demonstrated clean point and 80 Mbit/s the measured
descriptor-burst boundary. The repeated and longer follow-up below tightens
the sustained lossless claim to 70 Mbit/s.

## Frontier and IRQ follow-up

The follow-up instrumentation records the complete service-frontier
distribution and samples one in 64 non-coalesced RX wake epochs. Sampling is
required: reading the cross-core clock and publishing diagnostic atomics for
every IRQ measurably changed the boundary being observed. Small negative
cross-core clock offsets are rejected into a separate counter rather than
being interpreted as modular multi-minute delays.

One 30-second 75-Mbit/s follow-up received all 234,376 host datagrams at
74.985 Mbit/s with zero `BUFFER_FULL`, FIFO overflow or software drop. The
45,099 service calls observed frontiers in these buckets:

```text
0 / 1 / 2-3 / 4-7 / 8-15 / 16-31 / 32+
13 / 3177 / 9375 / 27057 / 4614 / 863 / 0
```

There were 235,299 RX interrupt posts, 45,099 distinct wake epochs and 704
timed samples. Valid sampled IRQ-to-service latency averaged 521.78 us and
reached 4,032 us. The exact service histogram, rather than the sampled timing,
is the qualification evidence for saturation. A later reset-separated repeat
on the exact final binary delivered all datagrams but observed one 32+ frontier
and one `BUFFER_FULL`. Therefore 75 Mbit/s is a marginal clean result, not a
repeatably lossless floor.

The final sustained qualification used 70 Mbit/s for 60 seconds. It delivered
all 437,504 host datagrams at 70.000 Mbit/s, published 437,716 network frames,
and recorded zero hardware starvation, FIFO overflow or software drop. Across
122,487 service calls the maximum frontier was 31 and the complete buckets
were:

```text
0 / 1 / 2-3 / 4-7 / 8-15 / 16-31 / 32+
118 / 15878 / 46083 / 56957 / 3134 / 317 / 0
```

The final sampled timing contained 1,911 valid IRQ-to-service samples and two
cross-core clock-skew rejects; valid latency averaged 343.51 us and reached
3,502 us. This 60-second run establishes 70 Mbit/s as the current sustained
lossless floor.

A reset-separated 80-Mbit/s run delivered all 250,001 datagrams at 80.000
Mbit/s, but strict qualification found one 32+ service frontier and one
`BUFFER_FULL`. This independently confirms that 80 Mbit/s is the current
descriptor-burst boundary, with 75 Mbit/s marginal and 70 Mbit/s the current
sustained lossless floor.

Two outer polling-order experiments were intentionally discarded. Prioritizing
the radio actor reduced RX wake latency but delayed production of network TX
leases and reduced simultaneous TX. Moving the radio between the network stack
and protocol actor prevented the RX benchmark interval from completing under
continuous bidirectional load. The current HIL combines three infinite actors
under one ordered `select5`; the independent-task follow-up below removes that
composition-level coupling without changing the single PAC/DMA owner.

## Independent Embassy-task follow-up

The composition root now spawns the network stack, staged RX protocol, radio
owner, network report and benchmark as five independent Embassy tasks. The
driver's `WifiRunner` remains the sole PAC/DMA/TX owner and retains its internal
RX-before-TX arbitration. Moving the running register owner, RX address table
and scratch ownership into explicit static task resources avoided a second PAC
singleton and made the long-running lifetimes visible in the type graph.

The first cold-run experiments exposed two issues that `select5` had hidden:

- an empty or greater-than-1,700-byte untrusted RX unit terminated the complete
  radio task with `Stage(TooLong)`; the production backend now discards and
  asynchronously recycles that one descriptor while preserving the recovered
  reload ownership order;
- allowing the application socket to run to pending could delay RX service,
  while yielding after nearly every datagram moved loss into smoltcp's UDP
  socket. The final policy yields only when an RX IRQ is pending and a 500-us
  cooperative application budget has elapsed. This is a latency bound, not a
  fixed frame batch, and therefore adapts to frame size and offered rate.

The host harness was also made deterministic. It opens UART before traffic,
uses a small positive/terminal UDP exchange as end-to-end readiness proof,
discovers the actual DHCP address from UART, and only then starts the paced
flow. A new 99% device/host UDP-delivery gate prevents a hardware-clean run
with application-socket loss from passing.

The final reset-separated HE20/MCS9 RX cell offered 70.001 Mbit/s for 60
seconds and delivered exactly 437,504/437,504 host datagrams at 70.000 Mbit/s.
It recorded zero `BUFFER_FULL`, FIFO overflow, software drops, empty discards
or oversize discards. The maximum frontier/admission was 31/31. Across 3,415
timed samples, IRQ-to-service latency averaged 147.04 us; the boot-lifetime
maximum was 3,320 us. The complete service buckets were:

```text
0 / 1 / 2-3 / 4-7 / 8-15 / 16-31 / 32+
2226 / 102885 / 88116 / 24238 / 1019 / 393 / 0
```

The final task topology therefore preserves the previous 70-Mbit/s sustained
lossless floor while substantially reducing average sampled IRQ latency and
removing the composition-level `select5` poll order.

## TX and simultaneous regression

The neighboring modes were rebuilt and flashed after the ownership change.

- TX-only: `PASS`; host/device floors 90.892/91.534 Mbit/s, 77,771 datagrams,
  zero missing or reordered. A-MPDU averaged 30.99 MPDUs; 3,764 aggregates had
  31 members and one had 32. Preparation/publication averaged 303.37/23.64 us.
- Bidirectional: `PASS`; 9.999-Mbit/s RX plus an 80.571-Mbit/s concurrent TX
  floor, for a conservative 90.570-Mbit/s sum. RX and TX had no terminal
  hardware failure. The report now includes both RX phase telemetry and TX
  A-MPDU preparation/publication/exchange timing.

After the independent-task and 500-us handoff changes, reset-separated
regressions also passed:

- TX-only: host/device floors 90.831/91.450 Mbit/s across three complete host
  bursts, with 109,052 datagrams and zero missing or reordered. A-MPDU averaged
  30.99 MPDUs; 2,504 aggregates contained 31 members and one contained 32.
- Bidirectional: 20,823/20,835 host datagrams reached the UDP application at
  10.001 Mbit/s while the concurrent TX floor remained 75.377 Mbit/s, for an
  85.378-Mbit/s conservative sum. Hardware starvation, FIFO overflow,
  software queue drops, TX timeouts and collisions were all zero.

## Artifact identity

- RX UART/report SHA-256:
  `e22f1f8cc6d899bccbaa0e7c3d90ebc740c294d64ff0c81ef4a1f0607ac278ae` /
  `c576a1995eddd6e5b14ec4d2b0655d93a02d3698cf414b0766e45697bf73065b`.
- TX UART/report SHA-256:
  `4925df4140940adc07adda99cf46ee2b55c7178cc2e6f7f127778300614211b4` /
  `58b794e2d5b1a046f9a5bc9fc5501a6a3468a83474d5fe13e2736adc33df9a68`.
- Bidirectional UART/report SHA-256:
  `d6ed63188a903349631435d4e6abaadae100745d335a50d238abfb1516154ba6` /
  `885992a0caeab77a7ade85c55917fb8067be9fdb0187a1a94f72392120b88833`.
- Final 60-second 70-Mbit/s RX UART/report SHA-256:
  `54777aceaab5f63035ef8302f91867f2fb22d960452eb499250f91939df7c2e0` /
  `f371ef97ef33b096654fd6c679980e96b14686bc6a8af328b764fff5aac70104`.
- Independent-task 500-us RX UART/report SHA-256:
  `f4432916e50b8557f50e35151653665346a25165cf7a9931c9c41541141189d2` /
  `a19ee3f0aa8e4249b9c0be5a3694d60388db1762c4e0ef0f72860eb33ff54faa`.
- Independent-task TX UART/report SHA-256:
  `d6f9838468567a9c1b834f308a999135885bc5962c5af366deb390a64304b05a` /
  `54dd3ebbf16d70a93b83f3f7e825d6166f05c14c699bf7dc67f76fef3ea1ed54`.
- Independent-task bidirectional UART/report SHA-256:
  `2a1be003a0360b5431f8a91ccfeadbee6e552c03c485f45001ad501e051e67b9` /
  `61bf59a9ba100221293ef62306ff92da8ce36d8ec24eeed70dac45b8bda4384b`.

Generated UART logs and reports remain under `target/hil/esp32s31/qualification`;
this record preserves their exact identity.

## Remaining boundary

The next performance question is no longer the outer `select5` or Embassy
queue depth. Independent tasks are qualified, and the final RX cell delivered
every host datagram despite 143 transient staging-credit boundaries. The
80-Mbit/s edge still coincides with the complete 32-descriptor frontier and the
recovered 32-object large-RX profile. Increasing that geometry is not a safe
generic tuning knob: it needs separate vendor-oracle, SRAM-placement and HIL
qualification. Direct protocol decode into a reserved final RX slot remains a
later experiment; the DMA-to-staging ownership copy must remain until an
alternative is proven against the vendor recycle boundary. Before raising the
throughput claim, repeat the final 70-Mbit/s cell across multiple cold boots
and qualify the same scheduler under AP, AP+STA, sniffer and power-save modes.

## HT40 / 150-Mbit/s preflight

HT40 MCS7 with short GI is still a useful A/B because its nominal 150-Mbit/s
PHY changes the airtime and descriptor-arrival pattern without changing the
software ownership pipeline. It was not honestly available in this RF cell:

- the ordinary FRITZ peer advertised HT support but no secondary channel, so
  station selection correctly remained HE20;
- the controlled `hostapd` HT40 profile requested channel 11 with a lower
  secondary channel, but Linux reported an actual 20-MHz channel after the
  mandatory 20/40 coexistence scan found neighboring HT20 BSSes.

Forcing 40 MHz despite that coexistence result would not be a valid
qualification. `open-radio-net start-ht40` now fails closed unless `iw`
confirms an actual 40-MHz channel context; the installed privileged helper must
be refreshed with `tools/open-radio-net/install.sh` before using this check.
The complete HT40 qualification therefore needs an RF-shielded cell or a
legitimately clear 2.4-GHz channel pair. Once available, run reset-separated
RX-only, TX-only and bidirectional cells with `--phy ht40` and require the
observed 40-MHz RX vector plus 150,000-kbit/s TX rate; do not infer HT40 from
the requested AP configuration alone.
