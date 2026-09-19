//! Destruction of one secure Host epoch, independent of physical radio release.
//!
//! The application owns the bond store and comparison sequence across calls.
//! After all Host producers are dropped, Reset is issued through the returned
//! Controller while its event reader continues. No discarded ACL is published
//! to a new Host. Reset completion is not physical retirement: the caller must
//! still close HCI and retire IRQ, timer, PHY and platform owners.

use super::{bonds::BondStore, comparison::NumericComparison, gatt};
use bt_hci::cmd::{SyncCmd, controller_baseband::Reset};
use core::{convert::Infallible, future::Future};
use embassy_futures::select::{Either, select};
use trouble_host::{BleHostError, Controller, Stack, prelude::DefaultPacketPool};

#[cfg(test)]
mod tests;

/// Why the old Host stopped. A failure never becomes a successful stop request.
#[derive(Debug)]
pub enum Cause<C, S> {
    Requested,
    Application(gatt::RunError<C, S>),
    Host(Result<(), BleHostError<C>>),
}

/// Reset failures keep the original Controller available for quarantine.
#[derive(Debug)]
pub enum ResetError<C> {
    Command(bt_hci::cmd::Error<C>),
    Receive(C),
}

/// The old software epoch has ended; physical ownership remains with the caller.
#[must_use = "Reset completion does not retire physical resources"]
pub struct Exit<C: Controller, S> {
    pub controller: C,
    pub cause: Cause<C::Error, S>,
    pub reset: Result<(), ResetError<C::Error>>,
}

/// Run one Host and secure application until explicitly stopped or failed.
///
/// Hardware must be polled concurrently. Retain `store` and `comparison` outside
/// this call; reconstruct a fresh Host after checked physical retirement/restart.
/// Once polled, drive this consuming future to completion. Cancelling it does
/// not release the hardware runner or prove that Reset completed. The caller
/// chooses any deadline and must retain physical owners if it expires.
pub async fn run<C: Controller, S: BondStore>(
    stack: Stack<'_, C, DefaultPacketPool>,
    store: &mut S,
    comparison: &NumericComparison,
    stop: impl Future<Output = ()>,
    observe: impl FnMut(gatt::Observation),
) -> Exit<C, S::Error> {
    let cause = {
        let mut runner = stack.runner();
        let application = async {
            match select(gatt::run(&stack, store, comparison, observe), stop).await {
                Either::First(Err(error)) => Cause::Application(error),
                Either::First(Ok(never)) => match never {},
                Either::Second(()) => Cause::Requested,
            }
        };
        match select(application, runner.run()).await {
            Either::First(cause) => cause,
            Either::Second(result) => Cause::Host(result),
        }
    };
    // The application, prompt, ATT objects and all Host runners are gone before
    // returning the Controller. No old producer can enqueue behind this Reset.
    let controller = stack.into_controller();
    let reset = reset(&controller).await;
    Exit {
        controller,
        cause,
        reset,
    }
}

async fn reset<C: Controller>(controller: &C) -> Result<(), ResetError<C::Error>> {
    let receive = async {
        loop {
            let mut buffer = controller.alloc_buf().map_err(ResetError::Receive)?;
            // ExternalController dispatches command responses inside read().
            // Other old-epoch packets are drained, never delivered to a new Host.
            controller
                .read(&mut buffer)
                .await
                .map_err(ResetError::Receive)?;
            embassy_futures::yield_now().await;
        }
        #[allow(unreachable_code)]
        Ok::<Infallible, ResetError<C::Error>>(unreachable!())
    };
    match select(Reset::new().exec(controller), receive).await {
        Either::First(result) => result.map_err(ResetError::Command),
        Either::Second(Err(error)) => Err(error),
        Either::Second(Ok(never)) => match never {},
    }
}
