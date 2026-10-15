#![no_std]
use oer_ieee80211_lower_mac::{InFlight, TxOutcome};
pub fn finish<C, T, M>(operation: InFlight<C, T, M>, outcome: TxOutcome) {
    let terminal = operation.complete(outcome);
    #[cfg(ui_reject)]
    let _again = operation.complete(outcome); // ui-error: E0382
    drop(terminal);
}
