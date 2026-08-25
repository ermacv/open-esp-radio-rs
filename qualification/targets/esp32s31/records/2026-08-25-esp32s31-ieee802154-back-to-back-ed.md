# ESP32-S31 production back-to-back IEEE 802.15.4 ED recovery

Date: 2026-08-25

Evidence ID: `HIL_ESP32S31_IEEE802154_BACK_TO_BACK_ED_2026_08_25`.

This record qualifies two consecutive invocations of the production
route-detached polled energy-detection API without a MAC reset between them.
It proves owner reuse after the exact selected `ED_DONE` recovery path. It
does not qualify calibrated RSS, RF retuning, CCA, active IRQ, or RX/TX.

## Cell

- Repository commit:
  `6c7940aa4acc48af7ed8c1c755dc8a4af59f7a65`.
- Target: ESP32-S31 revision 0.0 on `/dev/ttyACM0`.
- Commands:
  `cargo hil image flash diagnostic-ieee802154-ed-event` and
  `cargo hil run ieee802154-ed-event-selective-write`.
- Scenario: `hil/scenarios/ieee802154-ed-event-selective-write.toml`, schema 3,
  three reset-isolated boots, `poll_limit = 100000`,
  `timer_threshold = 1000`.
- Image class: `diagnostic-ieee802154-ed-event`.
- Application SHA-256:
  `9272f52021cadb2bbdce0caa0ae6ebcc1e5026f1bb4251ce832b9115130aa31a`.
- Runtime-advertised CRC32: `0xab50709c`.
- Result document: `ieee802154-ed-event.json`, schema 2, SHA-256
  `ee94a477ea43fee131b0ae46a037ad84ac5f3df84c90f94b717c04ca2e042b84`.
- Result: `PASS`, 3/3 boots.

The ignored raw run remains under
`target/hil/esp32s31/runs/ieee802154-ed-event-selective-write/`; UART logs,
firmware images and generated run documents are not committed.

## Production observations

Every boot entered the ordinary clock, reset, masked foundation and static
policy path on channel 11. It then called
`Ieee802154MacPolicyConfigured::energy_detection_raw(8, budget)` twice. The
first success returned the same reusable policy owner; that owner was consumed
directly by the second call, with no reset or foundation/policy reconstruction.

All three boots returned identical production evidence:

| Attempt | Result | Polls | Raw `ED_RSS` code |
| --- | --- | ---: | ---: |
| First | `Complete` | 188 | -128 |
| Second | `Complete` | 189 | -128 |

For each operation the production state machine required both complete
source-132 CPU route words to remain zero, enabled only `ED_DONE` and
`RX_ABORT`, enabled only ED abort/stop/coexistence-reject reasons, accepted
only lone `ED_DONE`, wrote the fixed generated `0x0040` selected image,
required the complete status field to read zero, remasked both enable sets and
returned ownership only after all recovery readbacks passed.

After the two production calls, the same owner entered the existing diagnostic
paired-bit discriminator. It again observed `ED_DONE | TIMER0 = 0x0140`, left
TIMER0 at `0x0100` after the selected `ED_DONE` write, cleared TIMER0 with its
separate diagnostic image, and ended with zero event and abort enables and
zero pending status. No RX abort or STOP occurred.

This closes the earlier repeated-operation gap for raw ED command completion
and exact selected-write recovery on this hardware cell. The stable value
`-128` is retained only as a signed register code; it is not interpreted as
dBm, sensitivity, noise floor, or proof that RF was tuned to channel 11.

## Explicitly not proven

The schema-2 result retains this exact `not_proven` list:

- `full-register-w1c-semantics`;
- `non-ed-event-write-semantics`;
- `concurrent-same-bit-arrival`;
- `level-triggered-route-behavior`;
- `production-phy-rf-btbb-readiness`;
- `synchronous-stop-semantics`;
- `calibrated-rss-or-dbm-conversion`;
- `rf-channel-retune`.

The cell also did not execute the production CCA result path, install source
132 on a CPU, transfer a frame, use DMA, or prove scheduling fairness. The
current public helper consumes a finite poll budget synchronously; an
executor-integrated operation boundary remains a tracked async gap rather than
an implicit readiness claim.
