#![expect(
    clippy::result_large_err,
    reason = "mailbox preparation failures retain the concrete no-alloc resource owner"
)]

//! Bounded handoff from borrowed RX dispatch to the connected control owner.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use embassy_futures::select::select6;
use embassy_sync::channel::{Channel, Receiver, Sender, TrySendError};
use open_esp_radio_embassy_net::RawMutex;
use open_esp_radio_esp32s31_wifi_sta::connected_rx::{
    ConnectedRxControlEvent, ConnectedRxEvent, ConnectedRxSink,
};
use open_esp_radio_wpa2::{OwnedEapolFrame, Wpa2Interface};

const EAPOL_ETHERTYPE: u16 = 0x888e;

/// Protection provenance retained across the borrowed RX-to-control handoff.
///
/// Connected WPA2 admits plaintext only through the dedicated duplicate-M3
/// lane. Keeping that fact outside `OwnedEapolFrame` prevents the control task
/// from treating an unprotected packet as a Group Message 1.
pub(super) enum ConnectedSecurityFrame {
    Protected(OwnedEapolFrame),
    Unprotected(OwnedEapolFrame),
}

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
        // `Esp32s31ConnectedControl` consumes these as semantic state. HE
        // Trigger/NDPA uses its own single-slot lane so it cannot starve a
        // beacon or ADDBA/DELBA transition in this bounded mailbox.
        event @ (ConnectedRxControlEvent::Beacon(_)
        | ConnectedRxControlEvent::ProbeResponse
        | ConnectedRxControlEvent::BlockAck(_)
        | ConnectedRxControlEvent::IndividualTwt(_)
        | ConnectedRxControlEvent::PowerSaveDelivery(_)) => Some(event),
        event @ ConnectedRxControlEvent::PeerDisconnect(_) => Some(event),
        ConnectedRxControlEvent::Trigger { .. }
        | ConnectedRxControlEvent::Ndpa { .. }
        | ConnectedRxControlEvent::PowerSaveDeliveryRace => None,
    }
}

fn scheduled_he_observation(event: ConnectedRxEvent<'_>) -> Option<ConnectedRxControlEvent> {
    match event.control()? {
        event
        @ (ConnectedRxControlEvent::Trigger { .. } | ConnectedRxControlEvent::Ndpa { .. }) => {
            Some(event)
        }
        ConnectedRxControlEvent::Beacon(_)
        | ConnectedRxControlEvent::ProbeResponse
        | ConnectedRxControlEvent::BlockAck(_)
        | ConnectedRxControlEvent::IndividualTwt(_)
        | ConnectedRxControlEvent::PeerDisconnect(_)
        | ConnectedRxControlEvent::PowerSaveDelivery(_)
        | ConnectedRxControlEvent::PowerSaveDeliveryRace => None,
    }
}

#[derive(Clone, Copy)]
struct PowerSaveDeliverySignal {
    generation: u32,
    event: ConnectedRxControlEvent,
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
    terminal: Channel<M, ConnectedRxControlEvent, 1>,
    he_observation: Channel<M, ConnectedRxControlEvent, 1>,
    /// Authenticated connected EAPOL such as Group Message 1. Losing one is a
    /// functional protocol overflow and remains fail-closed.
    security: Channel<M, ConnectedSecurityFrame, 1>,
    /// Best-effort plaintext EAPOL admitted only as a duplicate-M3 candidate.
    /// Keeping this separate prevents unauthenticated traffic from occupying
    /// the protected security lane or poisoning ordered-control overflow.
    unprotected_security: Channel<M, ConnectedSecurityFrame, 1>,
    power_save_delivery: Channel<M, PowerSaveDeliverySignal, 1>,
    power_save_delivery_generation: AtomicU32,
    power_save_delivery_gate: AtomicU32,
    power_save_delivery_claimed: AtomicU32,
    power_save_delivery_raced: AtomicBool,
    overflowed: AtomicBool,
    dropped_he_observations: AtomicU32,
}

impl<M: RawMutex, const CAPACITY: usize> ConnectedControlResources<M, CAPACITY> {
    pub const fn new() -> Self {
        Self {
            channel: Channel::new(),
            terminal: Channel::new(),
            he_observation: Channel::new(),
            security: Channel::new(),
            unprotected_security: Channel::new(),
            power_save_delivery: Channel::new(),
            power_save_delivery_generation: AtomicU32::new(0),
            power_save_delivery_gate: AtomicU32::new(0),
            power_save_delivery_claimed: AtomicU32::new(0),
            power_save_delivery_raced: AtomicBool::new(false),
            overflowed: AtomicBool::new(false),
            dropped_he_observations: AtomicU32::new(0),
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
        resources.overflowed.store(false, Ordering::Release);
        resources
            .power_save_delivery_gate
            .store(0, Ordering::Release);
        resources
            .power_save_delivery_claimed
            .store(0, Ordering::Release);
        resources
            .power_save_delivery_raced
            .store(false, Ordering::Release);
        resources
            .dropped_he_observations
            .store(0, Ordering::Release);
        (
            ConnectedControlPublisher {
                sender: resources.channel.sender(),
                terminal: resources.terminal.sender(),
                he_observation: resources.he_observation.sender(),
                security: resources.security.sender(),
                unprotected_security: resources.unprotected_security.sender(),
                power_save_delivery: resources.power_save_delivery.sender(),
                power_save_delivery_gate: &resources.power_save_delivery_gate,
                power_save_delivery_claimed: &resources.power_save_delivery_claimed,
                power_save_delivery_raced: &resources.power_save_delivery_raced,
                overflowed: &resources.overflowed,
                dropped_he_observations: &resources.dropped_he_observations,
            },
            ConnectedControlReceiver {
                receiver: resources.channel.receiver(),
                terminal: resources.terminal.receiver(),
                he_observation: resources.he_observation.receiver(),
                security: resources.security.receiver(),
                unprotected_security: resources.unprotected_security.receiver(),
                power_save_delivery: resources.power_save_delivery.receiver(),
                power_save_delivery_generation: &resources.power_save_delivery_generation,
                power_save_delivery_gate: &resources.power_save_delivery_gate,
                power_save_delivery_claimed: &resources.power_save_delivery_claimed,
                power_save_delivery_raced: &resources.power_save_delivery_raced,
                overflowed: &resources.overflowed,
                dropped_he_observations: &resources.dropped_he_observations,
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
    terminal: Sender<'resources, M, ConnectedRxControlEvent, 1>,
    he_observation: Sender<'resources, M, ConnectedRxControlEvent, 1>,
    security: Sender<'resources, M, ConnectedSecurityFrame, 1>,
    unprotected_security: Sender<'resources, M, ConnectedSecurityFrame, 1>,
    power_save_delivery: Sender<'resources, M, PowerSaveDeliverySignal, 1>,
    power_save_delivery_gate: &'resources AtomicU32,
    power_save_delivery_claimed: &'resources AtomicU32,
    power_save_delivery_raced: &'resources AtomicBool,
    overflowed: &'resources AtomicBool,
    dropped_he_observations: &'resources AtomicU32,
}

impl<M: RawMutex, const CAPACITY: usize> ConnectedRxSink
    for ConnectedControlPublisher<'_, M, CAPACITY>
{
    fn publish(&mut self, event: ConnectedRxEvent<'_>) {
        if let ConnectedRxEvent::UnprotectedEapol { source, payload } = event {
            if let Ok(frame) = OwnedEapolFrame::try_copy(Wpa2Interface::Station, source, payload) {
                // This is unauthenticated peer input until connected WPA2
                // verifies its MIC and exact completed-M3 commitment. Full is
                // therefore a peer-local coalescing drop, never authority to
                // invalidate the ordered control stream or protected lane.
                let _ = self
                    .unprotected_security
                    .try_send(ConnectedSecurityFrame::Unprotected(frame));
            }
            return;
        }
        if let ConnectedRxEvent::Ethernet { frame, .. } = event
            && frame.ether_type == EAPOL_ETHERTYPE
        {
            let result =
                OwnedEapolFrame::try_copy(Wpa2Interface::Station, frame.source, frame.payload)
                    .ok()
                    .map(|frame| {
                        self.security
                            .try_send(ConnectedSecurityFrame::Protected(frame))
                    });
            if !matches!(result, Some(Ok(()))) {
                self.overflowed.store(true, Ordering::Release);
            }
            return;
        }
        if let ConnectedRxEvent::PowerSaveDelivery(delivery) = event {
            let generation = self.power_save_delivery_gate.load(Ordering::Acquire);
            if generation == 0 {
                return;
            }
            if self
                .power_save_delivery_claimed
                .compare_exchange(0, generation, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                if self.power_save_delivery_gate.load(Ordering::Acquire) == generation {
                    self.power_save_delivery_raced
                        .store(true, Ordering::Release);
                }
                return;
            }
            if let Err(TrySendError::Full(_)) =
                self.power_save_delivery.try_send(PowerSaveDeliverySignal {
                    generation,
                    event: ConnectedRxControlEvent::PowerSaveDelivery(delivery),
                })
            {
                self.power_save_delivery_raced
                    .store(true, Ordering::Release);
            }
            return;
        }
        if let Some(event) = scheduled_he_observation(event) {
            if let Err(TrySendError::Full(_)) = self.he_observation.try_send(event) {
                self.dropped_he_observations.fetch_add(1, Ordering::Relaxed);
            }
            return;
        }
        let Some(event) = scheduled_connected_control(event) else {
            return;
        };
        let result = if matches!(event, ConnectedRxControlEvent::PeerDisconnect(_)) {
            self.terminal.try_send(event)
        } else {
            self.sender.try_send(event)
        };
        if let Err(TrySendError::Full(_)) = result {
            self.overflowed.store(true, Ordering::Release);
        }
    }
}

/// Scheduler-side control capability; it cannot publish borrowed RX data.
pub struct ConnectedControlReceiver<'resources, M: RawMutex, const CAPACITY: usize> {
    receiver: Receiver<'resources, M, ConnectedRxControlEvent, CAPACITY>,
    terminal: Receiver<'resources, M, ConnectedRxControlEvent, 1>,
    he_observation: Receiver<'resources, M, ConnectedRxControlEvent, 1>,
    security: Receiver<'resources, M, ConnectedSecurityFrame, 1>,
    unprotected_security: Receiver<'resources, M, ConnectedSecurityFrame, 1>,
    power_save_delivery: Receiver<'resources, M, PowerSaveDeliverySignal, 1>,
    power_save_delivery_generation: &'resources AtomicU32,
    power_save_delivery_gate: &'resources AtomicU32,
    power_save_delivery_claimed: &'resources AtomicU32,
    power_save_delivery_raced: &'resources AtomicBool,
    overflowed: &'resources AtomicBool,
    dropped_he_observations: &'resources AtomicU32,
}

impl<M: RawMutex, const CAPACITY: usize> ConnectedControlReceiver<'_, M, CAPACITY> {
    pub fn try_receive_terminal(&self) -> Option<ConnectedRxControlEvent> {
        self.terminal.try_receive().ok()
    }

    pub fn try_receive_control(&self) -> Option<ConnectedRxControlEvent> {
        self.receiver.try_receive().ok()
    }

    pub fn try_receive_he_observation(&self) -> Option<ConnectedRxControlEvent> {
        self.he_observation.try_receive().ok()
    }

    pub fn set_power_save_delivery_armed(&self, armed: bool) {
        if armed {
            if self.power_save_delivery_gate.load(Ordering::Acquire) != 0 {
                return;
            }
            while self.power_save_delivery.try_receive().is_ok() {}
            self.power_save_delivery_claimed.store(0, Ordering::Release);
            self.power_save_delivery_raced
                .store(false, Ordering::Release);
            let generation = self
                .power_save_delivery_generation
                .fetch_add(1, Ordering::AcqRel)
                .wrapping_add(1)
                .max(1);
            self.power_save_delivery_gate
                .store(generation, Ordering::Release);
        } else {
            self.power_save_delivery_gate.store(0, Ordering::Release);
            while self.power_save_delivery.try_receive().is_ok() {}
            self.power_save_delivery_claimed.store(0, Ordering::Release);
            self.power_save_delivery_raced
                .store(false, Ordering::Release);
        }
    }

    pub fn try_receive_power_save_delivery(&self) -> Option<ConnectedRxControlEvent> {
        if self.power_save_delivery.is_empty()
            && !self.power_save_delivery_raced.load(Ordering::Acquire)
        {
            return None;
        }
        let active_generation = self.power_save_delivery_gate.swap(0, Ordering::AcqRel);
        if active_generation == 0 {
            while self.power_save_delivery.try_receive().is_ok() {}
            self.power_save_delivery_raced
                .store(false, Ordering::Release);
            return None;
        }
        if self.power_save_delivery_raced.swap(false, Ordering::AcqRel) {
            while self.power_save_delivery.try_receive().is_ok() {}
            return Some(ConnectedRxControlEvent::PowerSaveDeliveryRace);
        }
        let Some(signal) = self.power_save_delivery.try_receive().ok() else {
            return Some(ConnectedRxControlEvent::PowerSaveDeliveryRace);
        };
        if signal.generation != active_generation {
            return Some(ConnectedRxControlEvent::PowerSaveDeliveryRace);
        }
        Some(signal.event)
    }

    pub fn try_receive(&self) -> Option<ConnectedRxControlEvent> {
        if let Some(event) = self.try_receive_terminal() {
            return Some(event);
        }
        self.try_receive_power_save_delivery()
            .or_else(|| self.try_receive_control())
            .or_else(|| self.try_receive_he_observation())
    }

    pub(super) fn try_receive_security(&self) -> Option<ConnectedSecurityFrame> {
        // Authenticated connected traffic always precedes the best-effort
        // duplicate-M3 candidate lane, regardless of arrival order.
        self.security
            .try_receive()
            .ok()
            .or_else(|| self.unprotected_security.try_receive().ok())
    }

    pub async fn ready(&self) {
        select6(
            self.terminal.ready_to_receive(),
            self.security.ready_to_receive(),
            self.unprotected_security.ready_to_receive(),
            self.power_save_delivery.ready_to_receive(),
            self.receiver.ready_to_receive(),
            self.he_observation.ready_to_receive(),
        )
        .await;
    }

    pub fn len(&self) -> usize {
        self.terminal.len()
            + self.security.len()
            + self.unprotected_security.len()
            + self.power_save_delivery.len()
            + self.receiver.len()
            + self.he_observation.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Whether work other than the delivery reserved by an active PS-Poll is
    /// queued. A PS-Poll TX completion must not treat its own already-arrived
    /// response as unrelated traffic and force an AP-visible PM=0 transition.
    pub fn non_power_save_delivery_pending(&self) -> bool {
        !self.terminal.is_empty()
            || !self.security.is_empty()
            || !self.unprotected_security.is_empty()
            || !self.receiver.is_empty()
            || !self.he_observation.is_empty()
    }

    pub fn security_pending(&self) -> bool {
        !self.security.is_empty() || !self.unprotected_security.is_empty()
    }

    /// Whether this mailbox epoch has lost any semantic control event.
    ///
    /// This is functional protocol state, not a diagnostic counter. Once set,
    /// the connected owner must fail closed and may only clear it by returning
    /// every endpoint and starting a fresh split epoch.
    pub fn overflowed(&self) -> bool {
        self.overflowed.load(Ordering::Acquire)
    }

    /// Number of HE control events coalesced by the single-slot runtime lane.
    /// Losing one does not invalidate Association/BlockAck state; the lane is
    /// intentionally best-effort because a later dequeue could not recover an
    /// already missed response window.
    pub fn dropped_he_observations(&self) -> u32 {
        self.dropped_he_observations.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use open_esp_radio_embassy_net::NoopRawMutex;
    use open_esp_radio_esp32s31_wifi_mac::tx_ampdu::BlockAckAction;
    use open_esp_radio_esp32s31_wifi_sta::connected_rx::{
        ConnectedRxControlEvent, ConnectedRxEvent, ConnectedRxSink,
    };
    use open_esp_radio_ieee80211::{
        data::EthernetFrameParts,
        station::{StaDisconnect, StaDisconnectKind},
    };

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
        assert!(receiver.overflowed());
        assert_eq!(
            receiver.try_receive(),
            Some(ConnectedRxControlEvent::BlockAck(first))
        );
        assert_eq!(receiver.try_receive(), None);
    }

    #[test]
    fn peer_disconnect_has_a_reserved_terminal_slot_and_priority() {
        let resources = ConnectedControlResources::<NoopRawMutex, 1>::new();
        let (mut publisher, receiver) = resources.split();
        let action = BlockAckAction::Delba {
            tid: 1,
            initiator: true,
            reason: 2,
        };
        let disconnect = StaDisconnect {
            kind: StaDisconnectKind::Disassociation,
            reason_code: 8,
        };

        publisher.publish(ConnectedRxEvent::BlockAck {
            action,
            body: &[3, 2, 0, 0, 2, 0],
        });
        publisher.publish(ConnectedRxEvent::PeerDisconnect(disconnect));

        assert_eq!(receiver.len(), 2);
        assert!(!receiver.overflowed());
        assert_eq!(
            receiver.try_receive(),
            Some(ConnectedRxControlEvent::PeerDisconnect(disconnect))
        );
        assert_eq!(
            receiver.try_receive(),
            Some(ConnectedRxControlEvent::BlockAck(action))
        );
    }

    #[test]
    fn connected_eapol_uses_the_security_lane_and_never_the_control_fifo() {
        let resources = ConnectedControlResources::<NoopRawMutex, 1>::new();
        let (mut publisher, receiver) = resources.split();
        let ap = [2, 0, 0, 0, 0, 2];
        let packet = open_esp_radio_wpa2::frames::Wpa2TxFrame::<512>::group_message1(
            [2, 0, 0, 0, 0, 1],
            3,
            [0; 8],
            &[0x55; 24],
        )
        .unwrap();

        publisher.publish(ConnectedRxEvent::Ethernet {
            frame: EthernetFrameParts {
                destination: [2, 0, 0, 0, 0, 1],
                source: ap,
                ether_type: EAPOL_ETHERTYPE,
                payload: packet.as_bytes(),
            },
            raw: &[],
            amsdu: false,
            metadata: open_esp_radio_wifi_softmac::MacRxMetadata::unavailable(),
        });

        assert_eq!(receiver.try_receive(), None);
        let ConnectedSecurityFrame::Protected(received) = receiver
            .try_receive_security()
            .expect("EAPOL must use the security lane")
        else {
            panic!("Ethernet EAPOL must retain protected provenance")
        };
        assert_eq!(received.peer(), &ap);
        assert_eq!(received.as_bytes(), packet.as_bytes());
        assert!(!receiver.overflowed());
    }

    #[test]
    fn protected_eapol_overflow_remains_fail_closed() {
        let resources = ConnectedControlResources::<NoopRawMutex, 1>::new();
        let (mut publisher, receiver) = resources.split();
        let ap = [2, 0, 0, 0, 0, 2];
        let packet = open_esp_radio_wpa2::frames::Wpa2TxFrame::<512>::group_message1(
            [2, 0, 0, 0, 0, 1],
            3,
            [0; 8],
            &[0x55; 24],
        )
        .unwrap();
        let event = || ConnectedRxEvent::Ethernet {
            frame: EthernetFrameParts {
                destination: [2, 0, 0, 0, 0, 1],
                source: ap,
                ether_type: EAPOL_ETHERTYPE,
                payload: packet.as_bytes(),
            },
            raw: &[],
            amsdu: false,
            metadata: open_esp_radio_wifi_softmac::MacRxMetadata::unavailable(),
        };

        publisher.publish(event());
        publisher.publish(event());

        assert!(receiver.overflowed());
        let ConnectedSecurityFrame::Protected(received) = receiver
            .try_receive_security()
            .expect("first protected EAPOL must remain queued")
        else {
            panic!("protected lane must retain its provenance")
        };
        assert_eq!(received.as_bytes(), packet.as_bytes());
    }

    #[test]
    fn plaintext_eapol_full_and_copy_rejection_are_peer_local_drops() {
        let resources = ConnectedControlResources::<NoopRawMutex, 1>::new();
        let (mut publisher, receiver) = resources.split();
        let ap = [2, 0, 0, 0, 0, 2];
        let first = open_esp_radio_wpa2::frames::Wpa2TxFrame::<512>::message3(
            [2, 0, 0, 0, 0, 1],
            3,
            [4; 32],
            [0; 8],
            &[0x55; 8],
        )
        .unwrap();
        let second = open_esp_radio_wpa2::frames::Wpa2TxFrame::<512>::message3(
            [2, 0, 0, 0, 0, 1],
            4,
            [5; 32],
            [0; 8],
            &[0x66; 8],
        )
        .unwrap();

        publisher.publish(ConnectedRxEvent::UnprotectedEapol {
            source: ap,
            payload: first.as_bytes(),
        });
        publisher.publish(ConnectedRxEvent::UnprotectedEapol {
            source: ap,
            payload: second.as_bytes(),
        });

        assert!(!receiver.overflowed());
        let ConnectedSecurityFrame::Unprotected(received) = receiver
            .try_receive_security()
            .expect("first plaintext EAPOL must remain queued")
        else {
            panic!("plaintext EAPOL must retain unprotected provenance")
        };
        assert_eq!(received.peer(), &ap);
        assert_eq!(received.as_bytes(), first.as_bytes());
        assert!(receiver.try_receive_security().is_none());

        publisher.publish(ConnectedRxEvent::UnprotectedEapol {
            source: ap,
            payload: &[0],
        });
        assert!(!receiver.overflowed());
        assert!(receiver.try_receive_security().is_none());
    }

    #[test]
    fn protected_security_precedes_an_earlier_plaintext_candidate() {
        let resources = ConnectedControlResources::<NoopRawMutex, 1>::new();
        let (mut publisher, receiver) = resources.split();
        let station = [2, 0, 0, 0, 0, 1];
        let ap = [2, 0, 0, 0, 0, 2];
        let message3 = open_esp_radio_wpa2::frames::Wpa2TxFrame::<512>::message3(
            station, 3, [4; 32], [0; 8], &[0x55; 8],
        )
        .unwrap();
        let group_message1 = open_esp_radio_wpa2::frames::Wpa2TxFrame::<512>::group_message1(
            station,
            4,
            [0; 8],
            &[0x66; 24],
        )
        .unwrap();

        publisher.publish(ConnectedRxEvent::UnprotectedEapol {
            source: ap,
            payload: message3.as_bytes(),
        });
        publisher.publish(ConnectedRxEvent::Ethernet {
            frame: EthernetFrameParts {
                destination: station,
                source: ap,
                ether_type: EAPOL_ETHERTYPE,
                payload: group_message1.as_bytes(),
            },
            raw: &[],
            amsdu: false,
            metadata: open_esp_radio_wifi_softmac::MacRxMetadata::unavailable(),
        });

        assert!(!receiver.overflowed());
        let ConnectedSecurityFrame::Protected(protected) = receiver
            .try_receive_security()
            .expect("protected lane must have dequeue priority")
        else {
            panic!("first dequeued security frame must be protected")
        };
        assert_eq!(protected.as_bytes(), group_message1.as_bytes());
        let ConnectedSecurityFrame::Unprotected(unprotected) = receiver
            .try_receive_security()
            .expect("plaintext candidate must remain independently queued")
        else {
            panic!("second dequeued security frame must be unprotected")
        };
        assert_eq!(unprotected.as_bytes(), message3.as_bytes());
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
            assert!(receiver.overflowed());
            assert_eq!(
                receiver.try_receive(),
                Some(ConnectedRxControlEvent::BlockAck(action))
            );
        }

        let (mut publisher, receiver) = resources.split();
        assert!(!receiver.overflowed());
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
