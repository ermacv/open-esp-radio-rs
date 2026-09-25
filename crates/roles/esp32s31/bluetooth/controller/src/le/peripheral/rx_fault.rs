//! One-shot corruption of a received active-data MIC for diagnostic firmware.
//!
//! This changes the copied RX input before the production authentication step;
//! it does not change DMA, keys, counters or the decoder's result. It is not
//! evidence that a corrupt packet crossed the RF link. The diagnostic owner
//! must disarm on disconnect/cancellation so an unused request cannot affect
//! the next connection. This module is absent without `rx-fault-injection`.

use core::cell::Cell;
use critical_section::Mutex;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Snapshot {
    pub armed: bool,
    pub injections: u32,
}

impl Snapshot {
    #[cfg(any(target_arch = "riscv32", test))]
    fn inject(&mut self, header: u8, ciphertext: &mut [u8]) {
        if self.armed && matches!(header & 3, 1 | 2) && ciphertext.len() > 4 {
            // Keep the header, ciphertext and all other MIC octets intact.
            *ciphertext.last_mut().expect("nonempty MIC") ^= 1;
            self.armed = false;
            self.injections = self.injections.saturating_add(1);
        }
    }
}

static STATE: Mutex<Cell<Snapshot>> = Mutex::new(Cell::new(Snapshot {
    armed: false,
    injections: 0,
}));

/// Arm or cancel the next active encrypted data packet, preserving evidence.
/// The diagnostic owner calls this before connection admission and disarms
/// when that connection ends. Control and empty packets never consume it.
pub fn configure(armed: bool) {
    critical_section::with(|cs| {
        let mut state = STATE.borrow(cs).get();
        state.armed = armed;
        STATE.borrow(cs).set(state);
    });
}

pub fn snapshot() -> Snapshot {
    critical_section::with(|cs| STATE.borrow(cs).get())
}

/// Called only for the active encrypted receive mode, after duplicate filtering.
#[cfg(any(target_arch = "riscv32", test))]
pub(super) fn inject_active_data(header: u8, ciphertext: &mut [u8]) {
    critical_section::with(|cs| {
        let mut state = STATE.borrow(cs).get();
        state.inject(header, ciphertext);
        STATE.borrow(cs).set(state);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fault_preserves_control_and_empty_packets_then_changes_one_mic_once() {
        let mut state = Snapshot {
            armed: true,
            injections: 0,
        };
        let mut control = [0x35; 5];
        state.inject(3, &mut control);
        assert_eq!(control, [0x35; 5]);
        state.inject(1, &mut []);
        state.inject(2, &mut [0; 4]);
        assert!(state.armed);
        let mut packet = [0x42; 9];
        state.inject(0x0e, &mut packet);
        assert_eq!(
            packet,
            [0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x43]
        );
        assert_eq!(
            state,
            Snapshot {
                armed: false,
                injections: 1
            }
        );
        state.inject(2, &mut packet);
        assert_eq!(packet[8], 0x43);
        assert_eq!(state.injections, 1);
    }
}
