#![no_std]
use oer_ieee80211_lower_mac::{Completed, InFlight, TxOutcome};

pub fn finish<C, T, M>(operation: InFlight<C, T, M>, outcome: TxOutcome) -> Completed<C, T, M> {
    #[cfg(ui_reject)]
    let terminal = Completed { operation, outcome }; // ui-error: E0451
    #[cfg(not(ui_reject))]
    let terminal = operation.complete(outcome);
    terminal
}
