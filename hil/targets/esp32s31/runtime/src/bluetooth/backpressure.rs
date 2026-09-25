//! Diagnostic Host: retain one RX ACL credit without stopping HCI event reads.
use super::*;
use oer_hil_protocol::{BluetoothAclBackpressureEvidence, bluetooth_backpressure_packet};

static ENABLED: AtomicBool = AtomicBool::new(false);
static ARMED: AtomicBool = AtomicBool::new(false);
static HELD: AtomicBool = AtomicBool::new(false);
static MTU: AtomicBool = AtomicBool::new(false);
static RECEIVED: AtomicU32 = AtomicU32::new(0);
static RETURNED: AtomicU32 = AtomicU32::new(0);
static SENT: AtomicU32 = AtomicU32::new(0);
static COMPLETED: AtomicU32 = AtomicU32::new(0);
static TIMEOUT: AtomicU32 = AtomicU32::new(0);
static HELD_AT: AtomicU32 = AtomicU32::new(0);
static DISCONNECTED_AT: AtomicU32 = AtomicU32::new(0);
static DISCONNECTED_HELD: AtomicBool = AtomicBool::new(false);
static FAULTS: AtomicU32 = AtomicU32::new(0);

pub(super) fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}
pub(super) fn configure(enabled: bool) -> Result<(), ()> {
    if enabled == self::enabled()
        || HELD.load(Ordering::Relaxed)
        || ARMED.load(Ordering::Relaxed)
        || RECEIVED.load(Ordering::Relaxed) != RETURNED.load(Ordering::Relaxed)
    {
        return Err(());
    }
    ENABLED.store(enabled, Ordering::Relaxed);
    Ok(())
}
pub(super) fn snapshot() -> BluetoothAclBackpressureEvidence {
    BluetoothAclBackpressureEvidence {
        enabled: enabled(),
        armed: ARMED.load(Ordering::Relaxed),
        held: HELD.load(Ordering::Relaxed),
        mtu_exchanged: MTU.load(Ordering::Relaxed),
        received: RECEIVED.load(Ordering::Relaxed),
        returned: RETURNED.load(Ordering::Relaxed),
        sent: SENT.load(Ordering::Relaxed),
        completed: COMPLETED.load(Ordering::Relaxed),
        supervision_timeout_millis: TIMEOUT.load(Ordering::Relaxed),
        held_at_millis: HELD_AT.load(Ordering::Relaxed),
        disconnected_at_millis: DISCONNECTED_AT.load(Ordering::Relaxed),
        disconnected_while_held: DISCONNECTED_HELD.load(Ordering::Relaxed),
        faults: FAULTS.load(Ordering::Relaxed),
    }
}
fn increment(value: &AtomicU32) {
    PeripheralHostEvents::increment(value);
}
fn now() -> u32 {
    u32::try_from(Instant::now().as_millis()).expect("bounded diagnostic boot")
}
async fn return_credit(credits: &HostAclCredits) -> Result<(), ()> {
    credits
        .return_completed_packets(&[ConnHandleCompletedPackets::new(ConnHandle::new(1), 1)])
        .await
        .map_err(|_| ())?;
    increment(&RETURNED);
    Ok(())
}
pub(super) async fn hold(hold: bool, credits: &HostAclCredits) -> Result<(), ()> {
    if !enabled() {
        return Err(());
    }
    if hold {
        if !PERIPHERAL_HOST_EVENTS
            .connection_live
            .load(Ordering::Relaxed)
            || !MTU.load(Ordering::Relaxed)
            || ARMED.load(Ordering::Relaxed)
            || HELD.load(Ordering::Relaxed)
        {
            return Err(());
        }
        ARMED.store(true, Ordering::Relaxed);
        Ok(())
    } else {
        ARMED.store(false, Ordering::Relaxed);
        if HELD.load(Ordering::Relaxed) {
            return_credit(credits).await?;
            HELD.store(false, Ordering::Relaxed);
        }
        Ok(())
    }
}
pub(super) async fn receive(
    hci: &Host,
    credits: &HostAclCredits,
    packet: ControllerToHostPacket<'_>,
) {
    match packet {
        ControllerToHostPacket::Acl(packet) => {
            if packet.handle().raw() != 1
                || packet.broadcast_flag() != AclBroadcastFlag::PointToPoint
            {
                increment(&FAULTS);
                return;
            }
            increment(&RECEIVED);
            let data = packet.data();
            let first = packet.boundary_flag() == AclPacketBoundary::FirstFlushable;
            let expected = bluetooth_backpressure_packet();
            if ARMED.load(Ordering::Relaxed)
                && first
                && (7..=expected.len()).contains(&data.len())
                && data == &expected[..data.len()]
            {
                if HELD.swap(true, Ordering::Relaxed) {
                    increment(&FAULTS);
                    return;
                }
                ARMED.store(false, Ordering::Relaxed);
                HELD_AT.store(now(), Ordering::Relaxed);
                return;
            }
            if return_credit(credits).await.is_err() {
                increment(&FAULTS);
                return;
            }
            if first && data == [3, 0, 4, 0, 2, 247, 0] {
                if MTU.load(Ordering::Relaxed) {
                    increment(&FAULTS);
                    return;
                }
                let response = [3, 0, 4, 0, 3, 247, 0];
                if hci
                    .write_acl_data(&AclPacket::new(
                        ConnHandle::new(1),
                        AclPacketBoundary::FirstNonFlushable,
                        AclBroadcastFlag::PointToPoint,
                        &response,
                    ))
                    .await
                    .is_err()
                {
                    increment(&FAULTS);
                    return;
                }
                increment(&SENT);
                MTU.store(true, Ordering::Relaxed);
            }
        }
        ControllerToHostPacket::Event(bytes) => {
            match HciEvent::try_from(bytes.clone()) {
                Ok(HciEvent::Le(LeEvent::LeConnectionComplete(event))) => {
                    MTU.store(false, Ordering::Relaxed);
                    TIMEOUT.store(
                        (event.supervision_timeout.as_micros() / 1000) as u32,
                        Ordering::Relaxed,
                    );
                    HELD_AT.store(0, Ordering::Relaxed);
                    DISCONNECTED_AT.store(0, Ordering::Relaxed);
                    DISCONNECTED_HELD.store(false, Ordering::Relaxed);
                }
                Ok(HciEvent::DisconnectionComplete(_)) => {
                    DISCONNECTED_AT.store(now(), Ordering::Relaxed);
                    DISCONNECTED_HELD.store(HELD.load(Ordering::Relaxed), Ordering::Relaxed);
                }
                _ => {}
            }
            if let PeripheralHostObservation::AclCompleted(count) =
                PERIPHERAL_HOST_EVENTS.observe(ControllerToHostPacket::Event(bytes))
            {
                PeripheralHostEvents::add(&COMPLETED, count);
                if COMPLETED.load(Ordering::Relaxed) > SENT.load(Ordering::Relaxed) {
                    increment(&FAULTS);
                }
            }
        }
        _ => increment(&FAULTS),
    }
}
