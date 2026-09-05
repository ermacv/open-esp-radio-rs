# Radio execution

`embassy/esp32s31/{ieee80211,bluetooth}` contains concrete radio execution under
Embassy. These existing packages retain their Cargo names and public imports;
directory boundaries describe their main responsibility.

| Module | Responsibility |
| --- | --- |
| `ieee80211/src/roles/` | Role execution and retained TX/RX owners |
| `ieee80211/src/roles/access_point/network_tx.rs` | One AP TX owner, publication and cancellation |
| `ieee80211/src/roles/access_point/network_tx/{queue,power_save,aggregate,completion}.rs` | Lease queues, TIM/DTIM release, standby aggregation and completion on that same owner |
| `ieee80211/src/datapath/` | Packet handoff and async composition around chip transactions |
| `ieee80211/src/composition/` | Compatibility reexport of the PHY time binding under its previous namespace |
| `ieee80211/src/diagnostics/` | Optional execution observation |
| `bluetooth/src/controller/` | One controller epoch, command/response boundaries and timer progress |
| `bluetooth/src/session/` | Finite DTM, advertising, scanning and peripheral sessions |
| Both `src/time/phy.rs` | Embedded Embassy implementations of shared PHY time contracts |

Hardware transactions and finite chip state remain below these packages. A
runtime retains their affine owners across borrowed waits, returns the same
resources on rejection and preserves terminal owners when quiescence is not
proven. Moving a file does not add a new hardware capability or runtime feature.

The PHY time leaves are adapters inside the execution packages. The Wi-Fi
binding supplies a direct Embassy delay; Bluetooth also validates the timebase
and handles overflow. Keeping them explicit avoids a new crate solely for two
small implementations and preserves their distinct contracts. The platform
executor/time ABI remains in [adapters](../adapters/embassy/README.md).

The [integration layer](../integration/esp32s31/embassy/) chooses memory budgets,
claims static resources and composes complete radio lifecycles. Bluetooth
`system/{construction,runner,quarantine}` separates assembly, the one hardware
loop and fail-stop retention. Wi-Fi's supervisor owns shared physical resources
and transitions between roles. Neither product composition depends on a second
runtime owner hidden in a task or network handle.

Portable protocol policy cannot depend on this execution domain. The
architecture audit follows transitive normal/build dependencies to enforce
that boundary; the generic radio facade may use generic Embassy contracts but
cannot depend on these concrete ESP32-S31 runtimes.
