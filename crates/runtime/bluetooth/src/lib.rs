#![no_std]
#![forbid(unsafe_code)]

//! Executor-independent service loop of the Bluetooth LE Controller.
//!
//! [`serve`] moves packets and radio work between three owners: the
//! Controller end of an in-process HCI transport, the sans-IO
//! [`LeController`] core and one [`LeRadioPort`]. It publishes the core's
//! queued packets, takes the next Host command when the core is ready for
//! one, submits the core's radio requests one at a time and feeds every
//! radio outcome back. A refused request is retried after
//! [`REFUSED_RETRY_DELAY`] or the next outcome or command.
//!
//! The loop owns no memory of its own beyond one command buffer and spawns
//! nothing; the caller polls it on a task of its choice. It ends when the
//! transport closes or fails, or when the radio port fails, faults or loses
//! outcomes. Host data packets are discarded: the core has no connections.

#[cfg(test)]
extern crate std;

use core::future::Future;

use embassy_futures::select::{Either4, select4};
use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_time::{Duration, Instant, Timer};
use oer_bluetooth_controller::LeController;
use oer_bluetooth_hci::{HciChannelError, HostToControllerFrame, InProcessHciControllerTransport};
use oer_bluetooth_radio::{
    RadioFault, RadioInstant, RadioOutcome, RadioRequest, RadioTiming, RequestError,
};

/// Delay before asking the core again after the radio refused a request.
pub const REFUSED_RETRY_DELAY: Duration = Duration::from_millis(1);

/// The radio queue overflowed and dropped outcomes; the roles can no longer
/// account their events.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OutcomesLost;

/// A radio backend as the service loop drives it.
pub trait LeRadioPort {
    /// One owned outcome.
    type Outcome;
    /// Why the port cannot serve at all.
    type Error;

    /// A fresh radio time and the radio's admission timing.
    fn clock(&self) -> impl Future<Output = Result<(RadioInstant, RadioTiming), Self::Error>>;

    /// Submit one request: `Ok(Err(_))` when the radio refused it.
    fn request(
        &self,
        request: RadioRequest<'_>,
    ) -> impl Future<Output = Result<Result<(), RequestError>, Self::Error>>;

    /// The next outcome. Dropping the future loses no outcome.
    fn next_outcome(&self) -> impl Future<Output = Result<Self::Outcome, OutcomesLost>>;

    /// The portable view of an owned outcome.
    fn view(outcome: &Self::Outcome) -> RadioOutcome<'_>;
}

/// Why [`serve`] ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServeExit<E> {
    /// The transport closed.
    Closed,
    /// The transport refused a packet.
    Transport(HciChannelError),
    /// The radio port failed.
    Radio(E),
    /// The radio reported a fault.
    Fault(RadioFault),
    /// The radio dropped outcomes.
    OutcomesLost,
}

/// A port without a radio: time stands still and every request is refused
/// as unavailable. Radio commands then complete with a failure status.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoRadio;

/// [`NoRadio`] never fails and never produces an outcome.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Never {}

impl LeRadioPort for NoRadio {
    type Outcome = Never;
    type Error = Never;

    async fn clock(&self) -> Result<(RadioInstant, RadioTiming), Never> {
        Ok((
            RadioInstant::from_micros(0),
            RadioTiming {
                preparation_lead: oer_bluetooth_radio::RadioDuration::from_micros(0),
                admission_guard: oer_bluetooth_radio::RadioDuration::from_micros(0),
            },
        ))
    }

    async fn request(&self, _: RadioRequest<'_>) -> Result<Result<(), RequestError>, Never> {
        Ok(Err(RequestError::Unavailable))
    }

    async fn next_outcome(&self) -> Result<Never, OutcomesLost> {
        core::future::pending().await
    }

    fn view(outcome: &Never) -> RadioOutcome<'_> {
        match *outcome {}
    }
}

/// Serve the Host through `transport` with `core` over `radio` until the
/// transport or the radio ends the service.
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
) -> ServeExit<P::Error>
where
    M: RawMutex,
    P: LeRadioPort,
{
    let mut buffer = [0; PACKET];
    let mut retry_at: Option<Instant> = None;
    loop {
        // Publish what the core queued.
        while let Some(packet) = core.front() {
            match transport.try_publish(packet.kind(), packet.as_bytes()) {
                Ok(()) => core.pop(),
                Err(HciChannelError::Full) => break,
                Err(HciChannelError::Closed) => return ServeExit::Closed,
                Err(error) => return ServeExit::Transport(error),
            }
        }
        if let Some(fault) = core.fault() {
            return ServeExit::Fault(fault);
        }

        // Submit the next radio request.
        if core.wants_radio() && retry_at.is_none_or(|at| Instant::now() >= at) {
            retry_at = None;
            let (now, timing) = match radio.clock().await {
                Ok(clock) => clock,
                Err(error) => return ServeExit::Radio(error),
            };
            if let Some(request) = core.next_request(now, timing) {
                let result = match radio.request(request).await {
                    Ok(result) => result,
                    Err(error) => return ServeExit::Radio(error),
                };
                if result.is_err() {
                    retry_at = Some(Instant::now() + REFUSED_RETRY_DELAY);
                }
                core.request_done(result);
                continue;
            }
            // Nothing could be placed now; ask again shortly.
            retry_at = Some(Instant::now() + REFUSED_RETRY_DELAY);
        }

        // Take the next command.
        if core.is_command_ready() {
            match transport.try_receive(&mut buffer) {
                Ok(HostToControllerFrame::Command(command)) => {
                    core.command(command)
                        .expect("the core is ready for a command");
                    continue;
                }
                // No connection carries data.
                Ok(_) => continue,
                Err(HciChannelError::Empty) => {}
                Err(HciChannelError::Closed) => return ServeExit::Closed,
                Err(error) => return ServeExit::Transport(error),
            }
        }

        // Wait for any of them to make progress.
        let command_ready = core.is_command_ready();
        let publishing = core.front().is_some();
        let retry = retry_at.filter(|_| core.wants_radio());
        let event = select4(
            radio.next_outcome(),
            async {
                if command_ready {
                    transport.wait_receive_ready().await;
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
                    Some(at) => Timer::at(at).await,
                    None => core::future::pending::<()>().await,
                }
            },
        )
        .await;
        match event {
            Either4::First(Ok(outcome)) => {
                retry_at = None;
                core.outcome(P::view(&outcome));
            }
            Either4::First(Err(OutcomesLost)) => return ServeExit::OutcomesLost,
            Either4::Second(()) | Either4::Third(()) => {}
            Either4::Fourth(()) => retry_at = None,
        }
    }
}

#[cfg(test)]
mod tests;
