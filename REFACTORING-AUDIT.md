# Architecture refactoring audit

Temporary review record of the architecture refactoring branch. It is kept
here at the maintainer's request and must be removed before the branch merges
into `main` (see the documentation rules in `AGENTS.md`).

Audited revision: `f07876d` (branch `ccr-4c23f497-cfn0jy`). Four independent
read-only reviews covered the port contracts, sans-IO and layering, the
Wi-Fi stack, and cross-cutting contracts with simulation readiness.

## Verdict

The direction is sound and the foundation exists, but two architectures
coexist: the portable path (radio ports, upper MAC, sans-IO protocols) runs
only in host tests, while firmware still uses the ESP32-S31 path. Several
responsibilities therefore have two owners, and some contracts are specified
more completely than they are implemented.

Confirmed correct:

- No `protocol` package waits: no async bodies, `.await`, spins, time-driver
  calls or mutable global state in any of the 14 protocol packages.
- Layer and platform edges are clean: no portable package depends on a family
  or chip package; services depend only on contract, protocol and service.
- Wi-Fi retransmission keeps sequence number and PN, sets the Retry bit only
  after an on-air attempt, resends only unacknowledged A-MPDU subframes and
  releases buffers on completion and refusal.

## Findings

### A. Unfinished migration

- `Esp32s31LowerMac` and `UpperMacTx` have no production callers. Retry,
  EDCA state, protection, transmit PN and receive reorder each exist in the
  portable path and in the S31 path.
- `oer-ieee80211-softmac` still carries portable types that duplicate
  `oer-ieee80211-lower-mac` (`VifId`, `MacRxMetadata`,
  `MacServiceCapabilities`).
- Station join, scan and RSN backends are chip-transaction ports, not clients
  of the lower-MAC port.

### B. Port event model

- `next_event` has exactly one consumer, and `UpperMacTx` holds it for a whole
  exchange; a second task steals completions and an exchange can wait forever.
- The S31 Wi-Fi backend keeps every event in one frame-sized bounded queue, so
  receive bursts drop completions and TBTT events, and an oversized frame is
  reported as queue loss.
- Backend progress (802.15.4 backoffs, the Wi-Fi publication watchdog and
  retune) runs inside `next_event`; no contract states that someone must keep
  polling it.
- Event loss is sometimes silent (uninstall clears the loss flag; the
  OpenThread adapter ignores `EventsLost`).
- After poisoning, the Wi-Fi runtime can spin on an expired deadline
  (to be verified).

### C. Failure classes and lifecycle

- "Not installed" or "paused" (transient) shares the port error channel with
  poisoning (reset required).
- Poisoning is invisible to a consumer awaiting events; no port emits
  `LifecycleEvent::Failed { Poisoned }`.
- Only the Wi-Fi port has a lifecycle part; Bluetooth LE has none and
  IEEE 802.15.4 completes lifecycle commands without terminal events.
- Bluetooth `RequestError::Unavailable` mixes refusal with a faulted radio, and
  `serve` retries every refusal, `Unsupported` included, every millisecond.
- Loss markers, fault types, correlation identities, cancellation and method
  names differ between the three ports without reason.

### D. Vendor policy in portable code

- The portable Bluetooth LE controller contains coexistence tables recovered
  from the ESP32-S31 `libble_app.a`, the vendor event priorities 13/8 and the
  vendor advertising tail; `ConnectionEvent::priority` is the Espressif
  scheduler's 0–15 domain.
- IEEE 802.15.4 CSMA-CA and retransmission run below the port.
- Family packages hold ESP32-S31-only facts (802.15.4 sensitivity and RSSI
  compensation, Wi-Fi `libpp.a` tables) without evidence that ESP32-C5 matches;
  the PTI table in `oer-espressif-coex` is the model to follow.

### E. Time

- `oer-time` is used only by protocols and services; runtimes call
  `embassy_time` directly and about fifteen ad-hoc timer traits remain, with
  mixed units and static functions that cannot carry per-instance time.
- No port states how its radio epoch relates to monotonic time;
  `RadioInstant` values of different backends type-check against each other.
- `Ieee802154RadioPort::clock` returns `fn() -> u64`, which forces a global
  clock and prevents several simulated nodes per process.
- Several 802.11 state machines take raw `u64` microseconds or count caller
  millisecond ticks instead of taking `Instant` and returning a deadline.

### F. Wi-Fi transmit contract gaps

- The control-frame rate is chosen above the port and dropped (`Protection`
  has no rate); no AIFSN/TXOP setting exists.
- `HardwareServices` flags are never read above the port; there is no
  software CCMP for backends without a hardware cipher.
- A-MPDU results are counts only; an error mid-exchange loses the exchange and
  leaves the contention window doubled.
- No portable key context ties key handle, transmit PN and replay state;
  `KeyInstall` lacks RSC and IGTK; key slots are not stated per role.
- The S31 hardware timeout maps to `Aborted`, which the planner treats as
  final.
- Not dual-band: the Espressif retry ladder falls back to CCK, AP and retune
  use the 2.4 GHz `WifiChannel`, widths stop at 40 MHz and the Block Ack
  bitmap stops at 64.

### G. Shared radio, memory, simulation

- `oer-radio-coex` is vocabulary only; there is no portable arbiter contract,
  and `RadioSystem` builds only for the chip.
- Buffer ownership is expressed twice (`TxBuffer` versus the unsafe,
  32-bit `StableDmaBacking`); 802.15.4 bypasses both.
- No platform/simulation package, virtual time driver or medium model exists;
  compositions and the facade know only ESP32-S31 with Embassy.

### H. Repository checks

- The sans-IO check is a line scan and misses manual `Future` impls,
  `poll_fn`, combinators, `critical_section`, interior-mutable statics and
  macro expansions.
- Third-party dependencies are not platform-checked; names are not checked
  against classification.

## Resolution plan

1. `oer-radio-port` contract package: failure classes with `PortError`, one
   `EventsLost` and a terminal `Poisoned` event, lifecycle vocabulary,
   correlation identities with a backend-reserved range, clock resolution and
   epoch relation. All three ports adopt it.
2. Event model: one consumer per port stated in every contract; backend
   progress moves to an explicit runner the composition polls; a portable
   event router dispatches completions by identity; backends keep completion
   queues that cannot overflow.
3. One time contract: ad-hoc timers become `oer_time::Timer` adapters,
   runtimes stop calling `embassy_time`, a virtual clock and virtual
   embassy-time driver exist, radio epochs are related to monotonic time, the
   802.15.4 function-pointer clock is removed and 802.11 state machines take
   `Instant` and return deadlines.
4. Vendor policy moves to family packages behind policy traits
   (Bluetooth LE coexistence and priorities), 802.15.4 CSMA/retry moves above
   the port into a service, and every family value carries ESP32-S31 and
   ESP32-C5 evidence or becomes a chip parameter.
5. Wi-Fi transmit contract completion: control rate, AIFSN/TXOP,
   acknowledgement bitmaps, resumable exchanges, retryable hardware timeout,
   capability validation, key context, portable receive pipeline, dual-band
   rate ladder and channels, rate-control trait.
6. Portable radio arbiter trait with `RadioSystem` as one implementation;
   `CoexPriority` in the 802.15.4 port.
7. Portable frame lease in `oer-memory`, separate from the chip DMA proofs;
   ports and datapath use it.
8. Finish the migration: roles run on the lower-MAC port, S31 duplicates are
   removed and the legacy softmac contract types merge into lower-mac.
9. Stronger checks: `syn`-based sans-IO analysis, third-party platform map,
   name/classification agreement, chip facts in family code.
10. Simulation platform: a medium model implementing the three ports and the
    arbiter, with a facade `sim` feature.
