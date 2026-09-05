# RX ownership boundary

`open_esp_radio_esp32s31_wifi::rx` owns the finite prepared/live/halted
descriptor frontier, its delay capability and the qualified storage profile.
`frontier` preserves the adapter's original public paths and supplies the
Embassy timer implementation. The lower `chips/esp32s31/ieee80211/dma` crate owns
descriptor publication, cursor proofs, buffer leases and sticky arena poisoning.

The staged producer remains an adapter composition owner:

- `Esp32s31StagedRxProducer` retains the live ring, static DMA storage, staging
  pool, ordered publisher, delay, admission policy and diagnostic observer.
- `Esp32s31StoppedRx`, `Esp32s31PreparedRx` and `Esp32s31RxEpochResources` return
  the same queue sender, pool and observation capability across role epochs.
- `Esp32s31StagedRxPublisher` selects standalone or paired STA/AP publication.
  The paired branch classifies the frame against the retained VIF addresses.
- `DatapathRxService::service` in `dma/service.rs` calls chip
  `rx::transaction::service` synchronously and wraps its result in `ready`.
  Physical work therefore completes before the returned future is polled.

## Chip transaction and adapter ports

`rx::transaction` owns the bounded physical completion, staging and recycle
algorithm. Its `Admission`, `Publisher` and value-only observations require no
executor dependency. The existing adapter publisher implements the chip
trait locally, preserving standalone/paired routing and the first handoff
timestamp. Old admission and diagnostic type paths remain reexports.

The chip transaction borrows the ring, DMA storage and staging pool for one
service call. `Publisher::try_send` accepts a unique `NetworkRxFrame` or
returns that same frame in `Err`. It never receives ring or hardware access.
`Counters` borrows the three original cumulative fields; their update point
stays before the final frontier read, including when that read fails.

`dma/service/observation.rs` implements statically dispatched `Hooks`: concrete
clocks, logging and telemetry remain in the adapter. Optional hardware samples
are guarded before evaluation. The existing recycled-append diagnostic selector
is retained exactly once in the adapter. Performance builds perform no
diagnostic clock reads; the existing publication timestamp is preserved.
No additional counter is created in the chip crate.

The chip function is `inline(always)` into the existing `inline(never)` hot
adapter service. The adapter retains the `.hot.text.open_radio_rx_dma_service`
attribute; the chip crate retains `forbid(unsafe_code)`. Final-image placement
and stack audits check this boundary after optimization.

## Acceptance criteria

- Preserve the complete owner return for stopped, prepared, live and failed
  transitions, including pool, queue, delay, admission and observer lifetimes.
- Preserve the two bounded released-prefix observations, frozen LAST boundary,
  terminal writeback handling and exhausted-list republication continuation.
- Keep append and reload completion in the same synchronous transaction. Add
  no `await`, yield, task, allocation, descriptor copy or unbounded retry.
- Preserve admission order, reserved critical credit, VIF classification,
  rejection recovery and contiguous-prefix recycling. One staged lease keeps
  exactly its original DMA allocation until the consumer returns it.
- Preserve diagnostic feature gates, callback order, timestamps, service
  counters and the single telemetry selector. Do not instantiate a second
  counter or selector in the lower crate.
- Preserve private fields, same-Core0 endpoint constraints and current
  `Send`/`Sync` properties. New traits must be implementable by adapter-local
  endpoint types without orphan impls or a dependency cycle.
- Retain the hot service section and inline attributes. Inspect final target
  stack frames and placement when an extracted call changes compiled symbols;
  do not widen a stack allowance merely to accept the move.
- Compare production bodies against a snapshot and preserve existing tests
  for lease return, queue saturation, deferred discard, frozen-frontier recycle
  and terminal owner recovery. Run chip and adapter host tests plus the target
  feature matrix. Hardware qualification requires fresh HIL evidence if the
  physical transaction or scheduling changes.

## Allocation ownership through publication

| Edge | Retained authority | Return/recovery |
|---|---|---|
| Completed unit still in the frozen frontier | Radio ring/storage | Capacity refusal leaves the unit in DMA ownership; no lease exists yet |
| Malformed or overload-discarded unit taken | Radio ring/storage | Explicit deferred recycle retains the unit; dropping the taken owner would quarantine the arena |
| Valid unit staged | One `NetworkRxFrame` owns the original allocation | Descriptor remains observed until upper ownership returns |
| Publisher accepts | Queue, then protocol/network consumer | Consumer drop releases the original allocation exactly once |
| Publisher rejects | Rejection returns the same frame to the transaction | Existing error path drops that frame and reports `Corrupt`; no retry or replacement lease |
| Released allocation behind an older retained frame | Radio storage records release | Only the longest contiguous released prefix may be rearmed |

A release is not a descriptor rearm, and terminal quarantine is not hardware
quiescence. Neither a publication callback nor an admission policy receives a
mutable ring, PAC access or permission to manufacture another physical owner.

## Separate lifecycle cancellation issue

The existing `Esp32s31StagedRxEpoch::start` replaces its stored state with
`Vacant` before awaiting the consuming `Esp32s31PreparedRx::start`. Cancelling
that future during the settle delay can therefore lose the recoverable
prepared owner and its sender. The walker has not been enabled at that point;
this finding does not establish a live-DMA use-after-free.

This structural transaction extraction does not change the start state
machine. A separate lifecycle correction should retain `Prepared` in the epoch
while awaiting only its borrowed delay, then perform the consuming enable
transition synchronously. Its regression must poll a pending delay, cancel,
and prove that retry preserves the original ring, pool and queue endpoint.
