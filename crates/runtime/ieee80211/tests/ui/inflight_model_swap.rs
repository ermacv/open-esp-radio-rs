#![no_std]
use oer_ieee80211_lower_mac::{Completed, InFlight, PhaseTypes, TerminalAccess, TxOutcome};
use oer_ieee80211_runtime::lower_mac_owner::model::{ModelMpdu, ModelTypes};
use oer_radio_port::Slot;
type Kind<'id> = <ModelTypes as PhaseTypes<'id>>::TicketKind;
type Path<'id> = <ModelTypes as PhaseTypes<'id>>::TicketPath;
type Ticket<'id> = Slot<'id, Kind<'id>, Path<'id>>;

pub fn swap<'id, C>(
    a: InFlight<C, Ticket<'id>, ModelMpdu<'id>>,
    b: InFlight<C, Ticket<'id>, ModelMpdu<'id>>,
    out: TxOutcome,
    _access: &mut TerminalAccess<'id, Kind<'id>, Path<'id>>,
) -> (Completed<C, Ticket<'id>, ModelMpdu<'id>>, Completed<C, Ticket<'id>, ModelMpdu<'id>>) {
    #[cfg(ui_reject)]
    {
        // Even the consumer permission cannot re-admit raw mixed components.
        let a = a.complete(out).into_parts(_access);
        let b = b.complete(out).into_parts(_access);
        (
            InFlight::admit(a.context, b.ticket, b.payload).complete(out), // ui-error: E0624
            InFlight::admit(b.context, a.ticket, a.payload).complete(out), // ui-error: E0624
        )
    }
    #[cfg(not(ui_reject))]
    (a.complete(out), b.complete(out))
}
