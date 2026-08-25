# ESP32-S31 IEEE 802.15.4 EVENT_STATUS selective acknowledgement

Date: 2026-08-25

This record preserves one bounded hardware observation of the ESP32-S31
IEEE 802.15.4 `EVENT_STATUS` register. It is not a register-wide W1C claim and
does not qualify an active interrupt route or an operational RX/TX dataplane.

## Cell

- Repository base commit:
  `093cfe9803f55615ec405f7de36faf3cb8405aac`.
- Target: ESP32-S31 revision 0.0 on `/dev/ttyACM0`.
- Command: `cargo hil run ieee802154-event-status-selective-ack`.
- Scenario: `hil/scenarios/ieee802154-event-status-selective-ack.toml`, schema
  3, three reset-isolated boots, `poll_limit = 100000`,
  `timer_threshold = 1000`.
- Image class: `diagnostic-ieee802154-event-status`.
- Application SHA-256:
  `6544cb5767f3312d95299c8a19574a61a7a2acd71ce7eb63fa8b3dfe788b831d`.
- Runtime-advertised CRC32: `0x6deeffdc`.
- Result document: `ieee802154-event-status.json`, schema 2, SHA-256
  `37f8656b54d4f598f4224e4672ea5e2486fb36da23e504b2fa0551feb5b24222`.
- Result: `PASS`, 3/3 boots.

The raw run remains in the ignored host-output directory
`target/hil/esp32s31/runs/ieee802154-event-status-selective-ack/`; no UART
capture or generated evidence is committed by this record.

## Observations

All three boots produced the same bounded checkpoints:

| Checkpoint | `EVENT_STATUS` |
| --- | ---: |
| Before event enable | `0x0000` |
| Exact active `EVENT_ENABLE` mask | `0x0300` |
| TIMER0 and TIMER1 observed and latched | `0x0300` |
| After acknowledging TIMER0 with `0x0100` | `0x0200` |
| After acknowledging TIMER1 with `0x0200` | `0x0000` |
| Final cleanup sample | `0x0000` |

The distinct-arrival discriminator first sampled TIMER0 as `0x0100`, then
observed `0x0300` after TIMER1 arrived while TIMER0 remained latched, and
observed `0x0200` after acknowledging only TIMER0. Both timer counters reached
the exact bounded threshold 1000. The source-132 CPU route words for both
cores remained reset-detached (`0`) before event enable, while events were
enabled, and after cleanup. `EVENT_ENABLE` returned to `0` after the probe.

This is evidence that, under this closed timer-only transaction, writing the
TIMER0 mask clears the latched TIMER0 bit without clearing TIMER1, and writing
the TIMER1 mask then clears TIMER1. It is sufficient to retain a reviewed
timer-bit selective-ack observation. It is not sufficient to assign one
modified-write class to the complete fourteen-bit register.

## Explicitly not proven

The schema-2 evidence document retains this exact `not_proven` list:

- `full-w1c-semantics`;
- `event-enable-generation-vs-visibility-semantics`;
- `concurrent-same-bit-arrival`;
- `level-triggered-route-behavior`;
- `masked-final-status-means-physical-cleanup`.

In particular, the cell did not exercise radio completion, abort, SFD or
clock-count bits; did not create a new arrival of the same event bit during an
acknowledge; and did not connect source 132 to either CPU. Production code must
therefore continue to treat the complete `EVENT_STATUS` write semantics as
unknown. Each operational event class needs its own paired-bit HIL evidence,
and continuous receive needs a same-bit concurrency result, before broader
acknowledgement behavior can be published.
