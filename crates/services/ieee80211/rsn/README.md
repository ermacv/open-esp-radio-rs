# WPA2 station drivers

`oer-ieee80211-rsn-service` drives the sans-IO WPA2 supplicant of
[`oer-ieee80211-rsn`](../../../protocols/ieee80211/security/rsn/README.md).
That crate returns typed requests and never waits; this crate owns the futures
that wait on its ports and on the `oer-time` `Timer`, for any executor.

| Module | Driver |
| --- | --- |
| `runner` | `RsnHandshakeRunner`: Message 1 to Message 3 against absolute response deadlines, returning the key-install ticket with RX stopped; `RsnKeyInstallRunner`: key publication, Message 4 and rollback ordering |
| `supplicant` | `process_frame`: one EAPOL-Key frame with the requested key-data unwrap awaited through `AsyncRsnKeyUnwrap` |

Chip ports (`RsnHandshakeBackend`, `RsnKeyInstallBackend`,
`AsyncRsnKeyUnwrap`) are declared by the protocol crate and implemented by
roles and runtimes; the concrete runtime polls these drivers.

Runner tests use `oer-time-virtual::SkipClock`. Wait observations count
polled waits; an unpolled losing branch leaves both time and counts alone.
