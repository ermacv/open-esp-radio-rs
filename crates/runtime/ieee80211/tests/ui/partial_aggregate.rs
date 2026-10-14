#![no_std]
use oer_ieee80211_lower_mac::{InFlight, PhaseTypes, TxCompleted, TxOutcome};
use oer_ieee80211_runtime::lower_mac_owner::model::{ModelSubframes, ModelTypes};
use oer_radio_port::Slot;

type Ticket<'id> = Slot<
    'id,
    <ModelTypes as PhaseTypes<'id>>::TicketKind,
    <ModelTypes as PhaseTypes<'id>>::TicketPath,
>;
pub fn finish<'id>(
    operation: InFlight<(), Ticket<'id>, ModelSubframes<'id>>,
    outcome: TxOutcome,
) -> TxCompleted<'id, ModelTypes> {
    #[cfg(ui_reject)]
    {
        let parts = operation.complete(outcome).into_parts();
        let one = parts.payload.into_iter().next().unwrap();
        TxCompleted::Aggregate(InFlight::admit(parts.context, parts.ticket, one).complete(outcome)) // ui-error: E0271
    }
    #[cfg(not(ui_reject))]
    TxCompleted::Aggregate(operation.complete(outcome))
}
