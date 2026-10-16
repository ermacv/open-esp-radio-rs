# HIL system and PHY watchdogs

The dedicated `system-watchdog` image uses the SoC deadline watchdog (TIMG1)
with a 1 s test budget. It starts neither radio nor a network stack,
advertises `system_watchdog`, and reports reset cause through
`GetBootStatus`. It needs only the DUT and records reset evidence, not RF-stop
timing or injected PHY restoration failures.

The shared radio's periodic tracking maintains the PHY, and PHY fault
injection targets Bluetooth maintenance.
No command refreshes a lease. The host requires a reached checkpoint and a
serialized release acknowledgement before expecting autonomous MWDT1 reset.
The checkpoint is either after the real PBus-clear child, before publishing its
completion, or after PHY return, before restoration. Modes block a synchronous
poll, withhold the child completion, stall restoration, or drop the actual
owner-bearing child future. Withheld completion is not an injected silicon IRQ
loss. Cancellation is acknowledged only after the child is dropped. The PHY
hooks have no timer, reset or transport dependency and are compiled out without
`lifecycle-fault-injection`. This does not measure RF cessation or qualify
cold-start, close/wake or temperature-dependent execution bounds.

The host's domain workloads own their link and calibration checks:
`workload/bluetooth/phy_watchdog.rs` owns peripheral ACL, and
`workload/phy/fault_lifecycle.rs` coordinates checkpoints and reboot evidence
without selecting or importing a protocol. SoC-only tests live under
`workload/system/`; DTM-triggered reset remains a Bluetooth workload.
