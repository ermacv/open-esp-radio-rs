# ESP32-S31 coexistence source capabilities

See the [whole-radio capability map](../FEATURES.md) for shared ownership,
clock/power lifecycle and cross-protocol composition limits.

This inventory distinguishes recovered models, validation hardware access and
production radio integration. The current code is not a working end-to-end
coexistence layer. A timer write, mailbox reply or matching vendor leaf does
not establish an RF grant, concurrent connectivity or hardware qualification.

| Status | Meaning |
| --- | --- |
| IMPLEMENTED | The exact named source operation is complete at the stated boundary. A model-only row claims only that model operation. |
| PARTIAL | Recovered hardware or source machinery exists without complete operational composition. |
| FAIL-CLOSED | A typed boundary deliberately prevents activation/publication. Missing code or known register addresses alone do not establish this status. |
| ABSENT | No production policy/runtime owner implements the operation. |

The [official S31 coexistence guide](https://docs.espressif.com/projects/esp-idf/en/latest/esp32s31/api-guides/coexist.html)
describes priority arbitration over a shared 2.4 GHz RF path. Wi-Fi, BT and BLE
use state-dependent time slices; IEEE 802.15.4 uses operation-dependent
priorities rather than a fourth fixed slice. These are vendor capabilities,
not open-driver implementation claims.

[S31 SoC declarations](https://github.com/espressif/esp-idf/blob/master/components/soc/esp32s31/include/soc/soc_caps.h)
include hardware PTI and advanced external coexistence. The source boundaries
below determine how much of that hardware the open driver can use.

## Internal arbitration hardware and models

| Feature | Status | Current source boundary |
| --- | --- | --- |
| Shared RF arbitration | PARTIAL | PTI, timer and client models exist. No complete radio request/grant/release composition controls shared RF ownership. |
| Hardware PTI | PARTIAL | Typed priority and timer client fields exist; a configured priority is not an observed RF grant. |
| 48-event PTI lookup | IMPLEMENTED | [Model](src/model.rs) owns the reviewed table, event validation and four-bit PTI domain. This is a source lookup, not live priority scheduling. |
| Event-to-timer mapping | IMPLEMENTED | The model maps the complete admitted event domain to five timer identities or no timer. An unmapped event is not a successful request. |
| Event-duration lookup | IMPLEMENTED | The model retains five reviewed duration entries and returns no duration for other events. |
| Five-entry hardware timer bank | PARTIAL | [Register model](../../../../registers/esp32s31/model/peripherals/coex-hw-timer.toml) and typed PAC access describe configuration, targets and enable/disable controls. Concrete core-to-MMIO access is validation-only. |
| Timer clock conversion | IMPLEMENTED | [Clock model](src/clock.rs) implements the four selector domains and reviewed integer conversion rules. Production clock acquisition and radio lifetime are separate. |
| Timer programming sequence | IMPLEMENTED | [Generic program](src/timer.rs) orders client/PTI, duration and latency programming, with a fresh clock sample for each target. This is a trait-driven sequence; timer enable is separate. |
| Physical timer set/enable/disable | PARTIAL | The [HAL bridge](../hal/src/coex.rs) implements hardware access only under `validation-probes`; no production hardware owner composes it with the protocol runtimes. |
| Timer force/unforce | PARTIAL | Trait/PAC operations and validation entry points exist without a production force lifetime or arbitration policy. |
| Core request semantics | PARTIAL | [Core](src/core.rs) checks enabled state, maps the event, programs/enables its timer and records the request. This does not own RF grant notification, deadline/cancellation or a complete radio exchange. |
| Core release/disable/status | PARTIAL | Mapped timer disable and local active-state bookkeeping exist. Software status is not arbiter grant/readback state; disabling tracked timers is not whole-radio shutdown. |

The [PTI comparison profile](../../../../verification/vendor/projects/esp32s31/profiles/coex-core.toml)
and [timer-map profile](../../../../verification/vendor/projects/esp32s31/profiles/coex-core-timer.toml)
exercise compiled source mappings against reviewed vendor inputs. Their scope
is the named leaves, not the scheme scheduler or end-to-end coexistence.

## Coexistence policy and scheduler

| Feature | Status | Current source boundary |
| --- | --- | --- |
| Scheduler state | PARTIAL | [Scheduler](src/scheduler.rs) retains interval, current schedule, period, phase index and phase slice. Activating this software state does not schedule RF. |
| Phase representation | PARTIAL | `CoexPhase` retains an opaque four-byte image. Individual byte semantics are not recovered and the image has no MMIO publication operation. |
| Executable phase transitions | ABSENT | No semantic phase decoder, expiry-driven transition or slice publication owner exists. |
| Scheme selection | ABSENT | No Wi-Fi/BT/BLE state-to-schedule policy exists. |
| Wi-Fi IDLE / CONNECTED / SCAN / CONNECTING schemes | ABSENT | No production state hooks select coexistence periods or allocations. |
| TBTT-anchored periods | ABSENT | No owner binds Wi-Fi TBTT to a coexistence period. |
| Wi-Fi / BT / BLE time slices | ABSENT | No executable slice scheduler exists. |
| BLE advertising dynamic priority | ABSENT | No event-count/promotion policy composes priority changes with advertising. |
| BLE connection dynamic priority | ABSENT | No connected-runtime producer or consumer implements the vendor dynamic-priority interface. |
| IEEE 802.15.4 operation priorities | FAIL-CLOSED | [Static MAC policy](../hal/src/ieee802154/policy.rs) requires TX/RX and ACK PTI disabled. No operation-dependent coexistence lifecycle replaces that policy. |
| Mesh status-driven schemes | ABSENT | No provisioning/traffic/standby status owner selects a coexistence scheme. |
| Classic A2DP status-driven policy | ABSENT | No Classic/application status integration selects a scheme. |

`CoexClient` represents the reviewed Bluetooth/Wi-Fi request domain only.
IEEE 802.15.4 must not be added as an inferred third numeric client. The
[vendor investigation scope](../../../../verification/vendor/projects/esp32s31/vendor-project.toml)
separately names `esp_coex_ieee802154_*` operations; MAC TX/RX/ACK PTI controls
also have their own ownership boundary. Neither surface is a fourth time slice.

## Protocol integration and lifetime

| Feature | Status | Current source boundary |
| --- | --- | --- |
| Embassy mailbox | IMPLEMENTED | [Adapter](../../../adapters/embassy/esp32s31/coex/src/lib.rs) serializes Enable, Disable, WifiRequest, BluetoothRequest, Release, Status and Shutdown through one generic `CoexOwner`. This is mailbox/core composition, not live radio integration. |
| Live Wi-Fi requests | ABSENT | Production Wi-Fi runtime does not submit requests through the coexistence mailbox. |
| Live BT/BLE requests | ABSENT | Production Bluetooth runtime does not submit requests through the coexistence mailbox. |
| Wi-Fi beacon / shared RX / individual-TWT PTI hooks | PARTIAL | [Wi-Fi MAC register model](../../../../registers/esp32s31/model/peripherals/wifi-mac-coex-runtime.toml) contains protocol-specific priority, timing and request-clear controls. These do not compose a shared arbiter lifecycle. |
| IEEE 802.15.4 request/break/stage hooks | PARTIAL | MAC PTI and coexistence abort vocabulary exist; the vendor operation family has no complete production bridge. |
| Wi-Fi + BLE coexistence | ABSENT | No safe joint-radio runtime and request/grant/release lifecycle exists. |
| Wi-Fi + Classic coexistence | ABSENT | No Classic controller or joint-radio composition exists. |
| Wi-Fi + IEEE 802.15.4 coexistence | ABSENT | No complete joint-radio runtime exists; the IEEE MAC keeps PTI disabled. |
| BLE + IEEE 802.15.4 coexistence | ABSENT | No complete paired-radio arbitration runtime exists. |
| Three-protocol coexistence | ABSENT | Shared register/core infrastructure does not compose concurrent protocol owners. |
| Coexistence power management | ABSENT | No shared sleep/wake, retention, request-deadline or radio-release lifecycle exists. |
| GPIO coexistence diagnostics | ABSENT | No production board-pin and arbitration-observation interface exists. |

## External coexistence

[ESP-IDF Kconfig](https://github.com/espressif/esp-idf/blob/master/components/esp_coex/Kconfig)
documents GPIO-based one-, two- and three-wire configurations. The public
[external coexistence API](https://github.com/espressif/esp-idf/blob/master/components/esp_coex/include/esp_coexist.h)
also defines signals and control options. An enum value alone does not establish
S31 support for an additional wire mode; four-wire operation is not claimed here.

| Feature | Status | Current source boundary |
| --- | --- | --- |
| Advanced external arbitration hardware | PARTIAL | [Reviewed external enable/disable evidence](../../../../registers/esp32s31/evidence/vendor-radio-libraries.toml) records vendor transactions in high MODEM_LPCON. Those controls are outside the radio-owned model; no platform-owned production bridge publishes them. |
| One-wire / two-wire / three-wire modes | ABSENT | No board wiring contract or mode activation/teardown owner exists. |
| Leader / Follower | ABSENT | No external arbitration role lifecycle exists. |
| Request / priority / grant / TX-line GPIOs | ABSENT | No production pin assignment, signal routing or arbitration handoff exists. |
| Grant delay / polarity | ABSENT | No production configuration and readback lifecycle exists. |
| External PTI Mid / High | ABSENT | No external-priority configuration and policy owner exists. |

## Official coexistence scenario scope

The [vendor scenario matrix](https://docs.espressif.com/projects/esp-idf/en/latest/esp32s31/api-guides/coexist.html#supported-coexistence-scenario-for-esp32-s31)
is an external compatibility reference, not open-driver evidence. Selected
entries retain its distinctions: Y means stable support, C1 unstable support,
X unsupported and S stable only in STA mode. All corresponding open production
compositions remain ABSENT as listed above.

| Vendor scenario | Vendor classification |
| --- | --- |
| Wi-Fi STA scan/connecting/connected + BLE scan/advertising/connected | Y |
| SoftAP connecting/connected or sniffer RX + BLE | C1 |
| ESP-NOW RX + BLE | S |
| Wi-Fi STA + Classic inquiry/page/connected roles | Y |
| Wi-Fi STA + Thread/Zigbee Router | C1 |
| SoftAP or sniffer RX + Thread/Zigbee Router | X |
| Thread/Zigbee Router + BLE scan | X |
| Thread/Zigbee Router + BLE advertising/connected | Y |

## Readiness authority

There is no standalone coex qualification target. Existing requirements live in
[Bluetooth qualification](../../../../qualification/targets/esp32s31/bluetooth-le.toml)
and the radio compositions described by the [Wi-Fi](../ieee80211/FEATURES.md)
and [IEEE 802.15.4](../ieee802154/FEATURES.md) inventories. The Bluetooth
`coexistence` capability remains incomplete. Recovered tables and validation
MMIO cannot satisfy missing protocol hooks, scheduler semantics or joint-radio
HIL evidence. This inventory does not introduce qualification scenarios or
promote readiness.
