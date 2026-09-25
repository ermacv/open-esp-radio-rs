# Wi-Fi DMA ownership

This crate owns stable descriptors and packet backing. MAC owners select queues,
prepare transmission parameters and interpret completion; the runtime drives
their interrupt and deadline transitions. Ordinary TX uses internal DMA SRAM.
The diagnostic PSRAM and aggregate paths have separate admission contracts.

## Ordinary TX handoff and reuse

The upper ordinary-TX owner checks [protection policy](../mac/src/tx/protection.rs)
before reserving or publishing DMA. An ordinary exchange requiring RTS/CTS or
CTS-to-Self returns `PhysicalPublicationUnverified` until its physical
publication and completion contract is qualified. In particular,
an HT Nonmember group exchange can stop the connected runner with
`HardwareFailure` during AP-loss recovery. That admission failure is distinct
from a stuck DMA transaction; it does not establish a detach or reuse failure.

The reviewed queue control has separate software RTS and CTS requests. Ordinary
descriptor-bound preparation replaces both requests with the PPDU's explicit
mode while the queue is idle. The RTS helper preserves CTS and spacing; cold HE
initialization clears CTS and retains RTS and spacing. The RTS request, optional
HE byte-threshold publication, threshold disable and cold-init reset follow the
reviewed [queue argument provenance](../../../../../../registers/esp32s31/evidence/vendor-libpp.toml);
they have no native compiled vendor comparison. The register transactions do not establish a CTS frame, its NAV duration, its relation to the protected
MPDU, or completion ownership. The ordinary API therefore still rejects a
protection-required exchange before DMA publication.

[`PinnedTxDmaStorage`](src/tx_storage.rs) permanently retains its allocation.
Dropping the movable owner does not free or detach hardware-visible memory.
[`TxSlot`](../mac/src/tx.rs) adds the active queue and generation cookie;
[`OrdinaryTxOwner`](../src/ordinary_tx.rs) retains retry and deadline state.

| State | Permitted operation | Memory ownership |
| --- | --- | --- |
| `Free` | Encode the next MPDU, then reserve | CPU may mutate packet backing |
| `Reserved` | Prepare queue registers or cancel reservation | Descriptor and payload remain retained; hardware queue is not published |
| `HardwareOwned` | Observe completion or begin a qualified abort | No safe buffer mutation or reservation cancellation |
| `Completed` | Detach the exact queue and descriptor | Completion alone does not permit reuse |
| `ResetRequired` | Return failure to the radio lifecycle owner | No local release or reuse, including after a late completion |

Reservation prepares the descriptor, including its hardware ownership word.
That word alone is not the queue publication edge. `TxDmaPublication::commit`
records software `HardwareOwned` before passing the start authority to the PAC.
`start_bound_mac_tx` checks the prepared descriptor head; the production
`start_prepared_mac_tx` fences device access and updates the existing control
word's ENABLE and VALID fields without reconstructing other queue fields.

The PAC checks the queue completion event, samples result registers and trigger
flow, then acknowledges completion. The slot returns the decoded completion and retains its
generation. Normal turnover disables the retained queue, fences, and reads back
ENABLE and VALID. Its scoped detach proof also identifies the descriptor head.
Only that proof permits `Completed -> Free`. A failed readback quarantines the
slot even if the transmission status reported success. A stale software cookie
cannot release a later generation; this does not identify an untagged stale
hardware event after a radio reset, which remains an IRQ-epoch responsibility.

Timeout abort has a separate force-CCA/settle/invalidate/disable sequence.
An executor deadline without the required hardware event quarantines storage.
Neither a timeout nor cancellation is an implicit completion. Per-queue turnover
is not proof that all radio DMA has stopped during global teardown.

## Vendor basis and executable boundary

The reviewed [ordinary queue provenance](../../../../../../registers/esp32s31/evidence/vendor-libpp.toml)
names the authenticated archive and complete function bodies. The production
operations mirror three finite vendor queue operations:

- `hal_mac_txq_enable`: the publication prefix through the call to `GetAccess`,
  with explicit production device fences. The rest of the vendor access/HE
  bookkeeping and statistics is outside this boundary.
- `hal_mac_clr_txq_state`: completion selector two, queues zero through three,
  preserving the existing clear-register image.
- `hal_mac_txq_disable`: the complete leaf clearing ENABLE and VALID while
  retaining the descriptor and other control fields.

These operations have no native compiled vendor comparison. They do not
establish physical DMA quiescence or memory-ordering sufficiency on silicon.

The inspected `lmacProcessTxComplete` body reads the completion result and
trigger-flow state before calling the completion acknowledgement. Full result
decoding, descriptor preparation and the enclosing release/abort root have no
compiled vendor comparison either. Their absence remains visible
in the [RX/TX capability](../../../../../../qualification/catalog/esp32s31/wifi-phy.toml).

The [storage tests](src/tx_storage/tests.rs) exercise publication authority and
quarantine. The [MAC ownership tests](../mac/tests/cases/tx_ownership.rs) exercise
repeated generations, retained payload, stale release and failed detach. These
are host models with explicit completion/detach inputs. HIL AP-loss/reconnect
exercises ordinary authentication and association TX through the real owner;
its functional result does not measure the exact last DMA access or inject a
failed detach readback.
