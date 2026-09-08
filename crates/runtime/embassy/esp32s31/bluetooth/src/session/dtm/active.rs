//! Cancellation-safe Embassy waits for one borrowed active DTM session.
//!
//! No future in this module owns the affine session. Dropping a wait therefore
//! cancels only readiness observation; every radio and HCI owner remains in the
//! caller's `DtmActiveSession`.

#![forbid(unsafe_code)]

use core::future::Future;

use embassy_futures::select::{Either, select};

#[cfg(target_arch = "riscv32")]
use crate::notification::{PostUnlinkSignal, RuntimeNotifications};
#[cfg(target_arch = "riscv32")]
use embassy_sync::blocking_mutex::raw::RawMutex;
#[cfg(target_arch = "riscv32")]
use oer_bluetooth_hci::LeControllerCommandEndpoint;
#[cfg(target_arch = "riscv32")]
use oer_esp32s31_bluetooth::{
    controller::SchedulerRunInterruptStorage,
    le::dtm::{DtmActiveRadioWait, DtmActiveSession, DtmOrderReady, DtmResponsePending},
};

/// Exact radio-side reason an active-session wait completed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DtmActiveRadioSignal {
    /// Other executor tasks had an opportunity to run between radio events.
    Cooperative,
    /// The durable scheduler wake cell became non-empty.
    Scheduler,
    /// The exact post-unlink mailbox wake became durable.
    PostUnlink,
    /// The caller-provided absolute recheck future completed.
    ControllerTime,
}

/// Next signal for an active session whose command response is still pending.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DtmActivePendingSignal {
    /// Radio progress won the wait, including a simultaneous-ready tie.
    Radio(DtmActiveRadioSignal),
    /// The matching Controller-to-Host queue reported a capacity hint.
    ResponseCapacity,
}

/// Next signal after the previous DTM response entered HCI.
///
/// Host readiness is deliberately non-consuming. The sole task owner performs
/// a later synchronous receive, so a radio-first tie, cancellation, or this
/// signal itself can neither classify nor borrow the oldest packet.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "route the radio edge or synchronously receive the ready Host packet"]
pub enum DtmActiveCommandSignal {
    /// Radio progress won the wait, including a simultaneous-ready tie.
    Radio(DtmActiveRadioSignal),
    /// Host-to-Controller storage was observed non-empty without consuming it.
    HostReady,
}

/// Failure to compose a pending response or command intake with an HCI endpoint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DtmActiveWaitError {
    /// The endpoint belongs to another live Controller epoch.
    EndpointMismatch,
}

#[cfg(target_arch = "riscv32")]
/// Borrowed wait view over one parked active DTM session.
///
/// Construction succeeds only while [`DtmActiveSession::radio_wait`]
/// exposes a real radio wait. The view owns neither axis and may be dropped at
/// any await point without dropping or changing the session.
#[must_use = "borrow this view only while waiting, then resume the separately owned session"]
pub struct DtmActiveWait<'borrow, 'runtime, S, const CAPACITY: usize, Order, M>
where
    S: SchedulerRunInterruptStorage,
    M: RawMutex,
{
    session: &'borrow DtmActiveSession<'runtime, S, CAPACITY, Order>,
    wakers: &'borrow RuntimeNotifications<M>,
}

#[cfg(target_arch = "riscv32")]
impl<'borrow, 'runtime, S, const CAPACITY: usize, Order, M>
    DtmActiveWait<'borrow, 'runtime, S, CAPACITY, Order, M>
where
    S: SchedulerRunInterruptStorage,
    M: RawMutex,
{
    /// Borrow a session only if its radio axis is currently parked.
    pub fn from_waiting(
        session: &'borrow DtmActiveSession<'runtime, S, CAPACITY, Order>,
        wakers: &'borrow RuntimeNotifications<M>,
    ) -> Option<Self> {
        session.radio_wait().map(|_| Self { session, wakers })
    }

    async fn wait_radio<R>(&self, controller_time_recheck: R) -> DtmActiveRadioSignal
    where
        R: Future<Output = ()>,
    {
        match self
            .session
            .radio_wait()
            .expect("the borrowed wait view retains an unchanged parked session")
        {
            DtmActiveRadioWait::Scheduler(wake) => {
                self.wakers.wait_scheduler_ready(wake).await;
                DtmActiveRadioSignal::Scheduler
            }
            DtmActiveRadioWait::PostUnlink(wake) => {
                match self
                    .wakers
                    .wait_post_unlink_or_recheck(wake, controller_time_recheck)
                    .await
                {
                    PostUnlinkSignal::Mailbox => DtmActiveRadioSignal::PostUnlink,
                    PostUnlinkSignal::Recheck => DtmActiveRadioSignal::ControllerTime,
                }
            }
            DtmActiveRadioWait::Cooperative => {
                embassy_futures::yield_now().await;
                DtmActiveRadioSignal::Cooperative
            }
            DtmActiveRadioWait::ControllerTime => {
                controller_time_recheck.await;
                DtmActiveRadioSignal::ControllerTime
            }
        }
    }
}

#[cfg(target_arch = "riscv32")]
impl<'borrow, 'runtime, S, const CAPACITY: usize, M>
    DtmActiveWait<'borrow, 'runtime, S, CAPACITY, DtmResponsePending<'runtime>, M>
where
    S: SchedulerRunInterruptStorage,
    M: RawMutex,
{
    /// Borrow both readiness sources and return whichever completes first.
    ///
    /// The radio future is the first `select` operand and therefore wins a
    /// simultaneous-ready tie. `controller_time_recheck` must be anchored to a
    /// caller-owned absolute deadline; rebuilding a relative delay after HCI
    /// wakeups would incorrectly extend the Controller-time wait.
    pub async fn wait_next<
        HciMutex: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
        R,
    >(
        &self,
        controller: &LeControllerCommandEndpoint<
            '_,
            HciMutex,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
        controller_time_recheck: R,
    ) -> Result<DtmActivePendingSignal, DtmActiveWaitError>
    where
        R: Future<Output = ()>,
    {
        if !self.session.matches_hci_endpoint(controller) {
            return Err(DtmActiveWaitError::EndpointMismatch);
        }

        Ok(
            match select_radio_first(
                self.wait_radio(controller_time_recheck),
                self.session.wait_response_capacity(controller),
            )
            .await
            {
                RadioFirst::Radio(signal) => DtmActivePendingSignal::Radio(signal),
                RadioFirst::Other(Ok(())) => DtmActivePendingSignal::ResponseCapacity,
                RadioFirst::Other(Err(_)) => {
                    return Err(DtmActiveWaitError::EndpointMismatch);
                }
            },
        )
    }
}

#[cfg(target_arch = "riscv32")]
impl<'borrow, 'runtime, S, const CAPACITY: usize, M>
    DtmActiveWait<'borrow, 'runtime, S, CAPACITY, DtmOrderReady<'runtime>, M>
where
    S: SchedulerRunInterruptStorage,
    M: RawMutex,
{
    /// Race radio progress against non-consuming Host packet readiness.
    ///
    /// Radio remains the first `select` operand and wins a simultaneous-ready
    /// tie. The readiness future neither borrows a packet buffer nor consumes,
    /// classifies or reserves the oldest packet. After `HostReady`, the sole
    /// task owner finishes with a synchronous receive and handles `Empty`
    /// losslessly because readiness is only a hint.
    pub async fn wait_next<
        HciMutex: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
        R,
    >(
        &self,
        controller: &LeControllerCommandEndpoint<
            '_,
            HciMutex,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
        controller_time_recheck: R,
    ) -> Result<DtmActiveCommandSignal, DtmActiveWaitError>
    where
        R: Future<Output = ()>,
    {
        if !self.session.accepts_hci_endpoint(controller) {
            return Err(DtmActiveWaitError::EndpointMismatch);
        }

        Ok(
            match select_radio_first(
                self.wait_radio(controller_time_recheck),
                self.session.wait_command_available(controller),
            )
            .await
            {
                RadioFirst::Radio(signal) => DtmActiveCommandSignal::Radio(signal),
                RadioFirst::Other(Ok(())) => DtmActiveCommandSignal::HostReady,
                RadioFirst::Other(Err(_)) => {
                    return Err(DtmActiveWaitError::EndpointMismatch);
                }
            },
        )
    }
}

enum RadioFirst<Radio, Other> {
    Radio(Radio),
    Other(Other),
}

async fn select_radio_first<R, O>(radio: R, other: O) -> RadioFirst<R::Output, O::Output>
where
    R: Future,
    O: Future,
{
    match select(radio, other).await {
        Either::First(radio) => RadioFirst::Radio(radio),
        Either::Second(other) => RadioFirst::Other(other),
    }
}

#[cfg(test)]
mod tests;
