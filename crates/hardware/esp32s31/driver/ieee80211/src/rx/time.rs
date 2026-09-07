//! Timer capability for finite RX walker ownership transitions.

use core::future::Future;

/// Executor edge between walker publication and its first live observation.
pub trait RxFrontierDelay {
    fn after_micros(micros: u32) -> impl Future<Output = ()>;
}
