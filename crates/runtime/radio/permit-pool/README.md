# Radio permit pool

`oer-radio-permit-pool` implements the `PermitPool` of
[`oer-radio-port`](../../../radio/port/README.md) without allocation:
`FifoPermitPool<'id, K, PERMITS, SEATS>` holds `PERMITS` permits of one
install, which the backend's `resources` mint, and serves its takers first in,
first out. Its line holds a place for each of the `SEATS` seats `Permits`
made with the `Line<'id, K, SEATS>` the pool is built with, and each taker
waits with its own seat. A backend keeps it in its held part of the install
(`PortBackend::Held`), so the permits of submitted work come back to it before
their terminal events are handed out.

- A taker takes its seat's place in the line at its first poll and is served
  once it leads and a permit is free. A taker whose future drops gives up its
  place, and its seat stays with it.
- A released permit wakes the leader. A served taker that leaves a permit free
  wakes the next leader.
- The line has a place for every seat, so it is never full and arrival order
  holds for every taker. A `Line` is a copyable receipt: several pools whose
  takers are the same seats (one per transmit queue) each take it. A backend mints a kind's seats once per install. A
  take that leaked without its drop (`mem::forget`) loses its place to its
  seat's next take.
- The pool is `!Sync`. It lives in the port body's task, and nothing outside
  that task reaches it.

Its tests run the shared rule `conformance::permits_serve_takers_in_arrival_order`
on a pool of one permit and on a pool with a permit held aside, and check
arrival order across every seat, a dropped and a leaked take, and the wakes of
the leader and the next leader.
