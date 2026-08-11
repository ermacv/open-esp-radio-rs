# ESP32-S31 RX delivery frontier

Qualification ID: `HIL_ESP32S31_RX_DELIVERY_FRONTIER_2026_08_10`.

This cell localizes UDP RX ordering and loss without weakening exact delivery.
The image records the same data sequence at post-BlockAck-reorder, network
enqueue and UDP consumption, pre-reorder ingress/retry cardinality, and the
post-reorder correlation between UDP and 802.11 sequence order.

## Cell and commands

- Board: ESP32-S31 revision 0.0, MAC `30:ed:a0:f3:f6:d0`.
- Base commit: `d45cc4100c9f4fa842c3cac3d030d610191076c9`, plus the
  implementation and record in the same change set.
- Memory profile: `psram-code-psram-data`; stack and placement audits passed.
- Runtime CRC32: HE20 RX `c9477586`; final HT40 bidirectional
  `013452e1`.
- HE20 peer: external FRITZ!Box; host route Ethernet -> OpenWrt -> FRITZ.
- HT40 peer: laboratory OpenWrt; bounded Wi-Fi-egress capture and ath10k
  counters enabled.
- Credentials and device paths came only from ignored, mode-0600 typed lab
  configuration.

```text
cargo hil --lab-config hil/fritz.local.toml traffic rx --rate 90M --seconds 12 --phy he20
cargo hil traffic bidirectional --rate 15M --tx-rate 15M --tx-floor 12M --seconds 12 --phy ht40
```

## Results

- HE20 RX delivered all `112487` datagrams through post-reorder, network
  enqueue and UDP consumption at `89.873 Mbit/s`; queue-full, invalid-length,
  ledger and hardware-overflow counters were zero.
- HE20 exact ordering failed. Sixteen late UDP datagrams all carried a forward
  802.11 MAC sequence. The defect is therefore before target MAC sequence
  assignment, not target BlockAck reorder, network enqueue or UDP consumption.
- Five consecutive reset-separated HT40 bidirectional runs passed exact
  delivery. The final run delivered RX `18751/18751` datagrams at
  `15.003 Mbit/s` and TX `15296/15296` datagrams at `15.043 Mbit/s`, with no
  missing, reordered or duplicate datagrams.
- The final OpenWrt Wi-Fi egress observed `18752` RX packets, exactly the
  payload plus terminal marker; station failed and firmware-drop counters
  were zero.
- Separate OpenWrt RX diagnostics at 40 and 50 Mbit/s showed Wi-Fi-egress
  counts matching host output while fewer frames reached the station/target.
  The OpenWrt AP is therefore retained for controlled lifecycle cells, not
  used as a high-rate performance qualifier.

## 2026-08-11 repeatability check

- Controlled OpenWrt AP loss and prolonged absence passed. Beacon loss was
  reported after `2.482 s`; the finite reconnect policy emitted typed
  `AttemptFailed` edges followed by `RetryExhausted`, and the fixture restored
  the same radio automatically.
- OpenWrt RX at 60, 65 and 90 Mbit/s did not pass exact delivery. At 60 Mbit/s
  the host offered `187439` datagrams while the AP transmitted `187063` and the
  target enqueued `187062` with zero target software or hardware drops. At
  90 Mbit/s the AP queued about `211144` of `281228` offered datagrams. Loss is
  therefore before the target RX frontier.
- Two 20 Mbit/s TX repeats lost complete 16/32-MPDU runs only after finite
  BlockAck retries. Target BlockAck acknowledgement and host delivery counts
  were identical, so no post-BlockAck driver or host loss was hidden.
- TCP remained correct despite the lossy peer: RX `20.012 Mbit/s`, TX
  `58.636 Mbit/s`, and full duplex `10.012/44.973 Mbit/s`, with zero pattern
  errors. ICMP delivered `100/100`; p50/p95/max were
  `2.990/4.896/12.496 ms`.

Exact-delivery remains fail-closed: a fully delivered but reordered stream is
not accepted as `PASS`.

Artifact SHA-256 values:

```text
HE20 RX report  be1681f93a6de431bc230bf2592cc0b42ffbb538315cc6a72324619dbec3a43a
HE20 RX UART    eabc5d938ae90b0893056396a24cb7b0603eff534e18b5e25fc073e2763d9aa3
HT40 bidi report d05b731938f0d02a81e96a296c86ff0421835afe659e7a264b25502f5924c28a
HT40 bidi UART   3385ab372b450be4ae8d55f9fa084026bc70677d4634acedbce6c9e1ce9878d0
```

## Remaining boundary

The target RX path after 802.11 sequence assignment is cleared for these
cells. External pre-MAC ordering and AP/forwarding loss are outside that
frontier. OpenWrt remains the deterministic AP-loss/absence fixture but is not
a stable exact-delivery or performance gate. FRITZ remains the performance and
cross-peer regression fixture. Cold-start and reconnect are covered by the
typed lifecycle HIL rather than inferred from traffic readiness.
