# IEEE 802.15.4 trace

`oer-ieee802154-trace` defines the trace points of the IEEE 802.15.4 MAC
driver as [`oer-trace`](../../../trace/src/lib.rs) events in
`Domain::Ieee802154`. The target that emits them and the host that decodes
them link the same types, so no record is read as a bare number.

The types are chip-neutral and use the ESP-IDF driver's vocabulary: the
private driver states of `esp_ieee802154_dev.c`, `esp_ieee802154_tx_error_t`
and the receive and transmit abort reasons with their vendor values. Every
encoding uses integer operations only, so the MAC interrupt handler emits
them while the FPU is off.

| Kind | Channel | Event | Emitted at |
| --- | --- | --- | --- |
| `ieee802154.1` | 0 | `StateChange` | Every change of the engine's driver state |
| `ieee802154.2` | 1 | `TxOutcome` | The engine reports a transmission done or failed |
| `ieee802154.3` | 2 | `RxOutcome` | The engine delivers a frame or drops it into the stub buffer; the runtime drops one on a full event queue |
| `ieee802154.4` | 3 | `Abort` | The interrupt handler samples a receive or transmit abort, before it acts on the reason |
| `ieee802154.5` | 4 | `Interrupt` | Every engine interrupt entry: the state and the sampled events |
| `ieee802154.6` | 5 | `PowerSequence` | A chip HAL starts a transmission: the power-sequencing delays |
| `ieee802154.7` | 5 | `DcdcControl` | With `PowerSequence`: the DC-DC control and which power words have reserved bits set |
| `ieee802154.8` | 6 | `Timer` | The engine arms or stops a MAC timer, or handles its overflow |
| `ieee802154.9` | 7 | `Lease` | The runtime pauses or resumes the radio around shared PHY work |

The Domain::Ieee802154 range has twelve channels (mask bits 52 to 63);
8 to 11 are unassigned. The two power events share one channel because they
are emitted together. The engine emits behind its `trace` feature, the
ESP32-S31 HAL behind `ieee802154-trace` and the ESP32-S31 runtime behind
`trace`; each enables `oer-trace/record`. Without it, `oer_trace::emit`
compiles to nothing, and the HAL reads the power registers only with its
feature on and the channel enabled.

A host describes drained records with `Ieee802154Trace` (an
`oer_trace::EventSet`).
