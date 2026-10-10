#![no_std]
#![forbid(unsafe_code)]

//! Executor-independent service loop of the Bluetooth LE Controller.
//!
//! [`serve`] moves packets and radio work between three owners: the
//! Controller end of an in-process HCI transport, the sans-IO
//! [`LeController`] core and one [`LeRadioPort`], which the protocol
//! package `oer-bluetooth-radio` declares. It publishes the core's
//! queued packets, takes the next Host command when the core is ready for
//! one, submits the core's radio requests one at a time and feeds every
//! radio outcome back. A refused request is retried after
//! [`REFUSED_RETRY_DELAY`] or the next outcome or command; a request
//! refused as [`RequestError::Unsupported`] lies outside the radio's
//! capabilities and is never retried on a timer: the loop asks the core
//! again only after an outcome, a command or Host data changed its plan. Whenever the
//! core's [`RadioActivity`] changes, the loop reports it to the port before
//! doing anything else.
//!
//! The loop enables the port when it starts, and cancels a scheduled event
//! through the port's cancellation when the core asks for it.
//!
//! The loop owns no memory of its own beyond one command buffer and spawns
//! nothing; the caller polls it on a task of its choice, beside the radio
//! backend's own runner. It is the port's one event consumer. It ends when
//! the transport closes or fails, when the radio backend is not installed
//! or its clock cannot be read, or when the port is poisoned
//! ([`ServeExit::Poisoned`] with the backend's cause). The port reserves
//! every event's end and a connection event's data when it admits the
//! event, so an [`EventsLost`] stands for advertising and scan reports only
//! and the loop goes on: the core's roles still account every event by its
//! end. Host ACL data enters the core while it
//! has room for a packet; until then commands pass queued data.
//!
//! A required schedule outside the current radio epoch ends through
//! [`ServeExit::Planning`] before another submission or retry. The caller keeps
//! the core for managed physical teardown and terminal accounting. A capture
//! timing failure belongs to its identified event: correct PDUs and TX ACKs
//! enter the core, no invalid stamp establishes a time reference, and this same
//! service continues after its terminal outcome.

#[cfg(test)]
extern crate std;

use embassy_futures::select::{Either4, select4};
use embassy_sync::blocking_mutex::raw::RawMutex;
use oer_bluetooth_controller::{LeController, PlanningError, RadioWork};
use oer_bluetooth_hci::HostToControllerFrame;
use oer_bluetooth_hci_transport::{HciChannelError, InProcessHciControllerTransport};
use oer_bluetooth_radio::{
    ClockError, EventsLost, LeRadioPort, LifecycleCommand, LifecycleError, Poisoned, RadioActivity,
    RequestError,
};
use oer_time::{Duration, Instant, Timer};

/// Delay before asking the core again after the radio refused a request.
pub const REFUSED_RETRY_DELAY: Duration = Duration::from_millis(1);

/// Why [`serve`] ended; `F` is the port's poison cause.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServeExit<F> {
    /// The transport closed.
    Closed,
    /// A required radio continuation failed; retained owners require managed stop.
    Planning(PlanningError),
    /// The transport refused a packet.
    Transport(HciChannelError),
    /// The port refused to enable.
    Lifecycle(LifecycleError),
    /// The radio clock could not be read.
    Clock(ClockError),
    /// The port is poisoned.
    Poisoned(Poisoned<F>),
}

impl<F> From<Poisoned<F>> for ServeExit<F> {
    fn from(poisoned: Poisoned<F>) -> Self {
        Self::Poisoned(poisoned)
    }
}

/// Serve the Host through `transport` with `core` over `radio` until the
/// transport or the radio ends the service. Retries after a refusal wait on
/// `timer`.
pub async fn serve<
    M,
    P,
    const H2C: usize,
    const C2H: usize,
    const PACKET: usize,
    const OUTPUT: usize,
>(
    transport: &InProcessHciControllerTransport<'_, M, H2C, C2H, PACKET>,
    core: &mut LeController<'_, OUTPUT>,
    radio: &P,
    timer: &impl Timer,
) -> ServeExit<P::Fault>
where
    M: RawMutex,
    P: LeRadioPort,
{
    match radio.lifecycle(LifecycleCommand::Enable).await {
        Ok(Ok(()) | Err(LifecycleError::AlreadyInState)) => {}
        Ok(Err(error)) => return ServeExit::Lifecycle(error),
        Err(poisoned) => return poisoned.into(),
    }
    let timing = radio.capabilities().timing;
    let mut buffer = [0; PACKET];
    let mut retry_at: Option<Instant> = None;
    // The last request was refused as unsupported: ask again only after
    // new input.
    let mut unsupported = false;
    let mut reported = RadioActivity::IDLE;
    loop {
        // Report a change of the active roles first.
        let activity = core.activity();
        if activity != reported {
            if let Err(poisoned) = radio.activity(activity) {
                return poisoned.into();
            }
            reported = activity;
        }

        // Publish what the core queued.
        while let Some(packet) = core.front() {
            match transport.try_publish(packet.kind(), packet.as_bytes()) {
                Ok(()) => core.pop(),
                Err(HciChannelError::Full) => break,
                Err(HciChannelError::Closed) => return ServeExit::Closed,
                Err(error) => return ServeExit::Transport(error),
            }
        }

        // Submit the next radio request.
        if core.wants_radio() && !unsupported && retry_at.is_none_or(|at| timer.now() >= at) {
            retry_at = None;
            let now = match radio.now().await {
                Ok(Ok(now)) => now,
                Ok(Err(error)) => return ServeExit::Clock(error),
                Err(poisoned) => return poisoned.into(),
            };
            let work = match core.next_request(now, timing) {
                Ok(work) => work,
                Err(error) => return ServeExit::Planning(error),
            };
            match work {
                Some(RadioWork::Submit(request)) => {
                    let result = match radio.submit(request).await {
                        Ok(result) => result,
                        Err(poisoned) => return poisoned.into(),
                    };
                    match result {
                        Ok(()) => {}
                        Err(RequestError::Unsupported) => unsupported = true,
                        Err(_) => retry_at = Some(retry_after_refusal(timer)),
                    }
                    core.request_done(result);
                    continue;
                }
                Some(RadioWork::Cancel(id)) => {
                    let result = match radio.cancel(id).await {
                        Ok(result) => result,
                        Err(poisoned) => return poisoned.into(),
                    };
                    core.cancel_done(result);
                    continue;
                }
                None => {}
            }
            // Nothing could be placed now; ask again shortly.
            retry_at = Some(retry_after_refusal(timer));
        }

        // Take the next command, or ACL data while the connection takes it.
        if core.is_command_ready() {
            match transport.try_receive_admitted(&mut buffer, core.is_acl_ready()) {
                Ok(HostToControllerFrame::Command(command)) => {
                    core.command(command)
                        .expect("the core is ready for a command");
                    unsupported = false;
                    continue;
                }
                Ok(HostToControllerFrame::Acl(packet)) => {
                    core.acl(packet);
                    unsupported = false;
                    continue;
                }
                // LE carries no synchronous or isochronous data here.
                Ok(_) => continue,
                Err(HciChannelError::Empty) => {}
                Err(HciChannelError::Closed) => return ServeExit::Closed,
                Err(error) => return ServeExit::Transport(error),
            }
        }

        // Wait for any of them to make progress.
        let command_ready = core.is_command_ready();
        let acl_ready = core.is_acl_ready();
        let publishing = core.front().is_some();
        let retry = retry_at.filter(|_| core.wants_radio() && !unsupported);
        let event = select4(
            radio.next_event(),
            async {
                if command_ready {
                    transport.wait_receive_admitted(acl_ready).await;
                } else {
                    core::future::pending::<()>().await;
                }
            },
            async {
                if publishing {
                    transport.wait_publish_ready().await;
                } else {
                    core::future::pending::<()>().await;
                }
            },
            async {
                match retry {
                    Some(at) => timer.wait_until(at).await,
                    None => core::future::pending::<()>().await,
                }
            },
        )
        .await;
        match event {
            Either4::First(Ok(Ok(event))) => {
                retry_at = None;
                unsupported = false;
                core.outcome(P::view(&event));
            }
            // Advertising and scan reports are gone; every event's end and
            // the connection's data still follow.
            Either4::First(Ok(Err(EventsLost))) => {}
            Either4::First(Err(poisoned)) => return poisoned.into(),
            Either4::Second(()) | Either4::Third(()) => {}
            Either4::Fourth(()) => retry_at = None,
        }
    }
}

/// When to ask the core again after a refusal; the end of time when the
/// delay would leave the timer's range.
fn retry_after_refusal(timer: &impl Timer) -> Instant {
    timer
        .deadline_after(REFUSED_RETRY_DELAY)
        .unwrap_or(Instant::from_micros(u64::MAX))
}

#[cfg(test)]
mod tests;
