#![no_std]
#![forbid(unsafe_code)]

//! ESP32-S31 Wi-Fi station composition.
//!
//! This crate composes chip PHY/MAC-backend owners for station operation. It must not
//! depend on Embassy, a network stack, board allocation or HIL protocols.

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod test_support {
    use core::{
        future::Future,
        pin::pin,
        task::{Context, Poll},
    };

    pub(crate) fn block_on<F: Future>(future: F) -> F::Output {
        let mut future = pin!(future);
        let mut context = Context::from_waker(core::task::Waker::noop());
        loop {
            match future.as_mut().poll(&mut context) {
                Poll::Ready(output) => return output,
                Poll::Pending => std::thread::yield_now(),
            }
        }
    }
}

pub mod association;
pub mod attempt;
#[cfg(target_arch = "riscv32")]
pub mod channel;
pub mod connected_control;
pub mod connected_control_hardware;
pub mod connected_rx;
pub mod control_tx;
pub mod join;
pub mod peer;
mod peer_policy;
pub mod scan;
pub mod scan_tx;
pub mod single_mpdu_tx;
pub mod tx_epoch;
pub mod wpa2;
