# Driver architecture

`driver/` contains the source-owned radio implementation, its protocol code
and production integrations. Applications supply board identity, credentials
and peripherals. HIL scenarios, traffic generators, UART test protocols,
vendor artifacts and qualification policy live outside this tree. See the
[source policy](../docs/SOURCE_POLICY.md).

## Source map

Paths describe responsibilities; a directory need not be a Cargo crate.
Existing package names remain independent of this directory hierarchy.
The [complete current tree](../docs/driver-audit/current-tree.txt) also lists
the child test files and internal module directories.

| Path | Responsibility |
| --- | --- |
| `radio/` | `wifi/` owns public requests and affine role lifecycle; `runtime/embassy` drives local control epochs |
| `common/dma/` | Audited stable-memory proofs and affine buffer/queue handoff |
| `common/network/` | Stack-neutral interface, link and error values |
| `ieee80211/{mac,softmac,sta,ap,security/wpa2}/` | Frame/protocol code, MAC contracts, role policy and security |
| `ieee80211/datapath/` | Software egress ownership, flow demand and physical materialization contracts |
| `bluetooth/le/ll/` | Portable LE PDU codecs and protocol-role state |
| `bluetooth/hci/` | Host/Controller transport and resources; `command/` owns codecs, classification and response ordering |
| `ieee802154/` | Portable frames, metadata and finite command/event contracts |
| `chips/esp32s31/{pac,hal,phy}/` | PAC `ownership` and HAL `owner` retain hardware authority; domain modules hold register operations, transactions and RF algorithms |
| `chips/esp32s31/ieee80211/{dma,mac,sta,ap}/` | S31 descriptor ownership, MAC `rx/tx/rate`, and chip role composition; `mac/tx/metadata` lowers portable traffic intent |
| `chips/esp32s31/{bluetooth,coex,ieee802154}/` | Chip radio actors; Bluetooth `memory/` and IEEE 802.15.4 `{dma,irq,mac,runtime}/` hold their lower ownership boundaries |
| `adapters/esp-hal/esp32s31/{soc,radio,ieee80211,ieee802154}/` | Upstream SoC access, singleton acquisition and concrete hardware bindings |
| `adapters/embassy/ieee80211/` | Generic Embassy Wi-Fi service contracts |
| `adapters/embassy/esp32s31/` | Concrete runtimes, wakeups and timers; Bluetooth separates `controller` and role `session` modules; `runtime/` holds platform ABI and placement |
| `adapters/network/embassy/{owned,compat}/` | Owned-packet and released-interface network adapters |
| `adapters/network/research/` | Experimental synchronous network engine; currently consumed only by a driver test |
| `integration/esp32s31/embassy/{ieee80211,bluetooth}/` | Static resources, one-time claims, final bindings and the concrete whole-radio lifecycle runners |

The [structure audit](../docs/DRIVER_STRUCTURE_AUDIT.md) records the original
mixed responsibilities. The [work plan](../docs/DRIVER_STRUCTURE_PLAN.md)
tracks completed namespace changes, moves across ownership boundaries and
the remaining separate lifecycle work.
The [boundary review](../docs/DRIVER_STRUCTURE_PLAN.md#уточнение-границ-adapters-common-и-integration)
records remaining mixed responsibilities: adapter code still selects production
resource profiles, and integration executes whole-radio lifecycles. Folder names
alone do not enforce these boundaries.
The [protocol naming convention](../docs/DRIVER_PROTOCOL_NAMING.md) defines
`ieee80211`, `ieee802154` and `bluetooth` as technical family namespaces.
Wi-Fi remains the application-facing technology name; portable LE Link Layer
code lives in `bluetooth/le/ll`, beside the family-wide HCI boundary.

Portable IEEE 802.11 encoders accept explicit local capabilities. Chip STA/AP
`profile` modules select the existing advertisements; STA profile also lowers
channel selection to S31 encodings. Authentication and association retry epochs
belong to `ieee80211/sta/join`, while `ieee80211/ap/limits` owns the fixed software peer
budget. Chip AP composition checks that hardware key slots cover that budget.
WPA2 retains secret bytes and zeroization; register-word conversion stays in
the PAC and does not require another key copy.
Chip STA `connected/security` retains the association-scoped supplicant, GTK
and replay owners together, including replacement, rollback and quarantine.
Embassy delivers classified EAPOL and drives the existing TX completion port.
The chip `startup` module performs common PHY/MAC initialization through an
abstract delay; integration chooses the concrete bindings and resource profile.
Its supervisor separates shared `physical` owners and `role_transition`
frontiers from AP execution and observation.

The chip Wi-Fi `rx/frontier` owns finite physical-ring transitions and borrows
an abstract delay. Its `rx/transaction` synchronously borrows the live ring,
storage and staging pool for one bounded completion/recycle pass. Publication
accepts the unique frame or returns that same lease on rejection. Embassy
retains queue endpoints, VIF routing, clocks, observation bindings and the
stopped/prepared/live composition owners. No chip dependency points back to an
adapter. Integration `interrupts` owns the ISR bindings and routes both
handlers and role epochs through the same static interrupt resources. See the
[RX ownership contract](adapters/embassy/esp32s31/ieee80211/src/datapath/rx/README.md).

## Register and execution boundaries

The radio-register direction is reviewed SVD → `pac/raw` accessors → semantic
`pac` → `hal` → chip driver. The raw package also contains handwritten trusted
sidecars; generated provenance does not extend to those files. The separate
upstream chain is `esp-pacs` through `esp-hal`, bound in
`adapters/esp-hal/esp32s31/`. Its `soc/` adapter contains non-radio SoC
transactions, cache/MMU access and GDMA ownership; it is not generated PAC
code. See [unsafe boundaries](UNSAFE.md) for the enforced exceptions.

PAC operations describe register-local fields and access. HAL operations own
multi-register order, polling, delays, lifecycle and recovery. Handwritten
code outside the restricted PAC uses typed accessors; missing fields must be
reviewed and published through the SVD/PAC. A Rust ownership proof does not
establish the meaning of a recovered register.

PHY borrows an opaque `PhyHal`; role code uses finite HAL/MAC capabilities.
Do not expose PAC callbacks, `Deref` escapes or owner re-exports above these
boundaries. Shared task-side handles remain tied to the HAL arena's explicit
serialization; a copyable handle grants no unsynchronized cross-thread MMIO
access. Existing Bluetooth/IEEE 802.15.4 PAC dependencies are explicit audit
exceptions, not a general route around HAL.

The current Wi-Fi execution domain has one Core0 supervisor and controlled
child execution. A station task may receive the exact affine owner through a
rendezvous while the supervisor waits for its return. This does not create a
second physical radio owner. ISR bindings record pending work and wake the
responsible runtime; network tasks and cross-core handoffs receive bounded
packet authority, not independent PAC ownership.

## Lifecycle and network ownership

Public handles carry commands. Dropping a handle never resets or destroys the
hardware. Starting consumes an idle owner; stopping returns idle only after
masking interrupt sources, draining pending IRQ work, completing or
quarantining TX, stopping RX, detaching queues and publishing link down.
Failure to prove quiescence retains a terminal fault owner instead of
fabricating reusable idle. Cancellation, abnormal drop and `mem::forget` must
not release storage still owned by hardware.

STA and AP own separate peer, security, Block-Ack, rate and lifecycle policy.
They share mechanisms below that policy. Their combined role has one physical
RX producer, TX owner and IRQ epoch on one channel; loss of the owning station
association tears down the pair before reuse. Monitor and standalone ESP-NOW
use explicit role composition rather than borrowing an unrelated active role.

Applications own `embassy-net::Stack`, DHCP, sockets and network tasks. The
Wi-Fi integration exposes a persistent network driver and selects the owned
or compatibility leaf at compile time. Idle, scanning, monitor and disconnected
states publish link down; role boundaries flush stale queue state. Network
adapters do not acquire radio policy or physical DMA ownership.

Radio scheduling selects a flow before reserving scarce internal SRAM.
Materialization transfers selected software work into a finite physical pool;
terminal completion or proven abort returns that storage. The experimental
research engine uses the shared egress and DMA contracts; its current
repository consumer is a driver test.
See the [egress architecture](../docs/WIFI_EGRESS_ARCHITECTURE.md) for queue,
materialization and transport contracts.

Ordinary code and task stacks may execute from PSRAM. DMA-visible storage,
interrupt/trap stacks and the audited hot/interrupt call graph remain in
internal SRAM; placement checks enforce this boundary.

## Feature scope and verification

Protocol limits, security modes and live/partial/fail-closed boundaries belong
in the [Wi-Fi feature frontier](chips/esp32s31/ieee80211/FEATURES.md) and
[Bluetooth LE feature frontier](chips/esp32s31/bluetooth/FEATURES.md). A parser,
register setter, descriptor or host test alone does not establish an
operational radio feature. Do not infer concurrent-radio support or new
capabilities from structural reuse.

[Qualification](../qualification/README.md) is the readiness authority. Its
[Wi-Fi](../qualification/targets/esp32s31/wifi-sta.toml),
[Bluetooth](../qualification/targets/esp32s31/bluetooth-le.toml) and
[IEEE 802.15.4](../qualification/targets/esp32s31/ieee802154.toml) manifests
connect production owners to host, vendor and dated HIL evidence. Validation
of a manifest is not a passing hardware qualification.

Unit tests live in separate child files beside the modules whose private
contracts they exercise; public integration tests live in crate `tests/`
directories. Preserve feature gating and ownership assertions when moving
code. Vendor comparison must exercise compiled production entries and fail
closed. Examples and HIL may compose or observe the driver, but must not own
another implementation of its protocol or hardware behavior.
