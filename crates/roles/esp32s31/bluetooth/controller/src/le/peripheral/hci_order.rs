//! Affine first/active peripheral HCI response order.
//!
//! This hardware-free coordination layer retains the exact response and its
//! owner while the physical session progresses independently of Host output
//! capacity. Only successful publication restores next-command authority.

#![forbid(unsafe_code)]

use embassy_sync::blocking_mutex::raw::RawMutex;
use oer_bluetooth_hci::{
    HciChannelError, LeControllerCommandEndpoint, LeControllerCommandReady,
    LeControllerEndpointMismatch, LeControllerResponsePending, LeControllerResponsePublication,
};

/// HCI-order axis retained beside the peripheral radio session.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyConnectablePeripheralFirstHciAxis {
    CommandReady,
    ResponsePending,
}

/// Result of waiting for response capacity on the current HCI-order axis.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyConnectablePeripheralFirstHciResponseWait {
    CommandReady,
    CapacityAvailable,
}

pub(super) enum LegacyConnectablePeripheralFirstHciOrder<'runtime, Owner> {
    CommandReady(LeControllerCommandReady<'runtime, Owner>),
    ResponsePending(LeControllerResponsePending<'runtime, Owner>),
}

impl<'runtime, Owner> LegacyConnectablePeripheralFirstHciOrder<'runtime, Owner> {
    pub(super) fn owner(&self) -> &Owner {
        match self {
            Self::CommandReady(ordered) => ordered.owner(),
            Self::ResponsePending(response) => response.owner(),
        }
    }

    pub(super) const fn axis(&self) -> LegacyConnectablePeripheralFirstHciAxis {
        match self {
            Self::CommandReady(_) => LegacyConnectablePeripheralFirstHciAxis::CommandReady,
            Self::ResponsePending(_) => LegacyConnectablePeripheralFirstHciAxis::ResponsePending,
        }
    }

    pub(super) fn accepts_endpoint<
        M: RawMutex,
        const H2C: usize,
        const C2H: usize,
        const PACKET: usize,
    >(
        &self,
        controller: &LeControllerCommandEndpoint<'_, M, H2C, C2H, PACKET>,
    ) -> bool {
        match self {
            Self::CommandReady(ready) => ready.accepts_endpoint(controller),
            Self::ResponsePending(pending) => pending.matches_endpoint(controller),
        }
    }

    pub(super) fn into_parts(
        self,
    ) -> (
        Owner,
        LegacyConnectablePeripheralFirstHciOrder<'runtime, ()>,
    ) {
        match self {
            Self::CommandReady(ordered) => {
                let (owner, ordered) = ordered.into_parts();
                (
                    owner,
                    LegacyConnectablePeripheralFirstHciOrder::CommandReady(ordered),
                )
            }
            Self::ResponsePending(response) => {
                let (owner, response) = response.into_parts();
                (
                    owner,
                    LegacyConnectablePeripheralFirstHciOrder::ResponsePending(response),
                )
            }
        }
    }

    pub(super) fn map_owner<Next>(
        self,
        map: impl FnOnce(Owner) -> Next,
    ) -> LegacyConnectablePeripheralFirstHciOrder<'runtime, Next> {
        match self {
            Self::CommandReady(ordered) => {
                LegacyConnectablePeripheralFirstHciOrder::CommandReady(ordered.map_owner(map))
            }
            Self::ResponsePending(response) => {
                LegacyConnectablePeripheralFirstHciOrder::ResponsePending(response.map_owner(map))
            }
        }
    }

    pub(super) async fn wait_response_capacity<
        M: RawMutex,
        const H2C: usize,
        const C2H: usize,
        const PACKET: usize,
    >(
        &self,
        controller: &LeControllerCommandEndpoint<'_, M, H2C, C2H, PACKET>,
    ) -> Result<LegacyConnectablePeripheralFirstHciResponseWait, LeControllerEndpointMismatch> {
        match self {
            Self::CommandReady(_) => {
                Ok(LegacyConnectablePeripheralFirstHciResponseWait::CommandReady)
            }
            Self::ResponsePending(response) => {
                controller.wait_response_capacity(response).await?;
                Ok(LegacyConnectablePeripheralFirstHciResponseWait::CapacityAvailable)
            }
        }
    }

    pub(super) fn try_publish_response<
        M: RawMutex,
        const H2C: usize,
        const C2H: usize,
        const PACKET: usize,
    >(
        self,
        controller: &LeControllerCommandEndpoint<'_, M, H2C, C2H, PACKET>,
    ) -> LegacyConnectablePeripheralFirstHciOrderPublication<'runtime, Owner> {
        match self {
            Self::CommandReady(ordered) => {
                LegacyConnectablePeripheralFirstHciOrderPublication::CommandReady(
                    Self::CommandReady(ordered),
                )
            }
            Self::ResponsePending(response) => match response.try_publish(controller) {
                LeControllerResponsePublication::Published(ordered) => {
                    LegacyConnectablePeripheralFirstHciOrderPublication::Published(
                        Self::CommandReady(ordered),
                    )
                }
                LeControllerResponsePublication::Pending(response) => {
                    LegacyConnectablePeripheralFirstHciOrderPublication::Pending(
                        Self::ResponsePending(response),
                    )
                }
                LeControllerResponsePublication::EndpointMismatch(response) => {
                    LegacyConnectablePeripheralFirstHciOrderPublication::EndpointMismatch(
                        Self::ResponsePending(response),
                    )
                }
                LeControllerResponsePublication::Fault {
                    pending: response,
                    error,
                } => LegacyConnectablePeripheralFirstHciOrderPublication::Fault {
                    order: Self::ResponsePending(response),
                    error,
                },
            },
        }
    }
}

pub(super) enum LegacyConnectablePeripheralFirstHciOrderPublication<'runtime, Owner> {
    CommandReady(LegacyConnectablePeripheralFirstHciOrder<'runtime, Owner>),
    Published(LegacyConnectablePeripheralFirstHciOrder<'runtime, Owner>),
    Pending(LegacyConnectablePeripheralFirstHciOrder<'runtime, Owner>),
    EndpointMismatch(LegacyConnectablePeripheralFirstHciOrder<'runtime, Owner>),
    Fault {
        order: LegacyConnectablePeripheralFirstHciOrder<'runtime, Owner>,
        error: HciChannelError,
    },
}

#[cfg(test)]
mod tests;
