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

## Negotiated RX-unit capacity follow-up

Three additional cold-boot 70-Mbit/s runs exposed a separate ownership-size
bug. The DMA buffers and connected dispatcher already supported the negotiated
3,839-byte A-MSDU class, but the intermediate 32-slot staging pool still used
the ordinary vendor singleton capacity of 1,700 bytes. Two boots happened to
see zero or 51 oversize units. A third boot recycled 1,163 otherwise valid
units and delivered only 433,719 of 437,504 host datagrams, despite zero
hardware `BUFFER_FULL`, FIFO overflow or software queue drop. The correlation
shows why a hardware-clean result alone was insufficient.

The staged frame, queue, protocol owner and DMA service now carry an explicit
const-generic capacity. The production HIL selects 4,608 bytes, matching its
already-qualified DMA buffer geometry; the default library type retains the
1,700-byte vendor profile for configurations which do not negotiate a wider
unit. A unit test covers admission immediately above the old limit.

On a reset-separated 60-second final run, the wider path received 436,944 of
437,504 host datagrams at 69.912 Mbit/s, published 437,113 network frames and
recorded zero empty/oversize recycle, hardware starvation, FIFO overflow or
software drop. Protocol data units numbered 436,486 while network publications
numbered 437,113. The additional 627 publications are direct evidence that the
dispatcher emitted multiple Ethernet MSDUs from some staged units; reducing
the staging capacity back to one ordinary MPDU is therefore invalid for this
association.

## Descriptor-only TX policy and admission follow-up

Adding a legitimate ordinary-MPDU fallback exposed a latent ownership error in
the descriptor-only A-MPDU owner. The first implementation tested capacity by
performing a speculative `begin -> can_push -> cancel` transaction. On the
target, the peer byte ceiling retained beside the DMA descriptors was zero by
the time the next network frame was admitted, even though construction had
installed 65,535 bytes and the selected HE MCS9 default APEP ceiling was
50,000 bytes. Every 1,514-byte Ethernet frame was consequently sent as an
ordinary MPDU and TX fell to roughly 17--21 Mbit/s.

The final boundary no longer mutates descriptor ownership to answer a value
question. Fresh-frame admission is a pure calculation over encoded length,
DMA capacity, negotiated peer limit, PHY-rate limit and HE TXOP. The peer limit
comes from `StaTxRuntimePolicy`; immediately before each real `Free ->
Reserved` transition the connected owner reinstalls that limit in the DMA
storage. Thus association policy is not inferred from cold scalar contents
co-located with hardware-owned descriptors. This also removes one complete
reservation/cancellation cycle per aggregate.

Reset-separated HE20 hardware regressions on the final implementation were:

- TX-only: `PASS`; host/device floors 90.585/91.351 Mbit/s, 77,756 datagrams,
  zero missing or reordered. A-MPDU averaged 30.99 MPDUs, with 2,505 exact
  31-member aggregates and one 32-member aggregate. Preparation/publication
  averaged 297.66/23.76 us; there were no timeouts or collisions.
- Bidirectional: `PASS`; all 12,501 downlink datagrams arrived at 10.002
  Mbit/s while concurrent TX remained at least 65.232 Mbit/s. A-MPDU averaged
  30.94 MPDUs and reached 32; no hardware RX starvation, FIFO overflow,
  software drop, unknown interrupt cause, TX timeout or collision occurred.
- RX-only: `PASS` at a 70.001-Mbit/s host offer; device median was 69.637
  Mbit/s, with zero hardware starvation, FIFO overflow, software drop,
  empty discard or oversize discard. This particular 30-second run observed
  376 transient staging-credit-limited services and delivered 217,072 of
  218,752 UDP datagrams, so it is a regression check rather than a replacement
  for the earlier exact-delivery 70-Mbit/s qualification.

These results establish that the old roughly 70-Mbit/s TX observation was not
a PHY ceiling. The remaining RX-only variance is in staging/protocol credit
turnaround and is independent of the repaired aggregate admission path.

## Hot-observer and credit-depth follow-up

The apparent roughly 15.5-ms protocol-dispatch tail was not a data-parser
latency. `dispatch_max_us` retained a boot-lifetime maximum, while the HIL
observer synchronously printed the received BlockAck action through UART from
inside `ConnectedRxSink::publish`. Removing that hot-path print reduced the
cold-boot dispatch maximum to 43 us. UART field names now explicitly say
`boot_max`, so an interval total can no longer be compared with a mislabeled
lifetime peak.

An attempted 32-frame cooperative ingress quantum was rejected after HIL:
ending `smoltcp` ingress after one hardware window increased `BUFFER_FULL`
from one observation to 32 without reducing the IRQ-to-service tail. The
64-entry network ingress queue must remain drainable; this experiment did not
justify a NAPI-like budget at the `embassy-net` device boundary.

The retained implementation removes the unused per-data `ProtectedFrame`
callback (the production control adapter discarded it) and samples HIL PHY
metadata once per 64 Ethernet frames instead of decoding and updating
diagnostic atomics on every benchmark packet. It also records maximum deferred
frontier length and minimum credits at a backpressured service. A cold 30-s
HE20 run then delivered all 218,752 UDP datagrams at a 70.001-Mbit/s host
offer, with zero `BUFFER_FULL`, FIFO overflow, software drop, oversize discard
or unknown IRQ. Dispatch averaged 19.11 us/frame with a 39-us boot maximum;
81 services observed backpressure, with a maximum 30-frame deferred suffix
and one remaining pool/queue credit. This shows burst phase overlap rather
than a permanent zero-credit deadlock.

## Staging-pool elasticity follow-up

The credit-depth telemetry then exposed a real burst boundary rather than a
per-frame CPU limit. A reset-separated 70-Mbit/s, 10-s run with the ordinary
32-slot staging profile reached zero pool and queue credits and recorded three
hardware `BUFFER_FULL` observations. The staging owner was also unnecessarily
limited to one native bitmap word, which made every RV32 platform profile
incapable of expressing more than 32 independent slots even when its memory
layout allowed it.

`RxStagePool` now uses two native atomic bitmap words and the Embassy staging
types carry the slot count as an explicit platform parameter. The reusable
default remains the vendor-equivalent 32-by-1,700 profile; only this HIL
composition selects 40 jumbo slots. This is an ownership-capacity choice, not
a frame-processing budget: one service still freezes and drains one finite DMA
completion frontier.

The SRAM/stack boundary was tested rather than inferred from linking alone:

- 48 jumbo slots failed the placement audit by 2,176 bytes;
- 47 and 44 slots linked but failed the on-device readiness path with only
  roughly 66 and 80 KiB of runtime-stack space left;
- 40 slots leave roughly 98 KiB and completed boot, association, readiness and
  sustained traffic.

With the retained 40-slot profile, a cold 30-s HE20 run delivered all 218,752
UDP datagrams at a 70.001-Mbit/s host offer and 69.996-Mbit/s device median,
with zero `BUFFER_FULL`, FIFO overflow or software drop. Four services were
briefly pool-limited, but the minimum pool/queue credits remained 1/2 and the
maximum deferred suffix was four frames. The qualified runtime CRC-32 is
`368fe8b1`.

Eighty Mbit/s is not yet qualified. A 10-s run delivered all 83,334 datagrams
with no drop, but the 30-s 40-slot run still recorded four `BUFFER_FULL`
observations. A fixed 100-us application yield removed radio backpressure but
lost 15,668 UDP-socket datagrams; a low-credit feedback yield later lost the
link. Both scheduling experiments were reverted. The remaining work is to
separate RX and TX queue-depth resources and diagnose the occasional
multi-millisecond `embassy-net` readiness wait, not to add another arbitrary
frame batch.

## Interrupt-to-poll experiment

The recovered vendor `wDev_ProcessFiq` is not NAPI-like: it reads one masked
STATUS image, acknowledges the complete image with W1C, publishes RX before
the known TX causes, and repeats until STATUS becomes zero. The open ISR keeps
that transaction and ordering as its equivalence boundary.

The runtime now counts entries into the hard MAC ISR separately from RX event
posts and coalesced Embassy wake epochs. In an ungated 30-second 70-Mbit/s
classification baseline, 220,732 RX-bearing first STATUS snapshots came from
220,734 hard ISR entries and collapsed to 99,929 task wake epochs. There were
no spurious entries, acknowledgement-loop saturations, hardware
`BUFFER_FULL`/FIFO overflow or software drops. Embassy `Signal` was already
coalescing bottom-half work, but it could not avoid the hardware interrupt
cost.

Every sustained-RX snapshot also carried bits 5 and 24, so the ordinary image
was `0x01004020`, not RX_SUCCESS alone. Both bits are enabled by the recovered
cold mask `0x19a879e0`. Complete `wDev_ProcessFiq` acknowledges the full image
but does not dispatch an independent worker for either bit. They are therefore
now named `RX_ASSOCIATED_AUXILIARY_5` and
`RX_ASSOCIATED_AUXILIARY_24` in the source SVD and PAC with
`instruction-exact-semantics-unknown` confidence. They remain outside the
work-producing `HANDLED_MAC_MASK`; the ISR no longer records them as unknown
events, but it still clears them in the exact full-image W1C transaction. Their
electrical meaning and independent transition rules remain open, so neither
bit is an RX ownership or frame-count oracle.

A NAPI-like experiment masked only `RX_SUCCESS` in the MAC `ENABLE` register
after the first event and restored it after one frozen descriptor frontier.
It did reduce RX posts to 114,899, but hard ISR entries increased to 278,537.
Moving the mask before W1C acknowledgement produced the same result: 111,127
posts and 275,638 hard entries. Both runs retained roughly 70-Mbit/s delivery,
but the extra empty/combined-line entries made this policy strictly worse, so
the masking API and runtime gate were removed rather than retained as dormant
production machinery.

The next experiment owned the complete combined CPU `WIFI_MAC` route. The hard
ISR disabled that route after acknowledging and publishing the first event;
the radio task serviced RX/TX, repeatedly inspected newly completed frontiers
for at most 500 microseconds, and reenabled the level route only at a quiescent
boundary. This was race-safe, but not a useful NAPI regime for this workload.
One repeat delivered all 72,918 datagrams at 70.003 Mbit/s with zero hardware
or software loss and reduced hard entries to 42,802 in ten seconds. It needed
51,514 DMA service calls, however, of which 15,191 had an empty frontier, plus
8,712 explicit repolls. Another repeat fell to 69.607 Mbit/s; the simpler
one-frontier variant recorded two hardware `BUFFER_FULL` events and 69.894
Mbit/s. The hard-IRQ reduction had merely been exchanged for more task polls
and less stable descriptor service because the bottom half usually drained
faster than the next frame arrived. The combined-route gate and its policy
state were removed.

The retained design is consequently the vendor-equivalent STATUS/full-W1C
hard ISR plus coalescing Embassy `Signal` and a complete frozen descriptor
frontier per service call. Its natural work boundary is ownership and
backpressure, not an artificial batch of 8 or 16 frames. A future
interrupt-moderation experiment requires a documented hardware coalescing
facility or a workload which actually accumulates useful multi-frame polling
epochs; repeatedly toggling the CPU route is not the next optimization. This
separation is consistent with the Linux NAPI lifecycle, but the measurements
show that copying its scheduling shape without its workload conditions is not
beneficial here; see the
[Linux NAPI documentation](https://docs.kernel.org/networking/napi.html).

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
- Final wide-stage/hard-IRQ 70-Mbit/s RX UART/report SHA-256:
  `3ca91c7f747d03240e92cf9ed77ead8de3fea8bb1fb61ec1f86f2679690dab83` /
  `6cec86ec7733e567f19d87650e3ac0f9723d403c0b1f2bc814a314c4982bd1c2`.
- First auxiliary-status-classified exact-delivery RX UART/report SHA-256:
  `e958b521236c0fbae64a26745c1a2b0013f2a6803bcf48cb79b945329ad67bb9` /
  `88289140cd603d6286f20c3dc04c75c841fe82b52b06bd0ff2e3dacd4dd294d8`.
- Auxiliary-status/final RX regression UART/report SHA-256:
  `a29777ebf8527955f9eee9beb5eb11dc04e369177c8786d3ff0fa037671e5942` /
  `d0f66c8281b3741cea35c22bc63c73759746d1b210fb0d7730d0891c91ad1106`.
- Descriptor-policy TX UART/report SHA-256:
  `fceccf5f919d3434e8c19ac8da673bdf46ed6f7b1b7888d42c6fd19b9d56dffc` /
  `06b02f87eab37bfd928125fa3bffacc88b87b79ef2b1c6c9662bf4ac443b58fd`.
- Descriptor-policy bidirectional UART/report SHA-256:
  `cba4981b87d864d498e7fc36ac8372b187d2371558215f773de771ad9104f303` /
  `936778091d1f9c69fc9eb5ce8dab06c587f3d9fd825a4cfc30d59edf6e2cc016`.
- Hot-observer/credit-depth 70-Mbit/s RX UART/report SHA-256:
  `db20d355c9e7c1517f043fbfffc700c1ca366e8bcfb3c5ee3aace5a0634b2af1` /
  `3ef83ffe58918db5008777c76173cb936e7f77a60714d74affe751157b727f2c`.
- Forty-slot 70-Mbit/s RX UART/report SHA-256:
  `5a60dbc68d8476019ee7048742a4a04bfa8fe369b004ee882bcca2ff18e93af7` /
  `6bf979511296301ac9a451de2a7b1ea0838ca46eb362f9badf2a9905e53631ea`.

Generated UART logs and reports remain under `target/hil/esp32s31/qualification`;
this record preserves their exact identity.

## Remaining boundary

The next performance question is no longer the outer `select5`, Embassy queue
depth or the obsolete 1,700-byte staging ceiling. Independent tasks and the
wider negotiated staging path are qualified. The 80-Mbit/s edge still
coincides with the complete 32-descriptor frontier and the recovered 32-object
large-RX profile. Increasing that geometry is not a safe generic tuning knob:
it needs separate vendor-oracle, SRAM-placement and HIL qualification. Direct
protocol decode into a reserved final RX slot remains a later experiment; the
DMA-to-staging ownership copy must remain until an alternative is proven
against the vendor recycle boundary. Before raising the throughput claim,
repeat the final 70-Mbit/s cell across multiple cold boots and qualify the same
scheduler under AP, AP+STA, sniffer and power-save modes. The combined-line
interrupt-to-poll experiment has been completed and rejected for this
workload; the next CPU-side target is measured protocol-dispatch and copy cost,
while TX remains a separate throughput constraint.

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
