# ESP32-S31 UDP, TCP and ICMP network regression

Date: 2026-08-06

Evidence ID: `HIL_ESP32S31_NETWORK_REGRESSION_2026_08_06`

## Cell

- target: ESP32-S31 revision 0.0;
- peer: external WPA2 HE20 AP;
- memory profile: PSRAM code/data with ISR, DMA and stack placement audited in
  internal SRAM;
- driver revision: `4a7b166`;
- transports: UDP RX, saturated UDP TX, simultaneous UDP RX/TX, TCP RX and
  host-to-device ICMP Echo;
- HIL protocol: v9 runtime provisioning and typed traffic sessions.

Credentials remained host-owned and were sent through the framed startup
protocol. They were not compiled into an image or retained in this record.

## False UDP TX regression

The first saturated TX-only repeats reported 18 and 20 missing UDP sequence
numbers. The simultaneous RX/TX cell reported 12. There was no reorder or
duplication. The exact sum, 50, matched the change in the host Linux counters:

```text
UdpInErrors=50
UdpRcvbufErrors=50
```

The target independently reported all aggregate MPDUs acknowledged, with zero
hardware timeout or collision. The losses therefore occurred after the host
kernel received the traffic, before the runner drained its UDP socket. The
default host `SO_RCVBUF` was only 212,992 bytes.

The HIL runner now requests a 4 MiB UDP receive queue and records the actual
`SO_RCVBUF` read-back. Linux reported 8,388,608 bytes because its API includes
kernel bookkeeping in the returned value. Reset-separated TX-only and
bidirectional repeats then had zero missing, reordered or duplicate sequence
numbers, while the system-wide `UdpRcvbufErrors` counter remained at 50.

This was a host qualification defect, not a driver packet-loss regression.

## Typed TX session accounting regression

A later repeat reported only 40,223 host datagrams while the typed target
session reported 121,152. The target had acknowledged every MPDU and neither
the UDP nor interface counters explained the difference. The receiver had
split its observation whenever the stream was idle for 500 ms, then qualified
only the first fragment whose sequence began at zero. Continuations of the
same typed session were therefore silently excluded.

The protocol session is now the authoritative boundary. Its receiver retains
one sequence observation until the session deadline even across temporary
radio or scheduling stalls. Idle and sequence-zero delimiting remain only for
legacy firmware which has no typed session. The unchanged driver image then
completed four reset-separated saturated repeats with exact target/host
delivery.

## Typed TCP readiness regression

The first TCP RX attempt reached textual `tcp-rx-ready` and DHCP but timed out
on the host. `ServiceReady` and `NetworkReady` had used the best-effort event
queue, so a full queue could erase a required control transition. The host
correctly refused to start an untyped stream.

Readiness events now use the reliable protocol queue outside the measured
traffic interval. Best-effort publication remains reserved for observations
which must not apply backpressure to the radio/network hot path. The rebuilt
TCP image then completed its typed session normally.

## Results

| Cell | Result |
| --- | --- |
| UDP RX-only | 20.002 Mbit/s; 25,001/25,001 datagrams; zero software or hardware drops |
| UDP TX-only | Four reset-separated device floors of 86.942, 89.093, 87.585 and 88.517 Mbit/s; exact delivery and zero sequence defects, timeouts or collisions in every run |
| UDP RX+TX | Two reset-separated runs at 10.004/69.445 and 10.007/71.156 Mbit/s RX/TX; exact delivery and zero sequence defects |
| TCP RX-only | 30,017,600 bytes exact; 19.985 Mbit/s; one EOF-completed stream; zero software or hardware drops |
| ICMP | 100/100 replies; 0% loss; min/median/average/p95/p99/max 2.573/3.090/3.272/3.660/5.560/14.137 ms |

The ICMP series used 56-byte payloads and a 100-ms interval. It measured the
complete laptop/AP/open-driver/Embassy network-stack round trip while the
ordinary `radio` image was connected. It is a latency observation, not a
hard-real-time deadline guarantee.

The latest TX-only report contains 120,284 datagrams and 177,058,048 bytes at
88.517 Mbit/s on the target and 88.515 Mbit/s on the host. Its maximum host
packet interarrival was 21,624 us; no timing-based burst split occurred. The
latest bidirectional report contains exactly 12,501 RX and 72,512 TX datagrams.

## TX performance investigation

The remembered 98.752--108.237-Mbit/s result is real, but it is not a HE20
baseline. It used a controlled Linux HT40 AP, MCS7, short GI and a nominal
150-Mbit/s PHY. The external FRITZ peer used by this record negotiates HE20,
MCS9 and a nominal 114.7-Mbit/s PHY. An HT40 result therefore cannot establish
a regression in the current HE20 cell.

An exact detached build of pre-refactor revision `fb63b7b` was flashed to the
same board and tested against the same FRITZ peer in the same RF interval. Its
repeated device samples were 85.68--87.44 Mbit/s. Preparation/publication
returned to 297--302/23.6--23.9 us, proving both that the CPU timing regression
is real and that it does not explain the missing 100-Mbit/s result in HE20.

Revision `4a7b166` was then rebuilt and flashed without current driver edits.
It reached 88.44 Mbit/s with 321.23/87.03-us preparation/publication. Two MPDUs
remained unacknowledged after the retry limit. Current owner-bound descriptor
publication reached one exact-delivery run at 87.760/87.244 Mbit/s host/device
and reduced publication from roughly 55 us in the first safe repair to 33.38
us. Preparation was 359.87 us because address/range validation now occurs once
at commit rather than being reconstructed by the upper MAC at submission.

Later saturated repeats of both unchanged `4a7b166` and the owner-bound path
observed small final BlockAck failures. In each case host sequence gaps exactly
matched `subframes - acknowledged`; they are therefore not host socket loss
and are not specific to the new retry backing map. A host test now also covers
two consecutive logical compactions, `[1, 3] -> [3]`, before releasing the
physical backing.

The safe path still costs about 70 us per full aggregate relative to the old
caller-asserted unsafe lifetime contract. That is a genuine CPU optimization
target, but the hardware A/B does not show a corresponding HE20 throughput
regression. The next performance qualification must compare the same PHY,
channel width, AP, memory profile and RF interval; an actual 100-Mbit/s claim
requires restoring a valid HT40 test cell.

## Image and artifact identity

- ordinary radio runtime CRC-32: `61aa0fb7`;
- UDP TX runtime CRC-32: `141eca79`;
- bidirectional runtime CRC-32: `fd9c87f7`;
- final reliable-readiness TCP runtime CRC-32: `a0fc08f2`;
- UDP RX report/UART SHA-256:
  `f5b2565f06099a5c19445cdd56f81ac325342ca63ef6d5bd7bb85ffaa76ca499` /
  `bd925b23274d5fe2829297d7a250dc7e2c6b4dcfd4f4c0cea1e4aff674597dce`;
- UDP TX report/UART SHA-256:
  `d09c471f6be5ee67f2f9f54d3b509e38dc2c09f732947239da3140ae84f1b73a` /
  `a97c0c9f7db874151d3a1c1e0871b1de30bf5f126348a02e60c17d2878e07d5e`;
- bidirectional report/UART SHA-256:
  `061b0eead16d84e5cb120b2d3f459f2a6262c75c7b661bed4cb07ebe1ff8a123` /
  `ae5cffefdb4e3e1d478fa481ba16e49c8643e4ecc322263c66964b71c1e7b2af`;
- TCP report/UART SHA-256:
  `2393296fbeaf18c905f933e3e89730e4d4a37ea7acefe1bb133ff7bc3632a9f5` /
  `0a6ebbac2aba750b8f0d4ad095ed64a6a03ae74a8957662c6d8d218f0c5c5229`;
- ICMP transcript SHA-256:
  `51a9f89854a0063bc249f69f76a819b524a31741705ceb25a7b8eb10187770e4`.

This record proves transport correctness after removing both host receive
queue loss and typed-session fragmentation. It does not claim that the older
publication cost or roughly 90-Mbit/s floor has been retained.
