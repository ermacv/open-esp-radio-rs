# Radio permit pool

`oer-radio-permit-pool` implements the `PermitPool` of
[`oer-radio-port`](../../../radio/port/README.md) without allocation:
`FifoPermitPool<'id, K, PERMITS, WAITERS>` holds `PERMITS` permits of one
install, which the backend's `resources` mint, and serves its takers first in,
first out. A backend keeps it in its held part of the install
(`PortBackend::Held`), so the permits of submitted work come back to it before
their terminal events are handed out.

- A taker takes its place in the line at its first poll and is served once it
  leads and a permit is free. A taker whose future drops gives up its place.
- A released permit wakes the leader. A served taker that leaves a permit free
  wakes the next leader.
- The line holds `WAITERS` places, which must cover every taker that can wait
  at once. A taker that finds the line full is a fault of the composition, and
  the pool panics, so arrival order is always strict.
- The pool is `!Sync`. It lives in the port body's task, and nothing outside
  that task reaches it.

Its tests run the shared rule `conformance::permits_serve_takers_in_arrival_order`
on a pool of one permit and on a pool with a permit held aside.
