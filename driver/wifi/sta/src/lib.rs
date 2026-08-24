#![no_std]
#![forbid(unsafe_code)]

//! Executor- and chip-independent Wi-Fi station MLME and policy.
//!
//! Protocol crates own scan, IEEE 802.11 and WPA state. Chip/runtime adapters
//! own concrete hardware and timer operations. This crate owns finite
//! candidate-scan ordering plus the outer attempt, reconnect and backoff policy
//! while preserving one caller-defined resource owner across every
//! asynchronous edge.

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

pub mod join;
pub mod link_monitor;
pub mod power_save;
pub mod request;
pub mod scan;
pub mod station;
pub mod twt;
