# ESP32-S31 IEEE 802.15.4 source capabilities

See the [whole-radio capability map](../FEATURES.md) for shared ownership,
clock/power lifecycle and cross-protocol composition limits.

This inventory describes silicon features, source-owned operations and their
composition limits. It does not claim a complete IEEE 802.15.4-2015 stack,
calibrated RF operation or on-air qualification.

| Status | Meaning |
| --- | --- |
| IMPLEMENTED | A bounded source path owns the complete operation in the exact scope named by the row, including static policy or a polled MAC primitive. This is not an RF qualification result. |
| PARTIAL | A source subset exists, but the named feature lacks a complete operational composition. |
| FAIL-CLOSED | A typed boundary deliberately prevents the stated activation/publication. Known registers alone do not establish this status. |
| ABSENT | No production protocol/runtime owner exists for the operation. |
| HOST-ONLY | An upper-stack function, not a radio/MAC implementation claim. This label does not claim that a stack integration exists. |

The [qualification specification](../../../../qualification/targets/esp32s31/ieee802154.toml)
owns readiness and hardware-evidence requirements independently. It marks
RF/channel readiness, active IRQ runtime and RX/TX dataplane composition
incomplete. Complete static MAC policy or raw ED/CCA does not supply those
missing boundaries.

## Qualification scope mapping

| Feature scope | Qualification capability | Agreement |
| --- | --- | --- |
| Clocks, reset and masked foundation | `clock-reset-foundation` | Complete finite source transition; hardware evidence is separate. |
| CCA modes, threshold and primary PAN identity | `static-mac-policy` | Complete static policy/readback, not a complete receive service. |
| Raw ED / standalone CCA | `polled-ed-cca` | Implementation is complete; async remains incomplete because the public path runs a finite synchronous poll budget. |
| Registered PHY and protocol timing / channel operation | `rf-channel-readiness` | `ieee802154-registered-timing-entry` exists; shared PLL, RF retune and tracking lifetime composition remain incomplete. |
| Active IRQ service | `active-interrupt-runtime` | Lower IRQ machinery does not close whole-radio route/runtime composition. |
| Lower RX/TX actor and DMA / public dataplane | `rx-tx-dataplane` | `ieee802154-mac-operation-subset` exists; complete RF-ready public operation is still incomplete. |

The [program](../../../../qualification/targets/esp32s31/ieee802154.toml) retains
these scopes and source contracts. It does not qualify absent features such as
CSMA/CA or CSL, and upper stacks remain outside the radio/MAC target.

## Capability sources and publication scope

The [ESP32-S31 datasheet v0.5, section 4.3.5](https://www.espressif.com/sites/default/files/documentation/esp32-s31_datasheet_en.pdf)
defines the hardware inventory. The [ESP-IDF IEEE 802.15.4 API](https://github.com/espressif/esp-idf/blob/master/components/ieee802154/include/esp_ieee802154.h)
also describes vendor operations such as timed RX/TX, source matching and
security; a vendor API is not evidence of an open S31 implementation.

[Portable `RadioCapabilities`](../../../ieee802154/src/radio/capabilities.rs)
is a vocabulary, not the S31 backend's supported-feature image. In particular,
`CSMA_CA`, `SCHEDULED_TRANSMIT`, `AUTOMATIC_ACKNOWLEDGEMENT`, `SECURITY_OFFLOAD`
and `SOURCE_MATCH` constants do not compose their corresponding operations.
The same applies to portable commands, configuration and software sleep states.

The [Embassy MAC owner](../../../adapters/embassy/esp32s31/ieee802154/src/owner.rs)
provides `receive_without_auto_ack()`, `transmit_without_ack()`,
`transmit_with_ack()`, `clear_channel_assessment()` and `energy_detection()`.
These are lower runtime entry points, not a complete public RF-ready service.
The [MAC actor](mac/src/actor.rs) admits only Direct or
ClearChannelAssessment TX access; neither is a CSMA/CA scheduler.

## PHY and RF

| Feature | Status | Current production boundary |
| --- | --- | --- |
| 2.4 GHz O-QPSK / 250 kbit/s | PARTIAL | MAC/PHY configuration and frame geometry exist, but registered common-PHY, RF wake/tracking and whole-radio operation are not composed. |
| Channels 11-26 | PARTIAL | [Channel validation](../hal/src/ieee802154/lifecycle.rs) and frequency-code publication exist. Static MAC channel readback does not establish RFPLL/BTBB retune readiness. |
| RSSI | PARTIAL | [RX frame view](dma/src/frame.rs) exposes the hardware RSSI byte; calibrated measurement through a complete receive service is not established. |
| LQI | PARTIAL | RX frame view exposes hardware LQI; the whole-radio receive path remains incomplete. |
| TX power | PARTIAL | [Power resolution](../hal/src/ieee802154/tx_power.rs) validates caller-supplied levels and resolves a dBm request to an opaque provider index. A calibrated S31 provider and MMIO publication are not composed; the resolved value grants no hardware authority. |

## CCA and channel access

| Feature | Status | Current production boundary |
| --- | --- | --- |
| Energy Detection command | IMPLEMENTED | [Polled operation](../hal/src/ieee802154/operation.rs) owns a finite route-detached raw ED command, duration, event/completion and bounded recovery. It does not establish calibrated RF energy, a multi-channel scan or a nonblocking public service. |
| Standalone CCA command | IMPLEMENTED | The same serialized polled boundary owns a finite MAC CCA operation. RF readiness and busy-channel HIL coverage remain separate requirements. |
| Four CCA modes | IMPLEMENTED | [Static policy](../hal/src/ieee802154/policy.rs) writes Carrier, Energy Detection, OR or AND selection and checks semantic readback. |
| CCA threshold | IMPLEMENTED | Static policy writes and verifies the threshold; this is configuration coverage, not calibrated RF assessment. |
| CCA-before-TX | PARTIAL | The actor/executor owns ClearChannelThenTransmit command selection. Whole-radio transmission remains incomplete. |
| CSMA/CA | ABSENT | No random backoff, contention/retry state or CSMA/CA transmission owner exists. A single CCA-gated command is not CSMA/CA. |
| Active Scan | ABSENT | No beacon-request and response-collection MAC procedure exists. Raw ED is a separate operation. |

## RX/TX dataplane and acknowledgments

| Feature | Status | Current production boundary |
| --- | --- | --- |
| RX | PARTIAL | [RX DMA](dma/src/rx.rs), actor command, acknowledged completion and bounded frame extraction exist. RF-ready public service and on-air reception remain incomplete. |
| Direct TX | PARTIAL | [TX DMA](dma/src/tx.rs), MAC actor and PAC command publication exist without a complete RF-ready radio composition. |
| Hardware FCS generation/check | PARTIAL | TX reserves FCS for hardware; the RX format replaces its two bytes with RSSI/LQI. Parsing that layout alone does not qualify on-air FCS behavior. |
| TX ACK request and ACK reception | PARTIAL | ACK-request admission, RX_ACK phase, watchdog and correlation exist. Complete on-air ACK exchange remains unqualified. |
| ACK timeout configuration | IMPLEMENTED | Static policy validates and converts requests to hardware timeout units, then checks semantic readback. This does not establish a working ACK exchange. |
| Hardware Auto ACK generation | FAIL-CLOSED | The [command executor](runtime/src/pac_command_executor.rs) rejects Receive when `tx_auto_ack` or `enhanced_ack_tx` is enabled. The exposed RX operation is explicitly `receive_without_auto_ack()`. |
| Enhanced ACK TX | FAIL-CLOSED | The same receive-policy guard prevents activation. No complete Enhanced ACK formatter/security/completion owner exists. |

## Filtering, addressing and MAC automation

All IMPLEMENTED rows in this section refer to interrupt-masked static policy
with semantic readback. They do not establish operational packet filtering.

| Feature | Status | Current production boundary |
| --- | --- | --- |
| Hardware frame filtering | PARTIAL | PAN identity and coordinator/promiscuous control are configured, but whole-radio filtering and on-air admission are not closed. |
| PAN ID | IMPLEMENTED | Static policy owns primary-context PAN identity write/readback. |
| Short address | IMPLEMENTED | Static policy owns primary-context short-address write/readback. |
| Extended address | IMPLEMENTED | Static policy owns primary-context extended-address write/readback. |
| Coordinator filtering mode | IMPLEMENTED | Static policy owns the coordinator control bit and readback; this is not a coordinator protocol stack. |
| Promiscuous receive mode | PARTIAL | Static control and readback exist; no complete public operational receive service is composed. |
| Four Multi-PAN contexts | PARTIAL | [PAC MAC access](../pac/src/ieee802154/mac.rs) represents four contexts. Production static policy uses context zero; no multi-context service/admission lifecycle exists. |
| Automatic Frame Pending | PARTIAL | Static `enhanced_pending` control and PAC frame-pending access exist. Source-match and pending-data lifecycle are absent; automatic ACK transmission remains blocked by the RX policy above. |
| Source Matching table | ABSENT | No production add/remove/reset, address ownership or pending-data selection lifecycle exists. |

## Timing

| Feature | Status | Current production boundary |
| --- | --- | --- |
| MAC timers / clock match | PARTIAL | PAC exposes timer and clock-match primitives, not a complete scheduled-radio epoch or service. |
| TX/RX SFD events | PARTIAL | [IRQ vocabulary](irq/src/lib.rs) and PAC events represent SFD observations. Complete source-route/runtime composition remains incomplete. |
| RX timestamp | PARTIAL | Timer/SFD primitives exist, but receive frames do not publish a timestamp in a validated monotonic radio epoch. |
| Timed TX (`transmit_at`) | ABSENT | No operational scheduled-TX owner composes timer, RF readiness, DMA and completion. |
| Timed RX (`receive_at`) | ABSENT | No operational scheduled-RX window owner exists. |
| Coordinated Sampled Listening (CSL) | ABSENT | No CSL scheduling, synchronization or protocol owner exists. |

## Security, power and coexistence

| Feature | Status | Current production boundary |
| --- | --- | --- |
| Hardware TX security | PARTIAL | PAC security controls and programming access exist. No complete runtime key/nonce/counter and protected-frame publication lifecycle is composed. Register access does not establish security offload. |
| RX-on-when-idle | ABSENT | Portable state exists without a dedicated S31 receive/rearm policy and radio lifetime. |
| Radio sleep / wake | ABSENT | No RF/PHY/baseband stop/wake, retention and restoration owner is composed. A portable sleep state or stopped MAC is not hardware radio sleep. |
| Powered lifecycle | PARTIAL | Clock/reset foundation and masked-policy transitions exist. Shared-PHY maintenance and complete RF-ready start/stop ownership remain incomplete. |
| Wi-Fi/Bluetooth coexistence | PARTIAL | [Coexistence infrastructure](../coex/src/lib.rs) and MAC PTI access exist. Static IEEE 802.15.4 policy requires TX/RX and ACK PTI disabled; complete multi-radio request/grant/release operation is not composed. |

## Product stacks

| Feature | Status | Current production boundary |
| --- | --- | --- |
| Thread | HOST-ONLY | Network/upper-MAC stack integration requires an adequate radio backend; this matrix does not claim a production Thread integration. |
| Zigbee | HOST-ONLY | Network/application stack capability is separate from the radio/MAC implementation. |
| Matter | HOST-ONLY | Application-stack capability; Matter over Thread requires Thread and its radio integration. It is not a hardware MAC feature. |

## Ownership and readiness

| Owner | Authority |
| --- | --- |
| [HAL](../hal/src/ieee802154.rs) | Masked clock/reset foundation, static policy and bounded polled operations |
| [PAC MAC](../pac/src/ieee802154/mac.rs) | Typed register operations and hardware-access leases |
| [DMA](dma/src/lib.rs) | CPU/device buffer ownership, frame geometry and terminal reclamation |
| [MAC actor](mac/src/actor.rs) | Serialized operation state, acknowledged IRQ batches, ACK watchdog and completion |
| [IRQ](irq/src/lib.rs) | Bounded event/abort classification and acknowledged batch handoff |
| [Runtime](runtime/src/lib.rs) | Executor-neutral command/completion progression; PAC executor enforces operation policy |
| [Embassy owner](../../../adapters/embassy/esp32s31/ieee802154/src/owner.rs) | Async waits and retained lower MAC/DMA owners |

The missing whole-radio service must connect these owners to RF/channel
readiness and the active IRQ route. Tests of the lower state machines do not
supply this composition. Qualification tracks those implementation gaps and
on-air RX/TX/ACK/filtering evidence independently of this inventory.
