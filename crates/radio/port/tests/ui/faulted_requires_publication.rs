#![no_std]
use oer_radio_port::{DriverExit, PoisonProof, PoisonState};
pub fn fault(state: &mut PoisonState<u8>) -> DriverExit<(), u8> {
    #[cfg(ui_reject)]
    let poisoned = PoisonProof { private: () }; // ui-error: E0451
    #[cfg(not(ui_reject))]
    let (_, poisoned) = state.publish(7);
    DriverExit::Faulted {
        state: (),
        cause: 7,
        poisoned,
    }
}
