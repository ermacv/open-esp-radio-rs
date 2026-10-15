#![no_std]
use oer_radio_port::{Retire, RetireProof};
pub async fn retire(retire: Retire<'_>) -> RetireProof {
    let first = retire.requested();
    #[cfg(ui_reject)]
    let _ = retire.requested(); // ui-error: E0382
    first.await
}
