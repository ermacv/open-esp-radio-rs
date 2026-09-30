# Wi-Fi station drivers

`oer-ieee80211-sta-service` drives the station state machines of
[`oer-ieee80211-sta`](../../../protocols/ieee80211/sta/README.md). That crate
is sans-IO: it owns state, policy and the ports; this crate owns the futures
that wait on those ports and on the `oer-time` `Timer`, for any executor.

| Module | Driver |
| --- | --- |
| `join` | `StaJoinRunner`: Open System and SAE Authentication and Association against absolute millisecond deadlines |
| `scan` | `StaCandidateScanService`: one finite channel plan and candidate selection |
| `station` | `StaLifecycleService`: outer attempt, reconnect, backoff, disconnect and stop |

Every driver returns the exact caller-owned radio state at each success,
retry, stop and failure edge. Chip ports are implemented by roles and runtimes;
the concrete runtime polls these drivers.
