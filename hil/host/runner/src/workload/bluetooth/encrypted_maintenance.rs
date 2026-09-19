//! Ordered observations of live encryption, PHY restoration and later ACL progress.

use super::*;

#[derive(Default, serde::Serialize)]
pub(super) struct Observation {
    before: Option<BluetoothPeripheralEvidence>,
    restored: Option<BluetoothPeripheralEvidence>,
    traffic_after: Option<BluetoothPeripheralEvidence>,
    error: Option<String>,
}

impl Observation {
    fn observe(
        &mut self,
        start: &BluetoothPeripheralEvidence,
        current: BluetoothPeripheralEvidence,
    ) -> Result<bool> {
        let encryption = current.encryption.ok_or("missing encryption evidence")?;
        if current.terminal
            || current.saturated
            || current.host_event_faults != 0
            || current.host_acl_faults != 0
            || encryption.faults != 0
        {
            return Err("unhealthy encrypted maintenance observation".into());
        }
        if current.target_reset_commands != start.target_reset_commands
            || current.target_disconnect_commands != start.target_disconnect_commands
        {
            return Err("encrypted maintenance changed the HCI epoch".into());
        }
        if self.traffic_after.is_some() {
            return Ok(true);
        }
        if current.disconnection_complete_events != start.disconnection_complete_events {
            return Err("connection ended before ordered encrypted maintenance proof".into());
        }
        if !encryption.encrypted {
            if self.before.is_some() {
                return Err("encryption stopped during maintenance proof".into());
            }
            return Ok(false);
        }
        if start.connection_complete_events.checked_add(1)
            != Some(current.connection_complete_events)
        {
            return Err("encrypted maintenance changed the connection identity".into());
        }
        let Some(before) = &self.before else {
            // Establish actual bidirectional encrypted traffic before observing
            // maintenance, not merely an Encryption Change event.
            if current.host_acl_received_packets > start.host_acl_received_packets
                && current.host_acl_transmitted_packets > start.host_acl_transmitted_packets
            {
                self.before = Some(current);
            }
            return Ok(false);
        };
        if current.encryption != before.encryption {
            return Err("maintenance replaced the encryption session".into());
        }
        let Some(restored) = &self.restored else {
            if current.phy_peripheral_maintenance > before.phy_peripheral_maintenance {
                if super::active_maintenance_restoration_pending(&current) {
                    return Ok(false);
                }
                require_active_maintenance(before.phy_peripheral_maintenance, &current)?;
                self.restored = Some(current);
            }
            return Ok(false);
        };
        if current.host_acl_received_packets > restored.host_acl_received_packets
            && current.host_acl_transmitted_packets > restored.host_acl_transmitted_packets
        {
            self.traffic_after = Some(current);
            return Ok(true);
        }
        Ok(false)
    }
}

pub(super) fn connect_observed(
    capture: &SerialCapture,
    start: &BluetoothPeripheralEvidence,
    observation: &mut Observation,
    connect: impl FnOnce() -> Result<bluetooth::model::ConnectionReset> + Send,
) -> Result<bluetooth::model::ConnectionReset> {
    std::thread::scope(|scope| {
        // Keep the canonical bounded helper lifetime and restoration path. A
        // serial observation failure never detaches its process or worker.
        let peer = scope.spawn(connect);
        let sampling = (|| -> Result<()> {
            let until = Instant::now() + Duration::from_secs(40);
            while !peer.is_finished() {
                let current =
                    capture.bluetooth_peripheral(BluetoothPeripheralOperation::Snapshot)?;
                if observation.observe(start, current)? {
                    return Ok(());
                }
                if Instant::now() >= until {
                    return Err("encrypted maintenance observation deadline expired".into());
                }
                oer_process::sleep(Duration::from_millis(50))?;
            }
            Err("peer finished without ordered encrypted maintenance proof".into())
        })();
        observation.error = sampling.as_ref().err().map(ToString::to_string);
        let fixture = peer
            .join()
            .map_err(|_| "encrypted maintenance peer worker panicked")??;
        sampling?;
        Ok(fixture)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn start() -> BluetoothPeripheralEvidence {
        super::super::peripheral_tests::evidence()
    }

    fn live(received: u32, acknowledged: u32, maintenance: u32) -> BluetoothPeripheralEvidence {
        let mut e = start();
        e.connection_complete_events = 1;
        e.host_acl_received_packets = received;
        e.host_acl_transmitted_packets = acknowledged;
        e.phy_peripheral_maintenance = maintenance;
        e.encryption = Some(open_esp_radio_hil_protocol::BluetoothEncryptionEvidence {
            enabled: true,
            encrypted: true,
            key_requests: 1,
            key_replies: 1,
            encryption_changes: 1,
            ..Default::default()
        });
        e.phy_maintenance = Some(
            open_esp_radio_hil_protocol::BluetoothPhyMaintenanceEvidence {
                restored: maintenance,
                ..Default::default()
            },
        );
        e
    }

    #[test]
    fn physical_completion_waits_for_the_same_guarded_run() {
        let mut proof = Observation::default();
        assert!(!proof.observe(&start(), live(10, 1, 2)).unwrap());
        let mut pending = live(10, 1, 3);
        let m = pending.phy_maintenance.as_mut().unwrap();
        m.restored = 2;
        m.physical_finished_at_micros = Some(100);
        m.restoration_deadline_micros = Some(200);
        assert!(!proof.observe(&start(), pending).unwrap());
        assert!(proof.restored.is_none());
        assert!(!proof.observe(&start(), live(10, 1, 3)).unwrap());
        assert!(proof.observe(&start(), live(20, 2, 3)).unwrap());
    }

    #[test]
    fn requires_traffic_on_both_sides_of_new_maintenance() {
        let mut proof = Observation::default();
        assert!(!proof.observe(&start(), live(10, 1, 2)).unwrap());
        assert!(!proof.observe(&start(), live(10, 1, 3)).unwrap());
        assert!(!proof.observe(&start(), live(20, 1, 3)).unwrap());
        assert!(proof.observe(&start(), live(20, 2, 3)).unwrap());
    }

    #[test]
    fn rejects_session_replacement_and_invalid_restoration() {
        for bad in 0..4 {
            let mut proof = Observation::default();
            proof.observe(&start(), live(10, 1, 2)).unwrap();
            let mut e = live(10, 1, 3);
            match bad {
                0 => e.encryption.as_mut().unwrap().key_requests += 1,
                1 => e.phy_maintenance.as_mut().unwrap().restored = 2,
                2 => e.target_reset_commands += 1,
                _ => e.disconnection_complete_events += 1,
            }
            assert!(proof.observe(&start(), e).is_err());
        }
    }

    #[test]
    fn old_maintenance_or_missing_initial_traffic_cannot_satisfy_proof() {
        let mut proof = Observation::default();
        assert!(!proof.observe(&start(), live(0, 0, 2)).unwrap());
        assert!(proof.before.is_none());
        assert!(!proof.observe(&start(), live(10, 1, 2)).unwrap());
        assert!(!proof.observe(&start(), live(20, 2, 2)).unwrap());
        assert!(proof.restored.is_none());
    }
}
