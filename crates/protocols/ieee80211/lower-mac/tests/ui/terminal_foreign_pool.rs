#![no_std]
use oer_ieee80211_lower_mac::{Completed, TerminalAccess, TerminalParts};
use oer_radio_port::{Child, Root, Slot};
pub fn unpack<'id, C, K, M>(
    terminal: Completed<C, Slot<'id, K, Child<Root, 0>>, M>,
    own: &mut TerminalAccess<'id, K, Child<Root, 0>>,
    foreign: &mut TerminalAccess<'id, K, Child<Root, 1>>,
) -> TerminalParts<C, Slot<'id, K, Child<Root, 0>>, M> {
    #[cfg(ui_reject)]
    return terminal.into_parts(foreign); // ui-error: E0308
    #[cfg(not(ui_reject))]
    terminal.into_parts(own)
}
