//! Channel-zero interrupt wakeup and race-closed Future polling.

use super::{
    registers::{disable_channel_interrupts, enable_channel_interrupts, terminal_status},
    transfer::{AxiGdmaMem2MemReport, AxiGdmaMem2MemTransferError, AxiGdmaMem2MemTransferOwner},
};
use atomic_waker::AtomicWaker;
use core::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

#[unsafe(link_section = ".flash.critical.bss.axi_gdma_mem2mem")]
static CHANNEL0_WAKER: AtomicWaker = AtomicWaker::new();

#[inline(never)]
#[unsafe(link_section = ".rwtext.axi_gdma_mem2mem")]
pub(super) extern "C" fn channel0_interrupt() {
    disable_channel_interrupts();
    CHANNEL0_WAKER.wake();
}

impl Future for AxiGdmaMem2MemTransferOwner<'_, '_, '_> {
    type Output = Result<AxiGdmaMem2MemReport, AxiGdmaMem2MemTransferError>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        assert!(this.active, "AXI-GDMA transfer polled after completion");

        if let Some((rx_raw, tx_raw)) = terminal_status() {
            return Poll::Ready(this.finish(rx_raw, tx_raw));
        }

        CHANNEL0_WAKER.register(context.waker());
        enable_channel_interrupts();

        // Re-check after publishing the waker and enabling the peripheral
        // sources. This closes both completion-before-registration and
        // completion-between-registration-and-enable races.
        if let Some((rx_raw, tx_raw)) = terminal_status() {
            disable_channel_interrupts();
            Poll::Ready(this.finish(rx_raw, tx_raw))
        } else {
            Poll::Pending
        }
    }
}
