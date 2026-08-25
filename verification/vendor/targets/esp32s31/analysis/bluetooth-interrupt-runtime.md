# ESP32-S31 Bluetooth interrupt runtime

Verdict: **PARTIAL**. The primary source-124 handler now has a lossless typed
prefix and disposition: its four baseline groups are fatal assertion lanes,
including the two conditional diagnostic captures, while its dynamic suffix
retains two temporally distinct scheduler-state observations and a coalesced
deferred-work contract. NRT source meanings, shared live ISR ownership and the
scheduler-list consumer remain unresolved, so neither CPU route may yet form a
live production interrupt epoch.

This review separates silicon behavior from the internal callback and RTOS
architecture of the reference Controller. The Rust driver does not reproduce
the vendor callback registry or FreeRTOS event queue.

## Pinned inputs

- public Controller archive:
  [`espressif/esp32s31-bt-lib@7f20740dd66ee774ffce5db0b55507892551aa31`](https://github.com/espressif/esp32s31-bt-lib/tree/7f20740dd66ee774ffce5db0b55507892551aa31),
  `libbtdm_common.a` SHA-256
  `fa22a8a2aca48b807addda2bbad78868d6774c82bcdeb8090f9140f6cbccd099`;
- public BLE archive at the same revision, `libble_app.a` SHA-256
  `62dbe7216619d1f1e3dcd51233d91b211add15c7c746851af0be6a632cdae195`;
- public OSAL source:
  [`btdm_osal_freertos.c`](https://github.com/espressif/esp-idf/blob/aeab6dcfbeb44aba4b1f8ed102e3086172833153/components/bt/porting_btdm/controller/btdm_common/src/btdm_osal_freertos.c#L139-L211),
  SHA-256
  `436d009f67fbe7c83fa367a6b80dbf87316727605e751febc03c5118a669b5c8`.

Complete function bodies were reviewed from temporary, hash-verified public
artifacts. Neither object files nor disassembly products are repository
inputs. The table below records only the distilled control and MMIO facts.

| Body | Size | Recovered contract |
| --- | ---: | --- |
| `r_sym_bt_R9GZfnUbtn7k6mHtoZbv` | 166 bytes | Primary ISR: masked sample, shared W1C acknowledgement, selector 0, dynamic scheduler suffix, selector 1. |
| `r_sym_bt_hkluARKZBNTcJqMeSNys` | 190 bytes | Primary fault prefix: tests exactly bank-zero source 15 and bank-one sources 9, 8 and 12 in that order; source 9 reads `0x2010_11c8/11cc` and source 12 reads `0x2010_1070` before asserting. |
| `r_sym_bt_3DcjgYC1ikQrP7jNyXGR` | 58 bytes | Walks a linked callback list in registration order and stops when one callback returns nonzero. |
| `r_sym_bt_MDZbajeLxsB0cvuBOREd` / `r_sym_bt_2VbAPsx8lRNZdbVZTEOe` | complete leaves | Read `0x2010_105c/1068`, then copy both images to `0x2010_1058/1064`. |
| `r_sym_bt_6lAYUFKOuBLyOZ6Kvsv5` / `r_sym_bt_Ak3CRkSbyZRhUlneqclG` / `r_sym_bt_DOVkQWJHjeuid8jcS9Bq` | 24 / 24 / 16 bytes | Enable, disable and W1C the exact dynamic images `0x1820_0000` and `0x0000_0008`. |
| `r_sym_bt_37HcX0qW6j1XVtKakUIG` / `r_sym_bt_zczKhmPr5kLPCXpBc7GE` | 80 / 92 bytes | Decode `SCHEDULER_STATE`; the boolean consumed by deferred-work construction is exactly bit 31 AND bit 29. |
| `r_sym_bt_iEsFo1nbR5S71P2lMKhY` / `r_sym_bt_gs5GeSH15pdzrMDbb7oK` | 90 / 78 bytes | Build one static deferred event, optionally set its one-bit argument, and optionally publish the decoded state through selector 4. |
| `r_sym_bt_uNi9OHmE7XdXfGqTelU5` | 112 bytes | Consume and clear the marker, drain scheduler work, then publish selector `0x8000_0001`; this is a drain event, not one callback per hardware edge. |
| `r_sym_ble_ywjh0f9yjTBeI7XgS5da` | 74 bytes | NRT ISR: raw sample at `0x2010_1340/1348`, shared W1C acknowledgement and selector `0x8000_0000`. |

## Exact primary suffix

The complete primary handler first preserves the implemented
sample/sample/ack/ack order. Its next complete function treats precisely the
baseline enable groups as fault/assert paths:

| Pending source | Diagnostic capture before assertion |
| --- | --- |
| bank 0 source 15 | none |
| bank 1 source 9 | complete words `IRQ_DIAGNOSTIC_DETAIL_0/1` at `0x2010_11c8/11cc` |
| bank 1 source 8 | none |
| bank 1 source 12 | complete `IRQ_DIAGNOSTIC_STATE` at `0x2010_1070` |

This is not a Link-Layer completion group. The restricted PAC therefore
returns one `BluetoothPrimaryInterruptEpoch` containing the lossless masked
observation plus conditional `BluetoothPrimaryFaultEvidence`. The Bluetooth
classifier gives any fault lane precedence over simultaneous dynamic bits and
returns `BluetoothPrimaryControllerFault`; a future live owner must retain it,
skip ordinary LL work and enter fail-stop/quiesce. Rust does not reproduce the
vendor assert routine in hard-interrupt context.

Only after that prefix does the reference handler dispatch selector 0 with the
two-word snapshot, execute the following dynamic branch, then dispatch selector
1 with the same snapshot.

Dynamic classification is positional because Link-Layer names are not yet
proven:

| Pending image | First work input | State publication requested | Extra reference gate |
| --- | ---: | ---: | --- |
| bank 1 source 3 | bank 0 source 27 OR 28 | yes | yes |
| no bank 1 source 3; bank 0 source 27 OR 28 | yes | bank 0 source 21 | no |
| only bank 0 source 21 | no | yes | no |
| none of these sources | no work | no | no |

Bank 1 source 3 has precedence over the bank-zero branch. Its reference gate
reads `SCHEDULER_STATE` at `0x2010_107c`; when bit 31 is clear it writes zero
to `SCHEDULER_REFERENCE` at `0x2010_1078` and dispatches selector 6. Any
dynamic branch then makes a later, independent `SCHEDULER_STATE` read. The
derived reference state is `bit31 && bit29`. Deferred work is marked only when
the first work input and that derived state are both true. Selector 4 receives
the same derived state when the table requests state publication.

The two reads cannot be folded into one observation: the reference clear and
selector-6 software path occur between their positions. The Rust classifier
therefore uses distinct reference-gate and work-observation types.

## Event multiplicity and RTOS-free replacement

The exact public OSAL refuses a second insertion while the same static event
is queued. The marker assignment occurs before insertion, so a later marked
edge upgrades an already queued ordinary event, and a later ordinary edge
cannot clear a marker. Dequeue clears the queued flag before the event handler
runs. Consequently the required deferred contract is:

1. the first publication opens a pending epoch and wakes the sole worker;
2. repeated publications coalesce into that epoch;
3. `marked` is sticky across every coalesced publication;
4. dequeue atomically consumes pending plus marker;
5. a racing publication after dequeue opens a new epoch and emits a new wake.

`BluetoothSchedulerWakeCell` implements this contract with one `AtomicU8`.
It is not an async primitive by itself: the platform must still install a
lost-wake-safe waker registration and the Controller worker must drain the
real scheduler list before considering the epoch complete.

The linked callback selectors are boundaries between closed vendor software
components, not ESP32-S31 hardware ABI. An open Link Layer may use its own
typed events. It must nevertheless preserve every proven MMIO ordering edge,
the sticky marked-work distinction, and any behavior later shown to be
required by the scheduler-list consumer.

## NRT default-lifecycle result

The NRT ISR belongs to callback-manager ID `0x4003` and synchronously dispatches
selector `0x8000_0000` after raw sample/sample/ack/ack. Complete relocation and
caller review across the pinned BLE, common and classic Controller archives
found no registration into manager `0x4003`; the manager is initialized with
an empty list in the default lifecycle. Other managers do have concrete
registrations, so absence here is not inferred merely from stripped names.

This establishes an acknowledge-only NRT path for the pinned default software
lifecycle, not a silicon guarantee that source 133 can be removed or that its
raw bits have no meaning under future features. The open ISR must retain the
opaque observation and shared-clear ordering. It must not invent LL wakes from
those bits until a feature-specific producer/consumer path or HIL establishes
them.

## Remaining publication blockers

- feature-specific meanings and mask/re-arm policy for NRT raw status beyond
  the pinned default lifecycle's acknowledge-only callback graph;
- shared same-core storage that combines the interrupt-bank owner with narrow
  scheduler reference/state access without exposing concurrent PAC aliases;
- exact scheduler-list drain, abort and completion behavior behind the static
  event handler;
- executor-neutral waker registration with a register-then-recheck poll
  protocol and a quiescent teardown barrier;
- compiled-production trace and HIL for simultaneous, repeated and teardown
  interrupt epochs.

Until these are closed, the dynamic masks remain disabled in production and
the typed ESP-HAL routes remain private.
