#![no_std]
use oer_ieee80211_port::{Disabled, Ended, Owner, PortBackend};

pub fn cannot_forge<'id, B: PortBackend>(owner: Owner<'_, 'id, B, Disabled>) -> Ended<'id, B> {
    #[cfg(ui_reject)]
    { drop(owner); Ended::fail_stop() } // ui-error: E0624
    #[cfg(not(ui_reject))]
    owner.fail_stop()
}
