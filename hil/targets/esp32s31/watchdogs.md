# HIL system and PHY watchdogs

HIL explicitly selects 5 s startup, 1 s maintenance and 1 s shutdown/retained
cycle budgets in `runtime/src/watchdog.rs`, with caller-owned stable TIMG1 and
policy storage. These engineering values are not qualified production defaults.
`system-watchdog` uses the same SoC service and a separate 1 s test budget.
Its dedicated `system-watchdog` image starts neither radio nor a network stack,
advertises `system_watchdog`, and reports reset cause through `GetBootStatus`.
It needs only the DUT and records reset evidence, not RF-stop timing or injected
PHY restoration failures. `wifi-maintenance-restart`
requires real connected common/Wi-Fi calibration, unchanged association and
continued UDP before cold restart and new-generation UDP recovery. Its
source definition is not hardware qualification.

`wifi-phy-watchdog` injects into actual maintenance using
`diagnostic-phy-fault`. The image explicitly selects a 5 s maintenance budget
to include the diagnostic handshake. Other radio images retain the 1 s maintenance budget above. No
command refreshes the lease. The host requires a reached checkpoint and a
serialized release acknowledgement before expecting autonomous MWDT1 reset.
The checkpoint is either after the real PBus-clear child, before publishing its
completion, or after PHY return, before IRQ/MAC restoration. Modes block a
synchronous poll, withhold the child completion, stall restoration, or drop the
actual owner-bearing child future. Withheld completion is not an injected
silicon IRQ loss. Cancellation is acknowledged only after the child is dropped.

Each fault is bracketed by bidirectional UDP checks on the original and reset
boots. Normal calibration is followed by a wait longer than the diagnostic
budget, and a zero-duration pause is rejected. The PHY
hooks have no timer, reset or transport dependency and are compiled out without
`lifecycle-fault-injection`. This scenario does not measure RF cessation or
qualify cold-start, close/wake or temperature-dependent execution bounds.

The host's domain workloads own their link and calibration checks:
`workload/bluetooth/phy_watchdog.rs` owns peripheral ACL, and
`workload/ieee80211/phy_watchdog.rs` owns station admission and UDP.
`workload/phy/fault_lifecycle.rs` coordinates checkpoints and reboot evidence
without selecting or importing either protocol. SoC-only tests live under
`workload/system/`; DTM-triggered reset remains a Bluetooth workload.
