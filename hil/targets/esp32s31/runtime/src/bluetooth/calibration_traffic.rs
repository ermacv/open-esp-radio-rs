//! HIL Host workload only: four-credit plaintext ATT notifications.
//! Production HCI, LL and PHY owners execute every RF transition.
use super::*;
use oer_hil_protocol::{BluetoothCalibrationTrafficEvidence, bluetooth_calibration_notification};
static ENABLED: AtomicBool = AtomicBool::new(false);
static MTU: AtomicBool = AtomicBool::new(false);
static INTERVAL: AtomicU32 = AtomicU32::new(0);
static SENT: AtomicU32 = AtomicU32::new(0);
static COMPLETED: AtomicU32 = AtomicU32::new(0);
static FAULTS: AtomicU32 = AtomicU32::new(0);

pub(super) fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}
pub(super) fn configure(enabled: bool) {
    ENABLED.store(enabled, Ordering::Relaxed);
    if enabled {
        MTU.store(false, Ordering::Relaxed);
        for counter in [&INTERVAL, &SENT, &COMPLETED, &FAULTS] {
            counter.store(0, Ordering::Relaxed);
        }
    }
}
pub(super) fn snapshot() -> BluetoothCalibrationTrafficEvidence {
    BluetoothCalibrationTrafficEvidence {
        enabled: enabled(),
        connected: PERIPHERAL_HOST_EVENTS
            .connection_live
            .load(Ordering::Relaxed),
        interval_micros: INTERVAL.load(Ordering::Relaxed),
        mtu_exchanged: MTU.load(Ordering::Relaxed),
        sent: SENT.load(Ordering::Relaxed),
        completed: COMPLETED.load(Ordering::Relaxed),
        faults: FAULTS.load(Ordering::Relaxed),
    }
}
fn fault() {
    PeripheralHostEvents::increment(&FAULTS);
}

pub(super) async fn burst(hci: &Host) -> Result<(), ()> {
    let before = snapshot();
    if !before.enabled
        || !before.connected
        || before.interval_micros != 7_500
        || !before.mtu_exchanged
        || before.sent != before.completed
        || before.faults != 0
    {
        return Err(());
    }
    let sequence = before.sent.checked_sub(1).ok_or(())?;
    for offset in 0..4 {
        let bytes = bluetooth_calibration_notification(sequence.checked_add(offset).ok_or(())?);
        hci.write_acl_data(&AclPacket::new(
            ConnHandle::new(1),
            AclPacketBoundary::FirstNonFlushable,
            AclBroadcastFlag::PointToPoint,
            &bytes,
        ))
        .await
        .map_err(|_| ())?;
        PeripheralHostEvents::increment(&SENT);
    }
    Ok(())
}

pub(super) async fn receive(
    hci: &Host,
    credits: &HostAclCredits,
    packet: ControllerToHostPacket<'_>,
) {
    match packet {
        ControllerToHostPacket::Acl(packet) => {
            let valid = packet.handle().raw() == 1
                && packet.boundary_flag() == AclPacketBoundary::FirstFlushable
                && packet.broadcast_flag() == AclBroadcastFlag::PointToPoint;
            let data = packet.data();
            let mtu = valid && data == [3, 0, 4, 0, 2, 247, 0] && !MTU.load(Ordering::Relaxed);
            // Linux may send a signalling packet; no response data is used to
            // satisfy the recovery obligation in this one-way traffic test.
            let signalling = valid && data.len() >= 4 && data[2..4] == [5, 0];
            if credits
                .return_completed_packets(&[ConnHandleCompletedPackets::new(packet.handle(), 1)])
                .await
                .is_err()
            {
                fault();
                return;
            }
            if mtu {
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
                    fault();
                    return;
                }
                PeripheralHostEvents::increment(&SENT);
                MTU.store(true, Ordering::Relaxed);
            } else if !signalling {
                fault();
            }
        }
        ControllerToHostPacket::Event(bytes) => {
            if let Ok(HciEvent::Le(LeEvent::LeConnectionComplete(event))) =
                HciEvent::try_from(bytes.clone())
            {
                INTERVAL.store(event.conn_interval.as_micros() as u32, Ordering::Relaxed);
            }
            if matches!(
                HciEvent::try_from(bytes.clone()),
                Ok(HciEvent::Le(LeEvent::LeConnectionUpdateComplete(_)))
            ) {
                fault();
            }
            if let PeripheralHostObservation::AclCompleted(count) =
                PERIPHERAL_HOST_EVENTS.observe(ControllerToHostPacket::Event(bytes))
            {
                PeripheralHostEvents::add(&COMPLETED, count);
                if COMPLETED.load(Ordering::Relaxed) > SENT.load(Ordering::Relaxed) {
                    fault();
                }
            }
        }
        _ => fault(),
    }
}
