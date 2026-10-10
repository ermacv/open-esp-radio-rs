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
//! The loop owns the port's session: it enables the port when it starts,
//! cancels a scheduled event through the port's cancellation when the core
//! asks for it, and ends with a managed stop. It is the port's one event
//! consumer from the first command to the last outcome.
//!
//! The loop owns no memory of its own beyond one command buffer and spawns
//! nothing; the caller polls it on a task of its choice, beside the radio
//! backend's own runner. It stops serving when the owner's stop request
//! completes, when the transport closes or fails, when the radio's clock
//! cannot be read, when the port refuses to enable, or when the radio epoch
//! is exhausted ([`ServeExit`]). The port reserves every event's end and a
//! connection event's data when it admits the event, so an [`EventsLost`]
//! stands for advertising and scan reports only and the loop goes on: the
//! core's roles still account every event by its end. Host ACL data enters
//! the core while it has room for a packet; until then commands pass queued
//! data.
//!
//! The radio epoch is exhausted when a fresh sample lies past its end
//! ([`ClockError::EpochExhausted`], [`RequestError::EpochExhausted`]) or a
//! required continuation of the core's schedule lies outside it
//! ([`EpochExhausted`]): no later sample brings either back, so the loop
//! submits nothing more, retries nothing and stops serving.
//!
//! The managed stop then disables the port and feeds every outcome to the
//! core until [`LifecycleEvent::Disabled`]: each admitted event ends exactly
//! once, with its real completion when it ran, and the port's terminal
//! reservations keep the bounded outcome queue from overflowing. A stop the
//! port refuses as busy takes one more outcome, towards the other lifecycle
//! command's terminal or a free slot, and asks again. The caller then owns a disabled port and a core that
//! accounted every event, ready for the backend's uninstall and the Host
//! epoch's retirement. A poisoned port is not stopped ([`Served::Poisoned`]);
//! a stop that fails keeps both causes ([`Served::StopFailed`]), and the
//! caller retains its owners.

#[cfg(test)]
extern crate std;

use core::pin::pin;

use embassy_futures::select::{Either, Either4, select, select4};
use embassy_sync::blocking_mutex::raw::RawMutex;
use oer_bluetooth_controller::{EpochExhausted, LeController, RadioWork};
use oer_bluetooth_hci::HostToControllerFrame;
use oer_bluetooth_hci_transport::{HciChannelError, InProcessHciControllerTransport};
use oer_bluetooth_radio::{
    ClockError, EventsLost, LeRadioPort, LifecycleCommand, LifecycleError, LifecycleEvent,
    Poisoned, RadioActivity, RadioOutcome, RequestError,
};
use oer_time::{Duration, Instant, Timer};

/// Delay before asking the core again after the radio refused a request.
pub const REFUSED_RETRY_DELAY: Duration = Duration::from_millis(1);

/// Why [`serve`] stopped serving.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServeExit {
    /// The owner's stop request completed.
    Stopped,
    /// The transport closed.
    Closed,
    /// The transport refused a packet.
    Transport(HciChannelError),
    /// The port refused to enable.
    Lifecycle(LifecycleError),
    /// The radio clock could not be read now.
    Clock(ClockError),
    /// The radio epoch is exhausted.
    EpochExhausted(Exhaustion),
}

/// Where [`serve`] found the radio epoch exhausted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Exhaustion {
    /// A fresh sample of the radio clock lies past the epoch's end.
    Clock,
    /// A required continuation of the core's schedule lies outside the epoch.
    Planning(EpochExhausted),
}

/// Why the managed stop failed; `F` is the port's poison cause.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StopError<F> {
    /// The port refused to disable.
    Lifecycle(LifecycleError),
    /// The port admitted the disable, then reported it failed.
    DisableFailed,
    /// The port is poisoned.
    Poisoned(Poisoned<F>),
}

/// How [`serve`] ended; `F` is the port's poison cause.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Served<F> {
    /// The service stopped serving for `exit`, and the managed stop ended
    /// every admitted event: the port is disabled.
    Stopped(ServeExit),
    /// The service stopped serving for `exit`, and the managed stop failed
    /// with `error`: admitted events may be unaccounted, so the caller keeps
    /// the port, the core and the backend's owners.
    StopFailed {
        /// Why the service stopped serving.
        exit: ServeExit,
        /// Why the managed stop failed.
        error: StopError<F>,
    },
    /// The port is poisoned; nothing was stopped.
    Poisoned(Poisoned<F>),
}

/// Serve the Host through `transport` with `core` over `radio` until `stop`
/// completes or the transport, the radio or the radio epoch ends the
/// service, then stop the port. Retries after a refusal wait on `timer`.
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
    stop: impl Future<Output = ()>,
) -> Served<P::Fault>
where
    M: RawMutex,
    P: LeRadioPort,
{
    let exit = match serve_until(transport, core, radio, timer, stop).await {
        Ok(exit) => exit,
        Err(poisoned) => return Served::Poisoned(poisoned),
    };
    match stop_port(core, radio).await {
        Ok(()) => Served::Stopped(exit),
        Err(error) => Served::StopFailed { exit, error },
    }
}

/// Disable `radio` and feed every outcome to `core` until the port reports
/// `Disabled`.
async fn stop_port<P: LeRadioPort, const OUTPUT: usize>(
    core: &mut LeController<'_, OUTPUT>,
    radio: &P,
) -> Result<(), StopError<P::Fault>> {
    loop {
        match radio.lifecycle(LifecycleCommand::Disable).await {
            Ok(Ok(())) => break,
            // Nothing was admitted: the port never enabled, or is disabled.
            Ok(Err(LifecycleError::AlreadyInState)) => return Ok(()),
            // Another lifecycle command has not ended, or the outcome queue
            // has no slot for the terminal: take one outcome, which frees a
            // slot or is on the way to the other terminal, and ask again.
            Ok(Err(LifecycleError::Busy)) => {
                take_outcome(core, radio).await?;
            }
            Ok(Err(error)) => return Err(StopError::Lifecycle(error)),
            Err(poisoned) => return Err(StopError::Poisoned(poisoned)),
        }
    }
    loop {
        match take_outcome(core, radio).await? {
            Some(LifecycleEvent::Disabled) => return Ok(()),
            Some(LifecycleEvent::Failed {
                command: LifecycleCommand::Disable,
            }) => return Err(StopError::DisableFailed),
            _ => {}
        }
    }
}

/// Feed the next outcome to `core`; the lifecycle terminal it was, if any.
async fn take_outcome<P: LeRadioPort, const OUTPUT: usize>(
    core: &mut LeController<'_, OUTPUT>,
    radio: &P,
) -> Result<Option<LifecycleEvent>, StopError<P::Fault>> {
    match radio.next_event().await {
        Ok(Ok(event)) => {
            let outcome = P::view(&event);
            core.outcome(outcome);
            Ok(match outcome {
                RadioOutcome::Lifecycle(terminal) => Some(terminal),
                _ => None,
            })
        }
        // Advertising and scan reports are gone; every end still follows.
        Ok(Err(EventsLost)) => Ok(None),
        Err(poisoned) => Err(StopError::Poisoned(poisoned)),
    }
}

/// Serve until the service must stop: why, or the port's poison.
async fn serve_until<
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
    stop: impl Future<Output = ()>,
) -> Result<ServeExit, Poisoned<P::Fault>>
where
    M: RawMutex,
    P: LeRadioPort,
{
    let mut stop = pin!(stop);
    match radio.lifecycle(LifecycleCommand::Enable).await {
        Ok(Ok(()) | Err(LifecycleError::AlreadyInState)) => {}
        Ok(Err(error)) => return Ok(ServeExit::Lifecycle(error)),
        Err(poisoned) => return Err(poisoned),
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
            radio.activity(activity)?;
            reported = activity;
        }

        // Publish what the core queued.
        while let Some(packet) = core.front() {
            match transport.try_publish(packet.kind(), packet.as_bytes()) {
                Ok(()) => core.pop(),
                Err(HciChannelError::Full) => break,
                Err(HciChannelError::Closed) => return Ok(ServeExit::Closed),
                Err(error) => return Ok(ServeExit::Transport(error)),
            }
        }

        // Submit the next radio request.
        if core.wants_radio() && !unsupported && retry_at.is_none_or(|at| timer.now() >= at) {
            retry_at = None;
            let now = match radio.now().await {
                Ok(Ok(now)) => now,
                Ok(Err(ClockError::EpochExhausted)) => {
                    return Ok(ServeExit::EpochExhausted(Exhaustion::Clock));
                }
                Ok(Err(error)) => return Ok(ServeExit::Clock(error)),
                Err(poisoned) => return Err(poisoned),
            };
            let work = match core.next_request(now, timing) {
                Ok(work) => work,
                Err(exhausted) => {
                    return Ok(ServeExit::EpochExhausted(Exhaustion::Planning(exhausted)));
                }
            };
            match work {
                Some(RadioWork::Submit(request)) => {
                    let result = match radio.submit(request).await {
                        Ok(result) => result,
                        Err(poisoned) => return Err(poisoned),
                    };
                    core.request_done(result);
                    match result {
                        Ok(()) => {}
                        Err(RequestError::EpochExhausted) => {
                            return Ok(ServeExit::EpochExhausted(Exhaustion::Clock));
                        }
                        Err(RequestError::Unsupported) => unsupported = true,
                        Err(_) => retry_at = Some(retry_after_refusal(timer)),
                    }
                    continue;
                }
                Some(RadioWork::Cancel(id)) => {
                    let result = match radio.cancel(id).await {
                        Ok(result) => result,
                        Err(poisoned) => return Err(poisoned),
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
                Err(HciChannelError::Closed) => return Ok(ServeExit::Closed),
                Err(error) => return Ok(ServeExit::Transport(error)),
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
            select(stop.as_mut(), async {
                match retry {
                    Some(at) => timer.wait_until(at).await,
                    None => core::future::pending::<()>().await,
                }
            }),
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
            Either4::First(Err(poisoned)) => return Err(poisoned),
            Either4::Second(()) | Either4::Third(()) => {}
            Either4::Fourth(Either::First(())) => return Ok(ServeExit::Stopped),
            Either4::Fourth(Either::Second(())) => retry_at = None,
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
