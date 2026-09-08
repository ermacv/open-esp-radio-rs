//! Read-only control-path counts; TX completion is distinct from RUN publication.

use core::cell::Cell;
use critical_section::Mutex;

#[derive(Clone, Copy, Debug)]
pub struct PeripheralControlDiagnostics {
    pub received: u32,
    pub discarded: u32,
    pub control: u32,
    pub queued: u32,
    pub completed: u32,
    pub last_opcode: Option<u8>,
}
static STATE: Mutex<Cell<PeripheralControlDiagnostics>> =
    Mutex::new(Cell::new(PeripheralControlDiagnostics {
        received: 0,
        discarded: 0,
        control: 0,
        queued: 0,
        completed: 0,
        last_opcode: None,
    }));
pub fn snapshot() -> PeripheralControlDiagnostics {
    critical_section::with(|cs| STATE.borrow(cs).get())
}
#[cfg(target_arch = "riscv32")]
pub(crate) fn record_received<const N: usize>(
    batch: &oer_esp32s31_bluetooth_memory::LeReceivedBatch<N>,
    completed: bool,
) {
    critical_section::with(|cs| {
        let mut state = STATE.borrow(cs).get();
        state.received = state.received.saturating_add(batch.len() as u32);
        state.discarded = state
            .discarded
            .saturating_add(batch.discarded_count() as u32);
        state.completed = state.completed.saturating_add(u32::from(completed));
        for index in 0..batch.len() {
            let pdu = batch.packet(index).expect("bounded RX batch").as_bytes();
            if pdu[0] & 3 == 3 {
                state.control = state.control.saturating_add(1);
                state.last_opcode = pdu.get(2).copied();
            }
        }
        STATE.borrow(cs).set(state);
    });
}
#[cfg(target_arch = "riscv32")]
pub(crate) fn record_enqueued() {
    critical_section::with(|cs| {
        let mut state = STATE.borrow(cs).get();
        state.queued = state.queued.saturating_add(1);
        STATE.borrow(cs).set(state);
    });
}
