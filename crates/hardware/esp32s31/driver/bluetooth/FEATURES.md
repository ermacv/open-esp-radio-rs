# ESP32-S31 Bluetooth source capabilities

See the [whole-radio capability map](../FEATURES.md) for shared ownership,
clock/power lifecycle and cross-protocol composition limits.

This inventory describes source-owned Bluetooth paths and their composition
limits. It does not claim RF delivery, interoperable connectivity or Bluetooth
qualification. Status applies only to the exact scope of each row.

| Status | Meaning |
| --- | --- |
| IMPLEMENTED | A bounded production source path owns the complete operation in the stated scope; this is not a hardware qualification result. |
| PARTIAL | A source subset exists, including narrower role, PHY, DTM or lower-layer machinery, but the full named feature is not composed. |
| FAIL-CLOSED | Production deliberately prevents activation or advertisement at the stated boundary. Recovered registers alone do not establish this status. |
| ABSENT | No production protocol/runtime owner exists for the operation. |
| HOST-ONLY | The named Host/profile function is above HCI. This is a scope label, not a claim that a Host integration implements it. |

The [qualification specification](../../../../../qualification/targets/esp32s31/bluetooth-le.toml)
owns readiness requirements and hardware evidence independently. A successful
HCI command, scheduler `RUN`, SRAM graph, PAC accessor, parser or isolated state
machine does not establish an end-to-end Controller capability. In particular,
DTM PHY coverage does not establish connected PHY support.

Standalone cold start prepares common PHY power, resets and calibration clocks
with semantic readback before invoking borrowed registration. The task retains
the PHY I2C clock lease throughout the powered epoch. Power-readback and
calibration failures retain fail-stop ownership; cold reunion requires physical
teardown, which remains incomplete. See the
[PHY consumer boundary](../../phy/FEATURES.md#protocol-consumer-composition).

## Capability sources and scope

| Source | Inventory scope |
| --- | --- |
| [ESP32-S31 datasheet, pre-release v0.5](https://www.espressif.com/sites/default/files/documentation/esp32-s31_datasheet_en.pdf), sections 4.3.3 and 4.3.4 | LE PHY, Controller/Host features and Classic BR/EDR capabilities |
| [ESP-IDF S31 SoC capabilities](https://github.com/espressif/esp-idf/blob/master/components/soc/esp32s31/include/soc/soc_caps.h) | Classic, LE, ISO/Audio, privacy, CTE, power control, subrating, PAwR and coexistence declarations |
| [ESP-IDF S31 Bluetooth documentation](https://docs.espressif.com/projects/esp-idf/en/latest/esp32s31/api-reference/bluetooth/index.html) | Vendor Controller/Host integration boundary |
| [Chip owners](README.md), [portable Bluetooth](../../../../protocols/bluetooth/) and [Embassy runtime](../../../../runtime/embassy/esp32s31/bluetooth/) | Actual open-driver production composition |

Vendor declarations identify capabilities to account for; they do not establish
open-driver support. The datasheet combines Controller and Host features, so
Host functions are listed separately below. Features beyond this hardware
inventory, such as Monitoring Advertisers, must not be inferred from passive
scanning or a Bluetooth version number.

## Qualification scope mapping

The [LE qualification program](../../../../../qualification/targets/esp32s31/bluetooth-le.toml)
uses broader Controller roots as well as narrow portable capabilities. These
scopes must agree with the rows below; missing RF evidence alone does not make
an implemented source operation incomplete.

| Feature scope | Qualification capability / source contract | Boundary |
| --- | --- | --- |
| Initial PHY registration, acquisition and tracking | `common-phy-baseband` / `bluetooth-initial-phy-handoff` | Common-PHY power/clock prerequisites and borrowed handoff are implemented; periodic tracking and physical release remain incomplete. |
| Bounded DTM sessions | `packet-dataplane` / `bounded-dtm-session`; `dtm-*` roots | DTM source composition exists; the broad dataplane still lacks unrelated-list dispatch and the full Controller lifetime. |
| HCI bootstrap and command/event handoff | `hci-bootstrap`, `hci-handoff-storage`, `hci-controller-endpoints` | Complete bounded interfaces do not establish `typed-hci-controller`, which also requires LL and ACL routing. |
| Portable advertising / peripheral admission | `portable-legacy-advertising`, `portable-connectable-advertising`, `portable-peripheral-connection` | Portable readiness has no RF/HIL obligation and does not qualify a hardware advertising or ACL role. |
| Passive scanning / complete Link Layer | `legacy-passive-scanning`, `le-link-layer` | Partial hardware roles remain incomplete at the broader runtime/reliable-link scope. |
| Coexistence | `coexistence` / `coex-timer-validation-bridge`, `wifi-bluetooth-coex-runtime` | Diagnostic timer access exists; production joint-radio arbitration is unimplemented. |
| Powered shutdown | `powered-teardown` | Selected cleanup paths do not close the complete last-owner lifetime. |
| Resource limits | `controller-capacity-limits` | Storage geometry does not establish supported concurrent connections, sets, syncs or streams. |
| Complete legacy roles | `legacy-advertising-roles`, `active-scanning`, `central-initiator`, `concurrent-le-roles` | Bounded advertising and passive scanning do not close Central or simultaneous roles. |
| Connected PHY and control | `connected-phy-and-data-length`, `ll-control-procedures`, `channel-assessment` | DTM PHY selectors and initial channel maps do not establish negotiated PHY, DLE, LLCP or live assessment. |
| Encryption, privacy and pairing | `link-layer-encryption`, `privacy-and-filtering`, `secure-connections-host` | Hardware encryption and resolving-list storage do not establish operational keys, addresses or pairing. |
| Extended advertising, scanning and initiating | `extended-advertising`, `extended-scanning-initiating` | AUX chains, multiple sets, secondary PHY and coding selection have no production role. |
| Periodic advertising and synchronization | `periodic-advertising`, `periodic-synchronization`, `periodic-sync-transfer` | Includes ADI, periodic enhancements and PAST; reserved storage is not a sync lifecycle. |
| PAwR | `pawr-advertiser`, `pawr-responder` | Advertiser response reception and synchronized response transmission require distinct owners. |
| Encrypted Advertising Data | `encrypted-advertising-data` | Host data authentication and key distribution are separate from LL encryption. |
| Direction Finding and CTE tests | `connected-direction-finding`, `connectionless-direction-finding` | The current CTE publication remains disabled. |
| RF power, LBT and connected power policy | `rf-power-and-lbt`, `le-power-control`, `connection-subrating` | Default power encoding does not supply live RF policy or connected procedures. |
| Sleep and external/multi-radio coexistence | `modem-sleep-wake`, `external-coexistence`, `multi-radio-coordination` | Always-awake operation and shared primitives do not establish retained wake or concurrent ownership. |
| ISO adaptation and connected streams | `iso-dataplane`, `cis-central`, `cis-peripheral` | ISO framing alone does not route SDUs or establish CIS in either role. |
| Broadcast isochronous streams | `bis-broadcaster`, `bis-synchronizer` | BIG/BIS publication and synchronization require periodic advertising and the ISO dataplane. |
| LE Audio | `le-audio-unicast`, `le-audio-broadcast`, `le-audio-profiles` | Host codec/profile composition depends on operational CIS/BIS transport. |
| Other LE Host features | `host-eatt-and-gatt`, `mesh-host`, `blufi-host` | EATT, GATT caching/security levels, Mesh 1.1 and BluFi require their own production Host integrations. |

The LE program includes the full LE inventory and its named Host integrations.
Its acceptance requirements do not promote source capability status. Classic
remains outside the LE program.

## LE PHY, RF and Direct Test Mode

| Feature | Status | Current production boundary |
| --- | --- | --- |
| LE 1M PHY | IMPLEMENTED | DTM and bounded legacy advertising/scanning paths own 1M event publication. This does not imply a complete connected Controller. |
| LE 2M PHY | PARTIAL | [DTM parameters](src/le/dtm/parameters.rs) own TX/RX selectors and scheduler geometry. Ordinary advertising/scanning and peripheral roles do not select 2M. |
| LE Coded S=8 / 125 kbit/s | PARTIAL | Enhanced DTM owns the S=8 TX selection and generic Coded RX selection. No ordinary advertising/scanning or ACL role selects Coded PHY. |
| LE Coded S=2 / 500 kbit/s | PARTIAL | DTM has a distinct S=2 TX identity. RX uses the generic Coded selector; S=2 is not a separate accepted RX selector. No connected Coded path exists. |
| Hardware Listen Before Talk (LBT) | ABSENT | No Bluetooth LBT policy/activation owner is composed. |
| Static/default TX power selection | PARTIAL | DTM, advertising, scanning and peripheral graph preparation carry default power requests through the recovered [power encoding](memory/src/le_tx_power.rs). No general live power-selection API or radiated-power qualification is implied. |
| LE Receiver Test / Transmitter Test | PARTIAL | [DTM](src/le/dtm.rs) commands, SRAM graphs, recurring events and RX accounting are composed with Embassy. Test End/Reset share finite common-scheduler cancellation with an absolute deadline, explicit aborted-item ownership and unlink/recycle. Quiet stop/restart hardware qualification remains pending. |
| Enhanced DTM PHY selection | IMPLEMENTED | HCI Receiver/Transmitter Test v2 select the bounded 1M/2M/Coded domains; S=2 selection is TX-only. This does not cover later CTE test-command versions. |
| DTM test patterns | IMPLEMENTED | [TX payload preparation](src/le/dtm/payload.rs) owns the retained HCI test-pattern variants. |
| Long-running RF/PHY maintenance, including DTM | PARTIAL | Initial common-PHY/client and baseband acquisition exist. Periodic tracking, timer-expiration handling and final PHY release remain incomplete. RF/HIL readiness belongs to qualification. |

## Legacy advertising and scanning

| Feature | Status | Current production boundary |
| --- | --- | --- |
| Legacy non-connectable advertising (`ADV_NONCONN_IND`) | IMPLEMENTED | A bounded event owns one to three primary-channel descriptors, scheduler publication, completion, retirement and recycle. |
| Legacy connectable advertising (`ADV_IND`) | PARTIAL | [Connectable advertising](src/le/advertising/connectable.rs) owns configuration, response graph, recurrence and `CONNECT_IND` handoff. The S31 slice accepts exactly one selected primary channel. |
| Advertiser Scan Response (`SCAN_RSP`) | PARTIAL | Bounded response state and graph preparation belong to connectable advertising; this is not a general scannable advertiser. |
| Scannable non-connectable advertising (`ADV_SCAN_IND`) | ABSENT | No production role publishes this advertising mode. |
| Low-duty / high-duty directed advertising (`ADV_DIRECT_IND`) | ABSENT | No directed-advertising scheduler and lifecycle exists. |
| High-duty-cycle non-connectable advertising | ABSENT | No production scheduling policy implements the named feature. |
| Static random advertiser address | PARTIAL | HCI retains a requested random address; application to advertising is role-specific. This does not provide private-address rotation. |
| Legacy passive scanning | PARTIAL | [Passive scanning](src/le/scanning/passive.rs) composes recurring primary-channel LE 1M windows, public own address, accept-all policy, bounded reports and optional exact report-duplicate suppression. Privacy is disabled; unrelated finished-list dispatch remains incomplete. |
| Active scanning | ABSENT | No `SCAN_REQ` producer and complete request/response scanning lifecycle exists. |
| Extended / secondary-channel scanning | ABSENT | No AUX scanner, secondary 2M scanner or ordinary Coded scanner is composed. DTM RX is separate. |
| Extended scanner filter policies | ABSENT | Production uses the restricted accept-all policy. |
| Filter Accept List operation | ABSENT | HCI capacity reporting exists, but list commands and advertiser/scanner admission policy are not composed. |

## LE roles, connections and reliability

| Feature | Status | Current production boundary |
| --- | --- | --- |
| Broadcaster / Observer | PARTIAL | The bounded legacy advertising and passive scanning subsets above exist. Extended/periodic roles and general multi-set operation are absent. |
| Peripheral | PARTIAL | `CONNECT_IND` handoff, first-event RUN and active completion/recurrence are composed. Central-initiated feature exchange has a bounded control-response path; reliable ACL, active commands and general LLCP remain unsupported. |
| Central / Initiator | ABSENT | No Create Connection/initiator scheduling or Central connection owner exists. |
| Simultaneous Broadcaster + Observer + Central + Peripheral | ABSENT | Individual role subsets do not form a simultaneous multi-role runtime. |
| Multiple connections / multi-connection optimization | ABSENT | No multi-handle connection scheduler, ACL ownership or connection-table lifecycle exists. |
| `CONNECT_IND` decoding and admission | IMPLEMENTED | [Portable connection policy](../../../../protocols/bluetooth/le/ll/src/connection.rs) validates addresses, Access Address, timing, channel map, hop and other request fields; advertiser admission checks the local address and allowed channel-selection algorithm. This row covers admission only. |
| First peripheral connection window | PARTIAL | Causal timestamp projection, window planning, SRAM preparation and first hardware `RUN` exist; a successful connection exchange is not established. |
| Recurring peripheral events | PARTIAL | [Active lifecycle](src/le/peripheral/active.rs) drives completion, recycle, fresh controller time and contiguous successor RUN through lower recurrence. Requires an explicit local clock accuracy bound; no missed-anchor recovery or hardware timing qualification. |
| Channel Selection Algorithm #1 | PARTIAL | Portable channel progression exists within the incomplete connection owner. |
| Channel Selection Algorithm #2 | PARTIAL | Portable CSA#2 exists, but S31 connectable advertising marks local CSA#2 support unsupported and rejects requests requiring it. |
| Peripheral latency | PARTIAL | Timing is validated and retained; live skip/recovery scheduling is not complete. |
| Sleep Clock Accuracy / window widening | PARTIAL | Portable SCA interpretation and bounded widening exist. A local accuracy bound is caller-owned; arbitrary missed-event recovery is not established. |
| Establishment / supervision timeout | ABSENT | Request timing validation is present, but no complete live timeout and disconnect owner exists. |
| Missed-event recovery | ABSENT | No complete connection resynchronization and accumulated-uncertainty policy exists. |
| LL Data PDU RX | PARTIAL | Lower peripheral RX extraction and buffer recycling exist without a reliable recurring ACL link. |
| LL Data PDU TX | PARTIAL | One controller control-PDU payload is retained through publication and descriptor completion; its cursor survives reclamation. ACL TX and general queueing are absent. |
| SN/NESN, retransmission, duplicate suppression and empty-PDU acknowledgments | PARTIAL | The bounded control path preserves hardware sequence state and uncompleted TX packets; connection RX applies the reviewed acceptance gate. General ACL reliability is not qualified. |
| LLCP framework | PARTIAL | A bounded peripheral responder queues feature and unknown responses. Procedure timers, general transaction collisions and mandatory live updates are absent. |
| Connection Update / Channel Map Update | ABSENT | Initial parameters are retained; live negotiation, instant handling and application are absent. |
| PHY Update | ABSENT | No connected PHY negotiation/application owner exists; DTM selection does not implement LLCP. |
| Data Length Extension / Data Length Update | ABSENT | No negotiated connected packet-length owner exists. |
| Feature Exchange | PARTIAL | Central `LL_FEATURE_REQ` produces a queued `LL_FEATURE_RSP` with zero optional feature bits. Peripheral-initiated exchange and remote-feature HCI routing are absent. |
| Version Exchange | PARTIAL | A peer request receives at most one queued reply using the configured Controller identity; no identity is inferred from the chip. Host-initiated version routing is absent. |
| LE Ping | ABSENT | No ping procedure owner exists. |
| Termination | ABSENT | No on-air termination transaction plus HCI disconnection lifecycle exists. |
| Adaptive Frequency Hopping / channel assessment | PARTIAL | Initial channel maps and CSA progression exist. Dynamic assessment and map updates are absent. |
| LE Channel Classification | ABSENT | No connected classification generation/report/update procedure exists. |

## Security and privacy

| Feature | Status | Current production boundary |
| --- | --- | --- |
| BLE encryption accelerator surface | PARTIAL | [Reviewed hardware registers](../../../../../registers/esp32s31/model/peripherals/ble-hw-accelerator.toml) describe encryption operations; no connected encryption owner drives them. |
| Link Layer Encryption | ABSENT | No key installation, packet counters/nonces, start/pause LLCP or encrypted ACL lifecycle exists. Its optional HCI feature bit stays clear. |
| LE Privacy 1.2 | FAIL-CLOSED | The production [scanner image](memory/src/passive_scanning_event_image.rs) fixes privacy disabled. Resolving-list hardware does not provide an RPA/IRK lifecycle. |
| Resolving List | PARTIAL | [PHY engine storage](memory/src/ble_phy_engine.rs) owns allocation and base publication. HCI list commands, IRKs and live resolution policy are not composed. |
| LE Secure Connections integration | ABSENT | No SMP Host integration is connected to a working ACL/encryption Controller path. Generic cryptographic hardware is not pairing support. |
| Encrypted Advertising Data | ABSENT | No key/material lifecycle and advertiser/scanner formatter or consumer exists. |

## Extended advertising, periodic advertising and Direction Finding

| Feature | Status | Current production boundary |
| --- | --- | --- |
| Advertising Extensions / multiple advertising sets | ABSENT | No `ADV_EXT_IND` or `AUX_ADV_IND` production role or set lifecycle exists. Allocation counts do not create roles. |
| Secondary-channel advertising, 2M / Coded advertising and Advertising Coding Selection | ABSENT | No AUX scheduling and PHY-selection owner exists. DTM coverage does not extend to advertising. |
| Periodic Advertising / synchronization | ABSENT | No periodic advertiser or sync role exists; reserved sync capacity is only geometry. |
| Periodic Advertising Sync Transfer (PAST) | ABSENT | Neither the ACL procedure nor periodic-sync owner is composed. |
| Periodic Advertising with Responses (PAwR) | ABSENT | No subevent/response-slot scheduling or advertiser/scanner runtime exists. |
| AdvDataInfo / Periodic Advertising Enhancements | ABSENT | No production periodic-advertising foundation exists. |
| Advertising Channel Index | ABSENT | No production owner implements the named feature. |
| Constant Tone Extension activation | FAIL-CLOSED | Normal initialization publishes a [disabled CTE workspace](memory/src/direction_finding_workspace.rs). Recovered CTE controls do not activate Direction Finding. |
| AoA / AoD | ABSENT | No antenna pattern, IQ sampling, CTE TX/RX protocol or report lifecycle exists; the CTE activation boundary remains disabled. |
| Connected / connectionless CTE procedures | ABSENT | Neither connected LLCP nor periodic-advertising CTE ownership exists. |
| IQ sample delivery over HCI | ABSENT | No sampling-to-HCI producer or reporting lifecycle exists. |

## Power control, subrating and isochronous transport

| Feature | Status | Current production boundary |
| --- | --- | --- |
| LE Power Control / Path Loss Monitoring | ABSENT | No connected procedure, threshold state or HCI event producer exists. Default power encoding is separate. |
| LE Connection Subrating | ABSENT | No LLCP, subrate anchor or scheduling lifecycle exists. |
| LE Isochronous Channels / ISOAL | ABSENT | No ISO adaptation, stream scheduling or Controller data-path owner exists. |
| Connected Isochronous Groups / Streams (CIG/CIS) | ABSENT | No connected group/stream control and scheduling lifecycle exists. |
| Broadcast Isochronous Groups / Streams (BIG/BIS) | ABSENT | No BIG control, periodic-advertising integration or broadcast ISO scheduler exists. |
| LE Audio transport | ABSENT | CIS/BIS transport is not implemented. Host audio profiles cannot supply the missing Controller path. |

## HCI and Host boundary

| Feature | Status | Current production boundary |
| --- | --- | --- |
| HCI Command / Event packets | IMPLEMENTED | Bounded in-process transport and typed dispatch own the current bootstrap, DTM and legacy advertising/scanning command/response subset. |
| ACL / SCO / ISO packet framing | IMPLEMENTED | [Packet validation](../../../../protocols/bluetooth/hci/src/transport/packet.rs) recognizes standard packet kinds and declared lengths. This is framing only, not an operational data plane. |
| HCI ACL routing and bidirectional flow control | ABSENT | No connected handle, radio queue or credit lifecycle exists. Bootstrap retains Host buffer/flow-control policy; non-command input is quarantined by the Controller composition. |
| HCI SCO / ISO data plane | ABSENT | Generic transport packet representations exist, but no synchronous/isochronous stream routes them to radio execution. |
| HCI Reset | IMPLEMENTED | Software bootstrap reset and selected active-role reset coordination exist. Complete powered reconstruction remains separate. |
| Event masks | PARTIAL | Bootstrap retains Host masks; complete event production is not composed. |
| Public BD_ADDR / LE Read Buffer Size | IMPLEMENTED | Bootstrap reports the configured address and bounded buffer information; buffer reporting does not establish ACL support. |
| LE Set Random Address | PARTIAL | Bootstrap retains the request; hardware application is role-specific and does not establish privacy. |
| LE Read Local Supported Features command | IMPLEMENTED | [Bootstrap](../../../../protocols/bluetooth/hci/src/controller/bootstrap/state.rs) returns eight zero feature bytes. The query itself is implemented. |
| Optional LE capability advertisement | FAIL-CLOSED | All optional LE feature bits remain clear until the corresponding production path is complete. DTM-only PHY support does not change this advertisement. |
| LE Read Filter Accept List Size | IMPLEMENTED | Configured capacity is reported; list operation remains absent. |
| Connection handles / Connection Complete / Disconnection Complete | ABSENT | No complete Host-visible connection lifecycle exists. |
| Trouble Host integration | PARTIAL | [Portable HCI tests](../../../../protocols/bluetooth/hci/Cargo.toml) use `bt-hci` 0.10.1 and development-only `trouble-host` 0.8.0 for bootstrap. No production Trouble runner or connected GATT interoperability composition exists. |
## Controller scheduling, power and lifetime

| Feature | Status | Current production boundary |
| --- | --- | --- |
| Controller scheduler timebase | PARTIAL | Always-awake event-driven time ownership uses two raw ticks per microsecond and retains fractional ticks across epoch updates. Effective counter lifetime, timer-expiration handling, wake behavior and periodic tracking remain incomplete. |
| Controller interrupt epoch | PARTIAL | IRQ routing, bounded hard-handler classification and task wake exist; unrelated finished-list and complete role dispatch remain incomplete. |
| Controller SRAM ownership | PARTIAL | Static DTM, advertising, scanning and peripheral graphs have CPU/hardware handoff contracts. Missing role graphs and packet-engine contracts are not implied. |
| Always-awake Controller | PARTIAL | Production selects standalone always-awake operation; incomplete role and maintenance lifetimes still apply. |
| Bluetooth modem sleep / wake request | ABSENT | No controller sleep scheduler, RF/PHY/baseband stop/wake, retention or wake-request transaction is composed. |
| Low-frequency sleep-clock operation | ABSENT | Production uses the standalone main-XTAL time profile. Portable SCA types do not implement a low-power clock/wake lifecycle. |
| Powered shutdown | PARTIAL | Pre-publication rollback and selected HCI Reset paths exist. Complete hardware quiescence, PHY/client release and cold reconstruction remain incomplete. |
## Coexistence

| Feature | Status | Current production boundary |
| --- | --- | --- |
| Internal Wi-Fi/Bluetooth coexistence | PARTIAL | The [coexistence core](../coex/src/lib.rs), PAC and [Embassy mailbox](../../../../adapters/embassy/esp32s31/coex/src/lib.rs) exist. Safe concurrent Wi-Fi/Bluetooth singleton ownership is not composed. |
| Wi-Fi/Bluetooth/IEEE 802.15.4 radio coordination | PARTIAL | Shared arbitration infrastructure exists without a complete multi-radio production lifecycle. |
| Hardware PTI coexistence | PARTIAL | Recovered priority/register machinery exists without a complete Bluetooth request/grant/release policy. |
| External coexistence interface | ABSENT | The SoC declares advanced external coexistence hardware. No Bluetooth board-pin contract and external request/grant runtime is composed. |

## Bluetooth Classic hardware scope

The datasheet and `SOC_BT_CLASSIC_SUPPORTED` declare BR/EDR hardware.
The open Controller implementation is LE-only; vendor Host availability does
not determine whether Classic belongs in the silicon inventory. Shared RF,
SRAM or HCI packet types do not implement a Classic protocol/runtime owner.

## Bluetooth Classic PHY

| Feature | Status | Current production boundary |
| --- | --- | --- |
| Basic Rate GFSK, 1 Mbit/s | ABSENT | No BR/EDR PHY-event or Baseband role exists. LE DTM does not exercise Classic. |
| EDR pi/4-DQPSK, 2 Mbit/s | ABSENT | No EDR formatter/receiver owner exists. |
| EDR 8DPSK, 3 Mbit/s | ABSENT | No EDR formatter/receiver owner exists. |
| Hardware CCA | ABSENT | No Classic PHY/runtime owner drives channel assessment. |
| Power Class 1 operation | ABSENT | No Classic TX power/runtime path establishes the advertised radio capability. |

## Bluetooth Classic Link Controller

| Feature | Status | Current production boundary |
| --- | --- | --- |
| Inquiry / Inquiry Scan | ABSENT | No Classic discovery role exists. |
| Paging / Page Scan | ABSENT | No Classic connection-establishment owner exists. |
| ACL links | ABSENT | No BR/EDR ACL connection or operational HCI data path exists. |
| SCO links | ABSENT | No synchronous Classic link owner exists. |
| eSCO links | ABSENT | No enhanced synchronous Classic link owner exists. |
| Active mode | ABSENT | No Classic connected state exists. |
| Sniff mode | ABSENT | No Classic low-duty connected scheduler exists. |
| Sniff Subrating | ABSENT | No Classic subrating procedure exists. |
| Role switching | ABSENT | No Classic piconet role manager exists. |
| Channel classification | ABSENT | No Classic channel assessment/classification owner exists. |
| Adaptive Frequency Hopping | ABSENT | No Classic hopping and channel-map lifecycle exists. |
| Traditional Power Control | ABSENT | No Classic power-control procedure exists. |
| Enhanced Power Control | ABSENT | No Classic enhanced power-control procedure exists. |
| Ping | ABSENT | No Classic Link Manager transaction owner exists. |
| Piconet management | ABSENT | No Classic piconet scheduler/link manager exists. |
| Scatternet management | ABSENT | No multi-piconet scheduler exists. |
| Multiple connections | ABSENT | No Classic link table or concurrent connection scheduler exists. |
| Active Peripheral Broadcast | ABSENT | No Classic broadcast role exists. |

## Bluetooth Classic security and audio

| Feature | Status | Current production boundary |
| --- | --- | --- |
| Secure Simple Pairing (SSP) | ABSENT | No Classic Host/Link Manager pairing lifecycle exists. |
| Secure Connections | ABSENT | No Classic connection/security owner exists. |
| E0 encryption | ABSENT | No Classic encryption procedure and packet lifecycle exists. |
| AES-CCM encryption | ABSENT | Generic crypto hardware does not establish a Classic encrypted link. |
| A-law voice | ABSENT | No SCO/eSCO audio path exists. |
| mu-law voice | ABSENT | No SCO/eSCO audio path exists. |
| CVSD voice | ABSENT | No SCO/eSCO audio path exists. |
| Transparent voice data | ABSENT | No synchronous Classic audio path exists. |
| Voice over HCI | ABSENT | Packet framing exists, but no synchronous link routes voice over HCI. |
| Voice over PCM/I2S | ABSENT | The SoC declares Bluetooth/I2S routing; no Classic synchronous-link composition owns it. |

## Host-only product capabilities

| Feature | Status | Current production boundary |
| --- | --- | --- |
| Bluetooth Mesh 1.1 | HOST-ONLY | Requires a Host/profile stack over an adequate LE Controller; no production Mesh integration is claimed. |
| Enhanced Attribute Protocol (EATT) | HOST-ONLY | ATT/L2CAP Host function; no production Host integration is claimed. |
| GATT Caching | HOST-ONLY | GATT Host function; no production Host integration is claimed. |
| LE GATT Security Levels Characteristic | HOST-ONLY | GATT/security Host function; no production Host integration is claimed. |
| BluFi | HOST-ONLY | Espressif application/profile protocol requiring LE and Wi-Fi integration; not supplied by this Controller. |
| LE Audio Generic Audio Framework | HOST-ONLY | Profiles/services are Host-side; the missing ISO/CIS/BIS Controller transport is tracked separately above. |
| Classic audio/data profiles | HOST-ONLY | A2DP/HFP and other Host profiles do not establish BR/EDR Controller support. |

## Capability advertisement

`LE Read Local Supported Features` currently returns eight zero feature bytes.
Each optional bit requires the complete production Controller operation named
by that feature. A recovered register, SRAM layout, portable parser, DTM-only
PHY or vendor capability cannot grant advertisement. The command itself is
implemented; its conservative result is not a claim of a complete Controller.

## Ownership

| Owner | Authority |
| --- | --- |
| PAC | MMIO representation and restricted access |
| HAL | Semantic accessors, register transactions and hardware ownership proofs |
| [Memory crate](memory/) | Controller-SRAM layouts, private codecs and DMA-visible storage |
| Portable LE LL | Protocol policy and advertising generation/event identity |
| Chip roles | Scheduler timing, admission, publication and role-specific RX/recycle |
| Embassy runtime | Waits, command/response fairness and durable task state |
| Integration | Static resources, platform claims and interrupt routes |

`single_item_completion` is the shared lower completion engine for advertising,
scanning and peripheral roles. `controller_start/timed_preparation` owns shared
time requests, rechecks, rollback and orphan draining. Sharing those mechanisms
does not compose a missing caller or transfer protocol policy between roles.

## Peripheral timing limits

The [active controller branch](../../../../runtime/embassy/esp32s31/bluetooth/src/controller/dispatch.rs)
drives completion and recycle before preparing a contiguous successor through
fresh controller time, validation and scheduler publication. Each
`PeripheralConnectionActive` boundary represents a first or successor `RUN`.
Response backpressure does not block radio readiness; cancellation retains
both the radio phase and HCI authority. Completion or preparation failures
seal both owners in `PeripheralConnectionActiveFailStop`.

`PeripheralConnectionRuntimeConfig::with_software_recurring_timing(ppm)` must
supply the local clock accuracy bound. The default configuration leaves it
unset and stops with `TimingPolicyUnavailable` when recurrence is attempted.
The actor responds to central feature requests through a bounded TX queue.
It does not consume active HCI commands, deliver ACL data, implement general
LLCP, or enforce supervision and missed-anchor recovery. A compiled lifecycle does not establish a successful over-air link.

Lower recurrence requires a caller-owned local clock accuracy bound. Its
software window widening does not establish arbitrary missed-event anchor
recovery or accumulated timing uncertainty. The
[memory codec](memory/src/peripheral_connection_memory/codec.rs) stores
link-state event span and scheduler captured anchor in distinct objects;
equal offsets within those objects do not make them the same field. A raw
captured anchor is not a normalized packet-start timestamp. Hardware timing
qualification must cover that interpretation independently of the ownership
and layout types.
