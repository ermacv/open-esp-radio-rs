# ESP32-S31 IEEE 802.15.4 control-state boundary

This note records the source-only control-state evidence used for Iteration 4.
Every vendor statement below comes from public ESP-IDF source pinned at commit
`7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe`. No vendor binary, disassembly,
private oracle, or extracted proprietary table is an input.

The overall verdict is **INCOMPLETE**. The private software-state vocabulary,
public-state projection, state-dependent stop paths, interrupt dispatch order,
and a bounded subset of terminal transitions are source-confirmed. Live command
execution is not qualified: `EVENT_STATUS` acknowledgement, `STOP` completion,
DMA quiescence, Timer1 races, RF/BTBB/PLL bring-up, and on-air behavior remain
open.

## Source ledger

The hashes below identify the complete public files, not excerpts. Existing
hashes are carried forward from the lifecycle and dataplane reviews; the state
and timer files added for this review were hashed from the same pinned commit.

| Public ESP-IDF source | Relevant lines | SHA-256 |
| --- | --- | --- |
| [`components/ieee802154/private_include/esp_ieee802154_dev.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/private_include/esp_ieee802154_dev.h#L27-L43) | exact private 12-state enum | `afdb884e6dc78f19c9d72adedfc1ea7fdbbdebb86d1e7aaa80bb89e62a74bfd3` |
| [`components/ieee802154/include/esp_ieee802154_types.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/include/esp_ieee802154_types.h#L19-L28) | five public radio states | `5206f935bfaee354562e7ab87d499a196db251aa52f1d1bd9051bb32b4957424` |
| [`components/ieee802154/esp_ieee802154.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/esp_ieee802154.c#L286-L315) | private-to-public state projection | `a83716d9944d4ffba1998cc64ebb635a605b60fc77c74ae6070e83a1c617f1bc` |
| [`components/ieee802154/driver/esp_ieee802154_dev.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/driver/esp_ieee802154_dev.c#L78-L165) | deferred-next flag and timer-zero callback | `9aaccfa2832cb89bfdfd98086a984269e621400a272b02926c4e088d16222830` |
| [`components/ieee802154/driver/esp_ieee802154_dev.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/driver/esp_ieee802154_dev.c#L314-L505) | state-specific stop and next-operation paths | same file/hash as above |
| [`components/ieee802154/driver/esp_ieee802154_dev.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/driver/esp_ieee802154_dev.c#L507-L769) | event-handler state changes and terminal effects | same file/hash as above |
| [`components/ieee802154/driver/esp_ieee802154_dev.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/driver/esp_ieee802154_dev.c#L782-L895) | snapshot, acknowledgement, assertions, and ordered dispatch | same file/hash as above |
| [`components/ieee802154/driver/esp_ieee802154_dev.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/driver/esp_ieee802154_dev.c#L985-L1131) | operation-start order and timed receive callbacks | same file/hash as above |
| [`components/ieee802154/driver/esp_ieee802154_dev.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/driver/esp_ieee802154_dev.c#L1193-L1252) | RF client guard, sleep, ED, and CCA entry | same file/hash as above |
| [`components/ieee802154/driver/esp_ieee802154_timer.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/driver/esp_ieee802154_timer.c#L13-L125) | timer callback storage, clearing, and invocation | `66a2b68bb718d8fa878cbf28488af810ecb902288fde26dc7a0e5b32dcbd1326` |
| [`components/esp_hal_ieee802154/include/hal/ieee802154_common_ll.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_hal_ieee802154/include/hal/ieee802154_common_ll.h#L25-L62) | opcodes and named event bits | `ba4ce294b402df311f25c4d0ce9cb33449e3eb41993aff94a25df5a66142d471` |
| [`components/esp_hal_ieee802154/include/hal/ieee802154_common_ll.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_hal_ieee802154/include/hal/ieee802154_common_ll.h#L175-L238) | command, event, abort, and direct-address accessors | same file/hash as above |
| [`components/soc/esp32s31/register/soc/ieee802154_reg.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/soc/esp32s31/register/soc/ieee802154_reg.h#L184-L194) | event status without an access-class annotation | `fd3f944ac97634605083031f96c0f942af26a81a9e9a3123281c59e5719f9d9c` |
| [`components/soc/esp32s31/register/soc/ieee802154_struct.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/soc/esp32s31/register/soc/ieee802154_struct.h#L143-L151) | event-status bitfield without acknowledgement semantics | `da13c2bc78cd6ef35a4e54ddddf11ce48fda967746193f1a0ad03578a5881752` |
| [`components/esp_phy/src/phy_init.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_phy/src/phy_init.c#L332-L475) and [L997-L1023](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_phy/src/phy_init.c#L997-L1023) | shared PHY client lifecycle and opaque registration/calibration | `1e230e72f91c4b11f35b6b623dc45cf8628961593e45774e2bf610dce9896fbf` |
| [`components/esp_phy/include/esp_private/phy.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_phy/include/esp_private/phy.h#L38-L182) | opaque PHY wakeup and PLL declarations | `04027a4f11a0cd6c6a76478d681f7e29e4ba8ecf038d120d991bcab01735a53f` |
| [`components/esp_phy/src/btbb_init.c`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_phy/src/btbb_init.c#L98-L139) | BTBB first-user initialization and refcount | `bde0cddaa033d2f34a4eaf0f1994b2d417850dba9d2fc5e6e9ee0ceb7caca3c3` |
| [`components/esp_phy/include/esp_private/btbb.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_phy/include/esp_private/btbb.h#L14-L20) | opaque BTBB initializer declaration | `659a94ca15d9e7d5531f34c755476523d285bc2cbc0a378b58505e67facea289` |

## 1. Exact software states and public projection

The private driver enum has twelve implicit, consecutive C values. Order is
part of the source observation; it is not a claim about a hardware-state field.
The public getter collapses those twelve values into five API states exactly as
shown:

| Value | Private state | Public API state |
| ---: | --- | --- |
| 0 | `IEEE802154_STATE_DISABLE` | `ESP_IEEE802154_RADIO_DISABLE` |
| 1 | `IEEE802154_STATE_IDLE` | `ESP_IEEE802154_RADIO_IDLE` |
| 2 | `IEEE802154_STATE_SLEEP` | `ESP_IEEE802154_RADIO_SLEEP` |
| 3 | `IEEE802154_STATE_RX` | `ESP_IEEE802154_RADIO_RECEIVE` |
| 4 | `IEEE802154_STATE_TX_ACK` | `ESP_IEEE802154_RADIO_RECEIVE` |
| 5 | `IEEE802154_STATE_TX_ENH_ACK` | `ESP_IEEE802154_RADIO_RECEIVE` |
| 6 | `IEEE802154_STATE_TX_CCA` | `ESP_IEEE802154_RADIO_TRANSMIT` |
| 7 | `IEEE802154_STATE_TX` | `ESP_IEEE802154_RADIO_TRANSMIT` |
| 8 | `IEEE802154_STATE_TEST_TX` | `ESP_IEEE802154_RADIO_TRANSMIT` |
| 9 | `IEEE802154_STATE_RX_ACK` | `ESP_IEEE802154_RADIO_TRANSMIT` |
| 10 | `IEEE802154_STATE_ED` | `ESP_IEEE802154_RADIO_RECEIVE` |
| 11 | `IEEE802154_STATE_CCA` | `ESP_IEEE802154_RADIO_TRANSMIT` |

The projection is deliberately counterintuitive in two places: standalone ED
is publicly receive, while standalone CCA and ACK reception after transmit are
publicly transmit. A five-state public value cannot recover the private phase
and is not sufficient to select a stop path or validate an interrupt batch.

The private value is software-owned. For example, timed receive assigns
`RX` before ETM starts the hardware receiver, and transmit assigns `TX` or
`TX_CCA` immediately after issuing the corresponding command. This review does
not equate that enum with instantaneous RF or MAC hardware state.

## 2. ISR legality is sequential, not set-wise

The ISR enters the driver spinlock, samples the event image plus both abort
reasons, passes the complete snapshot to the LL helper named `clear_events`,
and then visits the local snapshot in this exact order. Whether that helper is
an effective, race-safe acknowledgement remains an explicit gap.

1. RX-abort phase one;
2. RX SFD;
3. TX SFD;
4. TX done;
5. RX done;
6. ACK TX done;
7. ACK RX done;
8. RX-abort phase two;
9. TX abort;
10. ED done;
11. timer zero;
12. timer one;
13. at most one deferred `next_operation()` call.

The explicit state assertions at each dispatch point are:

| Event | State accepted at that point in the ordered dispatch |
| --- | --- |
| `RX_SFD_DONE` | `RX`, `RX_ACK`, `TX`, `TX_CCA`, or `TX_ENH_ACK` |
| `TX_SFD_DONE` | `TX`, `TX_CCA`, `TEST_TX`, `TX_ENH_ACK`, or `TX_ACK` |
| `TX_DONE` | `TX`, `TX_CCA`, or `TEST_TX` |
| `RX_DONE` | `RX` |
| `ACK_TX_DONE` | `TX_ACK`, `RX`, or `TX_ENH_ACK` |
| `ACK_RX_DONE` | `RX_ACK`, `TX`, `TX_CCA`, or `TX_ENH_ACK` |
| `ED_DONE` | `ED` or `CCA` |
| `TIMER0_OVERFLOW` | `RX_ACK`, except for the separate test-timer allowance |
| `TIMER1_OVERFLOW` | no state assertion; behavior depends on the installed callback |

Abort legality is reason-dependent rather than one event-wide state set:

- RX `SFD_TIMEOUT`, `CRC_ERROR`, `INVALID_LEN`, `FILTER_FAIL`, `NO_RSS`,
  `UNEXPECTED_ACK`, `RX_RESTART`, and `COEX_BREAK` assert `RX` in phase one.
  `ED_ABORT` and `ED_COEX_REJECT` assert `ED` or `CCA`. `RX_STOP`,
  `TX_ACK_STOP`, and `ED_STOP` return without a state assertion.
  `TX_ACK_TIMEOUT`, `TX_ACK_COEX_BREAK`, and `ENHACK_SECURITY_ERROR` are
  deferred to phase two, where the `TX_ACK`/`TX_ENH_ACK` assertions are
  conditional debug-monitor assertions.
- TX `RX_ACK_SFD_TIMEOUT`, `RX_ACK_CRC_ERROR`, `RX_ACK_INVALID_LEN`,
  `RX_ACK_FILTER_FAIL`, `RX_ACK_NO_RSS`, `RX_ACK_COEX_BREAK`,
  `RX_ACK_TYPE_NOT_ACK`, `RX_ACK_RESTART`, and `RX_ACK_TIMEOUT` assert
  `RX_ACK`. `TX_COEX_BREAK` and `TX_SECURITY_ERROR` assert `TX` or `TX_CCA`;
  `CCA_FAILED` and `CCA_BUSY` assert `TX_CCA`. `RX_ACK_STOP` and `TX_STOP` do
  not establish a normal-build state assertion.

These assertions observe the current mutable software state after all earlier
handlers, not a state frozen beside the event snapshot. Earlier handlers also
run callbacks and stop timers. Two source-backed same-snapshot examples show
why a set-wise validator is insufficient:

- `TX_DONE` for a frame requiring an ACK changes `TX`/`TX_CCA` to `RX_ACK` and
  assigns deferred-next false. A later co-latched `ACK_RX_DONE` is then legal,
  completes the logical transmit, and assigns deferred-next true.
- `RX_DONE` may change `RX` to `TX_ACK` or `TX_ENH_ACK` and assign
  deferred-next false. A later co-latched `ACK_TX_DONE` accepts that new state,
  delivers the received frame, and assigns deferred-next true.

Conversely, `event_end_process()` in an earlier handler stops both timers and
clears their callback slots. A co-latched Timer1 bit is still visited later,
but its handler can then find no callback. A faithful vendor comparison must
therefore exercise compiled production control logic in order; it cannot call
independent shadow handlers or validate every event against only the entry
state.

## 3. Deferred-next is last-assignment-wins

`NEEDS_NEXT_OPT(a)` is a plain assignment to one global boolean. It is not an
OR, a counter, or an append-only request. Ordered handlers can assign it more
than once, and the last executed assignment determines the end-of-batch
decision. In particular, a transition into ACK TX/RX assigns false so the
operation remains active, while a later ACK completion assigns true. Some
TX-abort branches also assign false, so replacing the fold with `any(true)`
would change behavior.

After all recognized bits have been visited, the ISR checks the final boolean.
If true, it calls `next_operation()` exactly once and then clears the boolean.
`next_operation()` first resolves a pending timed-receive stop, then either
starts another receive when `rx_when_idle` is set or moves through the
idle/sleep policy. It is not called once per terminal bit.

This proves an ordered, single-decision policy for accepted source paths. It
does not prove that every arbitrary multi-bit combination is valid. The vendor
ISR may already have performed earlier callbacks before a later assertion
detects an impossible combination; a safe pure model may instead reject the
whole unsupported batch before exposing any effect. Such fail-closed behavior
is intentionally narrower and must not be labelled exact vendor equivalence.

## 4. `STOP` is a state-specific reconciliation request

`stop_current_operation()` first clears both ETM channels and stops both MAC
timers. It then selects a private-state-specific path:

| Private state | Source action after common timer/ETM teardown |
| --- | --- |
| `DISABLE` | no command and no event reconciliation |
| `IDLE` | issue `STOP`; no status sample |
| `SLEEP` | no command and no event reconciliation |
| `RX` | issue `STOP`; sample events once; deliver an already-latched `RX_DONE`; clear `RX_DONE`, `RX_ABORT`, and `RX_SFD_DONE` |
| `TX_ACK` | issue `STOP`; clear security; synthesize receive delivery; clear `ACK_TX_DONE`, `RX_ABORT`, and `TX_SFD_DONE` |
| `TX_CCA` | run the TX stop path, then additionally clear `TX_ABORT` |
| `CCA` | issue `STOP`; clear `ED_DONE` and `RX_ABORT` |
| `TX` | issue `STOP`; sample events once; report done only for an already-latched eligible `TX_DONE`, otherwise report abort; clear `TX_DONE`, `TX_ABORT`, and `TX_SFD_DONE` |
| `TX_ENH_ACK` | use the TX stop path, but synthesize receive delivery and additionally clear `ACK_TX_DONE` |
| `TEST_TX` | no switch arm; reaches the default assertion |
| `RX_ACK` | issue `STOP`; sample events once; stop/mask timer zero; report transmit done for an already-latched `ACK_RX_DONE`, otherwise no-ACK failure; clear `ACK_RX_DONE`, `RX_SFD_DONE`, and `TX_ABORT` |
| `ED` | issue `STOP`; clear `ED_DONE` and `RX_ABORT` |

The function does not set a common post-stop state. Its boolean return is
always true and callers do not use it as a completion predicate. Where an
event sample exists, it is one immediate read after issuing opcode `0x45`;
there is no command-busy check, completion bit, fence, delay, or bounded poll.
Several paths only issue the command and clear selected event bits.

Operational start paths then continue immediately. For example, TX setup calls
stop, refreshes the PIB, publishes TX and optional ACK-RX addresses, and issues
`TX_START` or `CCA_TX_START`. Source order alone therefore does not prove that
the old engine has stopped touching DMA before a new address is published. Nor
does it prove that the event sample after `STOP` contains every terminal edge
that races with the request.

The event reconciliation also depends on the unresolved `EVENT_STATUS` access
class. The LL helper's write pattern is compatible with W1C, but neither S31
register header states acknowledgement or concurrent-arrival semantics. A live
open implementation must not turn this source sequence into an ordinary RW
modify operation.

## 5. Timer1 callback context and the unhandled clock event

The Timer1 implementation stores one function pointer and one opaque context.
On `TIMER1_OVERFLOW`, while the MAC ISR still holds its spinlock, the handler:

1. copies the callback and context to locals;
2. clears both global slots;
3. invokes the callback inline in interrupt context;
4. returns to the ISR, which only then evaluates deferred-next.

Clearing before invocation explicitly permits the callback to install a new
timer callback. There is no Timer1 state assertion. Timed receive uses this
facility as a two-stage chain: the first callback schedules the receive-window
end, and the second either leaves an in-progress frame running with a pending
stop or performs the state-specific stop and reports the window complete.
These callback, lock, ETM, SFD, and stop races are outside the untimed pure
control subset.

`IEEE802154_EVENT_CLOCK_COUNT_MATCH` is named as bit 10. Initialization's
non-test event image `0x3eff` leaves that bit enabled, but the reviewed ISR has
no branch for it. The bit remains in the local `events` value and reaches the
final `events == 0` assertion. Unnamed mask bits 7 and 13 have the same lack of
dispatch semantics. Until their purpose and acknowledgement behavior are
proved, a safe event batch must reject them before any modeled transition and
an active IRQ mask must not enable them.

## 6. RF, BTBB, and PLL remain outside the control proof

Every ordinary RX, TX, CCA, and ED entry first passes through the RF-enable
guard. The public enable/lifecycle path also acquires shared modem-clock and PHY
clients and enables BTBB before MAC operation. Public source exposes the
ownership shape, but not enough implementation to reproduce the physical
postconditions:

- first-user BTBB initialization calls opaque `bt_bb_v2_init_cmplx(1)`;
- PHY registration/calibration reaches opaque `register_chipv7_phy(...)`;
- RF wakeup and tracking depend on opaque `phy_wakeup_init()`,
  `phy_track_pll_init()`, and `phy_track_pll()` effects;
- MAC initialization still calls opaque `ieee802154_txon_delay_set()`;
- TX policy still depends on the opaque channel power table behind
  `bt_bb_get_tx_pwr_table()`.

The PHY client set is shared with Wi-Fi and Bluetooth, and BTBB has its own
first/last-user refcount. A control actor cannot safely replace either with an
unconditional boolean enable or release. Clock acquisition and MAC foundation
writes do not imply calibrated RF, a locked/tracked PLL, valid TX timing, or
on-air capability.

## 7. Strongest honest Iteration 4 endpoint

The source can justify a conservative, **non-operational** pure control model
with this boundary:

```text
model-only Ready
  -> retain typed RX, TX, or TX-plus-ACK-RX DMA leases
  -> emit a non-executable ordered intent
  -> consume one already sampled and internally consistent event batch
  -> Pending, or Deferred while retaining every DMA lease
  -> external completion proof + one resource-reclaim callback
  -> exactly one explicit next-policy choice
```

The ordered intent may name only the reviewed subset: receive, direct transmit,
CCA-gated transmit, standalone CCA, and standalone ED. Its steps are
requirements for state-specific quiescence, static-policy refresh, typed DMA
address publication, optional ED-duration configuration, and a command name.
There is no execute method and no target constructor justified by this review.

The pure batch boundary may require abort reasons and CCA/ED sidebands up front,
reject `CLOCK_COUNT_MATCH` and all unsupported bits transactionally, retain the
exact actor on rejection, represent `TX_DONE -> RX_ACK -> ACK_RX_DONE`, and
withhold its unique ready state until one deferred-next choice and external DMA
reclamation have completed. This is stronger ownership than an untyped event
callback, but narrower behavior than the vendor ISR.

It must not:

- read, acknowledge, or enable `EVENT_STATUS`;
- route an interrupt, issue `STOP` or an operation command, or write DMA
  addresses;
- manufacture target DMA completion from an event name;
- claim the full twelve-state state machine, automatic/Enhanced ACK TX, test
  mode, security, coexistence, sleep, timed TX/RX, or Timer1 behavior;
- call PHY/BTBB/PLL routines or claim receive-ready, transmit-ready, RF-ready,
  ISR-equivalent, or on-air status.

## Research and HIL gates

1. **Event acknowledgement:** obtain an authoritative S31 access class or use
   HIL to latch two independent bits, acknowledge one, and verify selected-bit
   clearing, preservation of the other bit, distinct-bit and same-bit
   concurrent arrival, level-line retrigger, reset image, and stale-event
   teardown. Only then may the IRQ route become active.
2. **State-by-state stop:** exercise `RX`, `TX_ACK`, `TX_ENH_ACK`, `TX_CCA`,
   `TX`, `RX_ACK`, `ED`, and `CCA`, with a terminal edge immediately before,
   during, and after `STOP`. Measure a bounded hardware-idle predicate and prove
   that neither TX nor RX DMA can access storage after it. Investigate
   `TEST_TX` separately because the source stop switch has no arm for it.
3. **Sequential batches:** force co-latched `TX_DONE + ACK_RX_DONE` and
   `RX_DONE + ACK_TX_DONE`, then record callback order, private state observed
   by each callback, and exactly one final next operation. Add done/abort and
   timer/terminal collision cases to determine which combinations are legal
   rather than inferring legality from individual bits.
4. **Timer1 and ETM:** verify callback execution core/interrupt context,
   clear-before-callback rescheduling, lock/reentrancy behavior, wraparound,
   zero-duration and expired windows, and stop/SFD/window-end races. Keep timed
   operations out of production until these cases have bounded outcomes.
5. **Clock-count event:** determine how bit 10 is generated, why the vendor
   initialization enables it, how it is acknowledged, and its intended state
   transition. Keep it masked and fail closed until a handler contract exists;
   apply the same gate to unnamed bits 7 and 13.
6. **RF and shared ownership:** recover or replace BTBB first-user setup, PHY
   calibration/wakeup, and PLL tracking using public, source-legal logic. Test
   cold start, warm start, and already-owned Wi-Fi/Bluetooth combinations, plus
   last-user release, without copying vendor tables.
7. **Timing and power:** establish `ieee802154_txon_delay_set()` postconditions
   and a legal channel-power contract; validate all channels and power limits on
   hardware before any TX-ready claim.
8. **Operational binding:** only after gates 1, 2, and 6, add a single owner
   that joins command issue, active IRQ acknowledgement, and DMA leases. Require
   real maximum-length RX/TX, ACK/no-ACK, CCA busy/clear, ED, abort, recovery,
   and repeated-operation HIL before upgrading this **INCOMPLETE** verdict.
