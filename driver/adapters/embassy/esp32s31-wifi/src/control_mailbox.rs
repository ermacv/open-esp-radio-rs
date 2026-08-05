//! Bounded handoff from borrowed RX dispatch to the connected control owner.

use core::{
    future::Future,
    sync::atomic::{AtomicU32, Ordering},
};

use embassy_sync::channel::{Channel, Receiver, Sender, TryReceiveError, TrySendError};
use open_esp_radio_embassy_net::RawMutex;
use open_esp_radio_esp32s31_wifi_mac::connected_rx::{
    ConnectedRxControlEvent, ConnectedRxEvent, ConnectedRxSink,
};

/// Explicit observer for profiles that intentionally ignore control-plane
/// events. Production association/BlockAck state should supply a real sink.
pub struct IgnoreConnectedControl;

impl ConnectedRxSink for IgnoreConnectedControl {
    fn publish(&mut self, _event: ConnectedRxEvent<'_>) {}
}

/// Fixed synchronous queue used by executor-independent composition tests.
/// Overflow is explicit evidence; it never allocates or silently overwrites an
/// older action.
pub struct ConnectedControlQueue<const CAPACITY: usize> {
    events: [Option<ConnectedRxControlEvent>; CAPACITY],
    head: usize,
    tail: usize,
    len: usize,
    dropped: u32,
}

fn scheduled_connected_control(event: ConnectedRxEvent<'_>) -> Option<ConnectedRxControlEvent> {
    match event.control()? {
        // These are the only event classes consumed by
        // `Esp32s31ConnectedControl` today. Diagnostic Trigger/NDPA events
        // must not starve a beacon or ADDBA/DELBA transition in the bounded
        // mailbox.
        event @ (ConnectedRxControlEvent::Beacon(_) | ConnectedRxControlEvent::BlockAck(_)) => {
            Some(event)
        }
        ConnectedRxControlEvent::Trigger { .. } | ConnectedRxControlEvent::Ndpa { .. } => None,
    }
}

impl<const CAPACITY: usize> ConnectedControlQueue<CAPACITY> {
    pub const fn new() -> Self {
        Self {
            events: [None; CAPACITY],
            head: 0,
            tail: 0,
            len: 0,
            dropped: 0,
        }
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub const fn dropped(&self) -> u32 {
        self.dropped
    }

    pub fn pop(&mut self) -> Option<ConnectedRxControlEvent> {
        if self.len == 0 || CAPACITY == 0 {
            return None;
        }
        let event = self.events[self.head].take()?;
        self.head = (self.head + 1) % CAPACITY;
        self.len -= 1;
        Some(event)
    }
}

impl<const CAPACITY: usize> ConnectedRxSink for ConnectedControlQueue<CAPACITY> {
    fn publish(&mut self, event: ConnectedRxEvent<'_>) {
        let Some(event) = scheduled_connected_control(event) else {
            return;
        };
        if CAPACITY == 0 || self.len == CAPACITY {
            self.dropped = self.dropped.saturating_add(1);
            return;
        }
        self.events[self.tail] = Some(event);
        self.tail = (self.tail + 1) % CAPACITY;
        self.len += 1;
    }
}

impl<const CAPACITY: usize> Default for ConnectedControlQueue<CAPACITY> {
    fn default() -> Self {
        Self::new()
    }
}

/// Static Embassy mailbox for owned connected control events.
pub struct ConnectedControlResources<M: RawMutex, const CAPACITY: usize> {
    channel: Channel<M, ConnectedRxControlEvent, CAPACITY>,
    dropped: AtomicU32,
}

impl<M: RawMutex, const CAPACITY: usize> ConnectedControlResources<M, CAPACITY> {
    pub const fn new() -> Self {
        Self {
            channel: Channel::new(),
            dropped: AtomicU32::new(0),
        }
    }

    /// Split one station epoch into a receive-dispatch publisher and its
    /// scheduler-side consumer.
    ///
    /// Embassy channels support recreating their lightweight endpoints. The
    /// station lifecycle owner must nevertheless keep epochs disjoint: it
    /// stops the RX protocol task and drops the connected-control consumer
    /// before calling `split` for a later association. Accepting `&self`
    /// permits that sequential reuse for statically located resources without
    /// manufacturing a second channel allocation.
    pub fn split(
        &self,
    ) -> (
        ConnectedControlPublisher<'_, M, CAPACITY>,
        ConnectedControlReceiver<'_, M, CAPACITY>,
    ) {
        let resources: &Self = self;
        (
            ConnectedControlPublisher {
                sender: resources.channel.sender(),
                dropped: &resources.dropped,
            },
            ConnectedControlReceiver {
                receiver: resources.channel.receiver(),
                dropped: &resources.dropped,
                dropped_at_start: resources.dropped.load(Ordering::Relaxed),
            },
        )
    }
}

impl<M: RawMutex, const CAPACITY: usize> Default for ConnectedControlResources<M, CAPACITY> {
    fn default() -> Self {
        Self::new()
    }
}

/// RX-dispatch capability; it can publish but cannot consume or execute a
/// control action.
#[derive(Clone, Copy)]
pub struct ConnectedControlPublisher<'resources, M: RawMutex, const CAPACITY: usize> {
    sender: Sender<'resources, M, ConnectedRxControlEvent, CAPACITY>,
    dropped: &'resources AtomicU32,
}

impl<M: RawMutex, const CAPACITY: usize> ConnectedRxSink
    for ConnectedControlPublisher<'_, M, CAPACITY>
{
    fn publish(&mut self, event: ConnectedRxEvent<'_>) {
        let Some(event) = scheduled_connected_control(event) else {
            return;
        };
        if let Err(TrySendError::Full(_)) = self.sender.try_send(event) {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Scheduler-side control capability; it cannot publish borrowed RX data.
pub struct ConnectedControlReceiver<'resources, M: RawMutex, const CAPACITY: usize> {
    receiver: Receiver<'resources, M, ConnectedRxControlEvent, CAPACITY>,
    dropped: &'resources AtomicU32,
    dropped_at_start: u32,
}

impl<M: RawMutex, const CAPACITY: usize> ConnectedControlReceiver<'_, M, CAPACITY> {
    pub fn try_receive(&self) -> Option<ConnectedRxControlEvent> {
        match self.receiver.try_receive() {
            Ok(event) => Some(event),
            Err(TryReceiveError::Empty) => None,
        }
    }

    pub fn ready(&self) -> impl Future<Output = ()> + '_ {
        self.receiver.ready_to_receive()
    }

    pub fn len(&self) -> usize {
        self.receiver.len()
    }

    pub fn dropped(&self) -> u32 {
        self.dropped
            .load(Ordering::Relaxed)
            .wrapping_sub(self.dropped_at_start)
    }
}

#[cfg(test)]
mod tests {
    use open_esp_radio_embassy_net::NoopRawMutex;
    use open_esp_radio_esp32s31_wifi_mac::{
        connected_rx::{ConnectedRxControlEvent, ConnectedRxEvent, ConnectedRxSink},
        tx_ampdu::BlockAckAction,
    };
    use open_esp_radio_ieee80211::data::EthernetFrameParts;

    use super::*;

    #[test]
    fn synchronous_queue_copies_actions_but_never_borrowed_ethernet() {
        let body = [3, 2, 0, 0, 0, 0];
        let action = BlockAckAction::Delba {
            tid: 0,
            initiator: true,
            reason: 37,
        };
        let mut queue = ConnectedControlQueue::<1>::new();

        queue.publish(ConnectedRxEvent::Ethernet {
            frame: EthernetFrameParts {
                destination: [0; 6],
                source: [0; 6],
                ether_type: 0,
                payload: &[],
            },
            raw: &[0; 14],
            amsdu: false,
            metadata: open_esp_radio_wifi_softmac::MacRxMetadata::unavailable(),
        });
        queue.publish(ConnectedRxEvent::BlockAck {
            action,
            body: &body,
        });
        assert_eq!(queue.len(), 1);
        assert_eq!(queue.dropped(), 0);
        assert_eq!(queue.pop(), Some(ConnectedRxControlEvent::BlockAck(action)));
        assert!(queue.is_empty());
    }

    #[test]
    fn embassy_endpoints_preserve_fifo_and_report_overflow() {
        let resources = ConnectedControlResources::<NoopRawMutex, 1>::new();
        let (mut publisher, receiver) = resources.split();
        let first = BlockAckAction::Delba {
            tid: 1,
            initiator: true,
            reason: 2,
        };
        let second = BlockAckAction::Delba {
            tid: 3,
            initiator: false,
            reason: 4,
        };

        publisher.publish(ConnectedRxEvent::BlockAck {
            action: first,
            body: &[3, 2, 0, 0, 2, 0],
        });
        publisher.publish(ConnectedRxEvent::BlockAck {
            action: second,
            body: &[3, 2, 0, 0, 4, 0],
        });

        assert_eq!(receiver.len(), 1);
        assert_eq!(receiver.dropped(), 1);
        assert_eq!(
            receiver.try_receive(),
            Some(ConnectedRxControlEvent::BlockAck(first))
        );
        assert_eq!(receiver.try_receive(), None);
    }

    #[test]
    fn resources_are_reused_by_sequential_epochs() {
        let resources = ConnectedControlResources::<NoopRawMutex, 1>::new();
        let action = BlockAckAction::Delba {
            tid: 2,
            initiator: true,
            reason: 8,
        };

        {
            let (mut publisher, receiver) = resources.split();
            publisher.publish(ConnectedRxEvent::BlockAck {
                action,
                body: &[3, 2, 0, 0, 8, 0],
            });
            assert_eq!(
                receiver.try_receive(),
                Some(ConnectedRxControlEvent::BlockAck(action))
            );
            publisher.publish(ConnectedRxEvent::BlockAck {
                action,
                body: &[3, 2, 0, 0, 8, 0],
            });
            publisher.publish(ConnectedRxEvent::BlockAck {
                action,
                body: &[3, 2, 0, 0, 8, 0],
            });
            assert_eq!(receiver.dropped(), 1);
            assert_eq!(
                receiver.try_receive(),
                Some(ConnectedRxControlEvent::BlockAck(action))
            );
        }

        let (mut publisher, receiver) = resources.split();
        assert_eq!(receiver.dropped(), 0);
        publisher.publish(ConnectedRxEvent::BlockAck {
            action,
            body: &[3, 2, 0, 0, 8, 0],
        });
        assert_eq!(
            receiver.try_receive(),
            Some(ConnectedRxControlEvent::BlockAck(action))
        );
    }
}
