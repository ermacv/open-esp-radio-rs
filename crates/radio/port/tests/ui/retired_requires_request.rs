#![no_std]
use oer_radio_port::{DriverExit, Retire, RetireProof};
pub async fn retire(retire: Retire<'_>) -> DriverExit<(), ()> {
    #[cfg(ui_reject)]
    let proof = RetireProof { private: () }; // ui-error: E0451
    #[cfg(not(ui_reject))]
    let proof = retire.requested().await;
    DriverExit::Retired { state: (), proof }
}
