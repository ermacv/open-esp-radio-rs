//! Contiguous Peripheral LE1M events with independently retained HCI order.
//!
//! Requires an explicit local-clock timing policy in the runtime configuration.
//! Central feature requests and unsupported optional LLCP requests enter a
//! bounded control-response queue. Active HCI commands, ACL delivery, mandatory
//! connection updates and host-initiated teardown remain unavailable. Peer
//! termination retires the unlinked graph and restores ordered idle HCI intake.
//! An unanswered initial transmit window recurs with its full WinSize; six
//! events without establishment retire the connection with reason `0x3e`.
//! Established supervision uses the independent hardware valid-RX time and
//! retires expired unlinked connections with reason `0x08`.
//! Version exchange requires a caller-supplied Controller implementation identity.
//! Any radio fault or unsupported mandatory-control transition seals its owners.

#![forbid(unsafe_code)]

mod radio;

use super::first_hci::{
    LegacyConnectablePeripheralFirstHciAxis as Axis,
    LegacyConnectablePeripheralFirstHciOrder as Order,
    LegacyConnectablePeripheralFirstHciResponsePublication as Publication,
    LegacyConnectablePeripheralFirstHciResponseWait as ResponseWait,
    LegacyConnectablePeripheralFirstHciRunning as FirstRunning,
    LegacyConnectablePeripheralFirstHciRunningOrder as RunningOrder, map_order_publication,
};
use crate::controller::SchedulerRunInterruptStorage;
use embassy_sync::blocking_mutex::raw::RawMutex;
use oer_bluetooth_hci::{LeControllerCommandEndpoint, LeControllerEndpointMismatch};
pub use radio::{PeripheralConnectionActiveFaultCause, PeripheralConnectionActiveWait};

/// Sole owner of completion, successor preparation, and the ordered response.
#[must_use = "drive or retain the exact active connection owner"]
pub struct PeripheralConnectionActiveSession<'a, S: SchedulerRunInterruptStorage, const N: usize> {
    order: Order<'a, radio::Radio<'a, S, N>>,
    control: oer_bluetooth_ll::control::LePeripheralControl,
    supervision: Option<super::supervision::PeripheralSupervisionDeadline>,
}

/// One finite radio transition; only `Published` represents a new scheduler RUN.
#[must_use = "retain the returned session or sealed failure"]
pub enum PeripheralConnectionActiveStep<'a, S: SchedulerRunInterruptStorage, const N: usize> {
    /// Termination or failed establishment returned the ordered idle command owner.
    Stopped {
        task: crate::controller::ControllerIdleCommandTask<'a, S, N>,
        reason: u8,
    },
    Continue(PeripheralConnectionActiveSession<'a, S, N>),
    Published(PeripheralConnectionActiveSession<'a, S, N>),
    Fault(PeripheralConnectionActiveFault<'a, S, N>),
}

/// Sealed radio transaction and HCI authority; does not authorize reclamation.
#[must_use = "retain the fault until the hardware is quarantined"]
pub struct PeripheralConnectionActiveFault<'a, S: SchedulerRunInterruptStorage, const N: usize> {
    radio: radio::Fault<'a, S, N>,
    _order: Order<'a, ()>,
}

impl<S: SchedulerRunInterruptStorage, const N: usize> PeripheralConnectionActiveFault<'_, S, N> {
    pub const fn cause(&self) -> PeripheralConnectionActiveFaultCause {
        self.radio.cause
    }
}

impl<'a, S: SchedulerRunInterruptStorage, const N: usize>
    PeripheralConnectionActiveSession<'a, S, N>
{
    pub fn from_first(first: FirstRunning<'a, S, N>) -> Self {
        let (running, order) = first.into_parts();
        let order = match order {
            RunningOrder::CommandReady(order) => Order::CommandReady(order),
            RunningOrder::ResponsePending(order) => Order::ResponsePending(order),
        };
        Self {
            order: order.map_owner(|()| radio::Radio::Running(running)),
            control: oer_bluetooth_ll::control::LePeripheralControl::new(),
            supervision: None,
        }
    }

    pub const fn hci_axis(&self) -> Axis {
        self.order.axis()
    }

    /// Borrow readiness from the retained phase. `None` requires an immediate step.
    pub fn radio_wait(&self) -> Option<PeripheralConnectionActiveWait<'_>> {
        self.order.owner().wait()
    }

    // Do not merge the affine radio transition's temporaries into the much
    // larger controller dispatch future's stack frame.
    #[inline(never)]
    pub fn step_radio(self) -> PeripheralConnectionActiveStep<'a, S, N> {
        let Self {
            order,
            mut control,
            mut supervision,
        } = self;
        let (radio, order) = order.into_parts();
        let (radio, order) = match (radio, order) {
            (radio::Radio::Stopped { task, reason }, Order::CommandReady(ready)) => {
                return PeripheralConnectionActiveStep::Stopped {
                    task: crate::controller::ControllerIdleCommandTask::from_parts(task, ready),
                    reason,
                };
            }
            pair => pair,
        };
        match radio.step(&mut control, &mut supervision) {
            radio::Step::Continue(radio) => PeripheralConnectionActiveStep::Continue(Self {
                order: order.map_owner(|()| radio),
                control,
                supervision,
            }),
            radio::Step::Published(radio) => PeripheralConnectionActiveStep::Published(Self {
                order: order.map_owner(|()| radio),
                control,
                supervision,
            }),
            radio::Step::Fault(radio) => {
                PeripheralConnectionActiveStep::Fault(PeripheralConnectionActiveFault {
                    radio,
                    _order: order,
                })
            }
        }
    }

    /// Cancellation leaves the exact response and radio phase in this session.
    pub async fn wait_response_capacity<
        M: RawMutex,
        const H2C: usize,
        const C2H: usize,
        const PACKET: usize,
    >(
        &self,
        controller: &LeControllerCommandEndpoint<'_, M, H2C, C2H, PACKET>,
    ) -> Result<ResponseWait, LeControllerEndpointMismatch> {
        self.order.wait_response_capacity(controller).await
    }

    pub fn try_publish_response<
        M: RawMutex,
        const H2C: usize,
        const C2H: usize,
        const PACKET: usize,
    >(
        self,
        controller: &LeControllerCommandEndpoint<'_, M, H2C, C2H, PACKET>,
    ) -> Publication<Self> {
        let Self {
            order,
            control,
            supervision,
        } = self;
        map_order_publication(order.try_publish_response(controller), |order| Self {
            order,
            control,
            supervision,
        })
    }
}
