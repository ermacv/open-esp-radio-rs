#[cfg(target_os = "linux")]
mod peer {
    use super::super::{
        Result, connection_reset,
        hci::Socket,
        model::{
            PeerAddress,
            security_failure::{Observation, Report},
        },
    };
    use bt_hci::{
        FromHciBytes,
        cmd::{
            Cmd,
            le::LeEnableEncryption,
            link_control::{Disconnect, ReadRemoteVersionInformation},
        },
        event::Event,
        param::{ConnHandle, EncryptionEnabledLevel, Status},
    };
    use open_esp_radio_hil_protocol::BluetoothSecurityFailure as Failure;
    use open_esp_radio_hil_protocol::{
        BLUETOOTH_TEST_EDIV, BLUETOOTH_TEST_LTK, BLUETOOTH_TEST_RAND,
    };
    use std::time::{Duration, Instant};

    #[derive(Debug, PartialEq, Eq)]
    enum Action {
        Wait,
        ReadVersion,
        Disconnect,
        Complete,
    }

    pub(crate) fn run(user: &Socket, peer: PeerAddress, report: &mut Report) -> Result<()> {
        let (handle, _, _) = connection_reset::connect(user, peer)?;
        report.connected = true;
        let started = Instant::now();
        user.send_command(&LeEnableEncryption::new(
            handle,
            BLUETOOTH_TEST_RAND,
            BLUETOOTH_TEST_EDIV,
            BLUETOOTH_TEST_LTK,
        ))?;
        let deadline = started + Duration::from_secs(8);
        loop {
            let packet = user.receive(deadline)?;
            if report.events.len() >= 64 || packet.len() > 258 {
                return Err("security failure HCI observation limit exceeded".into());
            }
            report.events.push(Observation {
                after_micros: started.elapsed().as_micros().try_into()?,
                packet: packet.clone(),
            });
            match observe(&packet, handle, report)? {
                Action::Wait => {}
                Action::ReadVersion => {
                    user.send_command(&ReadRemoteVersionInformation::new(handle))?;
                }
                Action::Disconnect => {
                    // Initial key rejection preserves the ACL. Close explicitly after proving rejection.
                    user.send_command(&Disconnect::new(
                        handle,
                        bt_hci::param::DisconnectReason::RemoteUserTerminatedConn,
                    ))?;
                    report.disconnect_requested = true;
                }
                Action::Complete => {
                    report.completed_after_micros = Some(started.elapsed().as_micros().try_into()?);
                    return Ok(());
                }
            }
        }
    }

    fn observe(packet: &[u8], handle: ConnHandle, report: &mut Report) -> Result<Action> {
        if packet.first() != Some(&4) {
            return Err("application data during rejected encryption".into());
        }
        let (event, rest) = Event::from_hci_bytes(&packet[1..])
            .map_err(|e| format!("security failure event: {e:?}"))?;
        if !rest.is_empty() {
            return Err("trailing security failure event bytes".into());
        }
        match event {
            Event::CommandStatus(s) if s.cmd_opcode == LeEnableEncryption::OPCODE => {
                if s.status != Status::SUCCESS || report.command_status {
                    return Err("failed or duplicate encryption command admission".into());
                }
                report.command_status = true;
            }
            Event::CommandStatus(s) if s.cmd_opcode == Disconnect::OPCODE => {
                if !report.disconnect_requested
                    || s.status != Status::SUCCESS
                    || report.disconnect_command_status
                {
                    return Err("invalid cleanup Disconnect status".into());
                }
                report.disconnect_command_status = true;
            }
            Event::CommandStatus(s) if s.cmd_opcode == ReadRemoteVersionInformation::OPCODE => {
                if !report.read_version_before_disconnect
                    || report.encryption_failure != Some(6)
                    || report.version_command_status
                    || s.status != Status::SUCCESS
                {
                    return Err("invalid post-rejection remote-version command status".into());
                }
                report.version_command_status = true;
            }
            Event::ReadRemoteVersionInformationComplete(e)
                if report.read_version_before_disconnect =>
            {
                if !report.version_command_status
                    || report.remote_version.is_some()
                    || e.handle != handle
                    || e.status != Status::SUCCESS
                    || (e.version.into_inner(), e.company_id, e.subversion) != (0x0d, 0xffff, 1)
                {
                    return Err("invalid post-rejection remote-version completion".into());
                }
                report.remote_version = Some((e.version.into_inner(), e.company_id, e.subversion));
                return Ok(Action::Disconnect);
            }
            Event::EncryptionChangeV1(e) => {
                let expected = match report.failure {
                    Failure::MissingKey => 6,
                    Failure::WrongKey => 8,
                };
                if !report.command_status
                    || e.handle != handle
                    || e.status != Status::new(expected)
                    || e.enabled != EncryptionEnabledLevel::Off
                    || report.encryption_failure.is_some()
                {
                    return Err("unexpected encryption result for injected key failure".into());
                }
                report.encryption_failure = Some(expected);
                if report.failure == Failure::MissingKey {
                    return Ok(if report.read_version_before_disconnect {
                        Action::ReadVersion
                    } else {
                        Action::Disconnect
                    });
                }
            }
            Event::EncryptionKeyRefreshComplete(_) => {
                return Err("unexpected refresh in initial key failure".into());
            }
            Event::DisconnectionComplete(e) => {
                let expected = match report.failure {
                    Failure::MissingKey => 0x16,
                    Failure::WrongKey => 8,
                };
                if !report.command_status
                    || e.handle != handle
                    || e.status != Status::SUCCESS
                    || e.reason != Status::new(expected)
                    || (report.failure == Failure::MissingKey
                        && (!report.disconnect_command_status
                            || report.encryption_failure != Some(6)))
                {
                    return Err("unexpected peer disconnect for key failure".into());
                }
                report.disconnect_reason = Some(expected);
                return Ok(Action::Complete);
            }
            Event::HardwareError(_) => {
                return Err("adapter hardware error during key failure".into());
            }
            _ => {}
        }
        Ok(Action::Wait)
    }

    #[cfg(test)]
    mod tests {
        use super::super::super::model::Adapter;
        use super::*;
        fn report(failure: Failure) -> Report {
            Report::new(Adapter(0), PeerAddress([1; 6]), failure)
        }
        const STATUS: [u8; 7] = [4, 15, 4, 0, 1, 0x19, 0x20];
        const VERSION_STATUS: [u8; 7] = [4, 15, 4, 0, 1, 0x1d, 0x04];
        const VERSION: [u8; 11] = [4, 12, 8, 0, 1, 0, 0x0d, 0xff, 0xff, 1, 0];

        fn rejected_with_version_probe() -> Report {
            let mut r = report(Failure::MissingKey);
            r.read_version_before_disconnect = true;
            let h = ConnHandle::new(1);
            observe(&STATUS, h, &mut r).unwrap();
            assert_eq!(
                observe(&[4, 8, 4, 6, 1, 0, 0], h, &mut r).unwrap(),
                Action::ReadVersion
            );
            r
        }

        #[test]
        fn version_diagnostic_requires_matching_completion_before_disconnect() {
            let h = ConnHandle::new(1);
            let mut r = rejected_with_version_probe();
            assert!(observe(&VERSION, h, &mut r).is_err());
            assert_eq!(observe(&VERSION_STATUS, h, &mut r).unwrap(), Action::Wait);
            assert_eq!(observe(&VERSION, h, &mut r).unwrap(), Action::Disconnect);
            assert_eq!(r.remote_version, Some((0x0d, 0xffff, 1)));
            assert!(observe(&VERSION, h, &mut r).is_err());
            assert!(observe(&VERSION_STATUS, h, &mut r).is_err());
            assert!(!r.disconnect_requested);
            r.disconnect_requested = true;
            observe(&[4, 15, 4, 0, 1, 6, 4], h, &mut r).unwrap();
            assert_eq!(
                observe(&[4, 5, 4, 0, 1, 0, 0x16], h, &mut r).unwrap(),
                Action::Complete
            );
        }

        #[test]
        fn version_diagnostic_rejects_failed_command_foreign_identity_and_early_disconnect() {
            let h = ConnHandle::new(1);
            let mut early = report(Failure::MissingKey);
            early.read_version_before_disconnect = true;
            assert!(observe(&VERSION_STATUS, h, &mut early).is_err());
            let mut rejected = rejected_with_version_probe();
            assert!(observe(&[4, 15, 4, 0x0c, 1, 0x1d, 4], h, &mut rejected).is_err());
            for (index, value) in [(3, 8), (4, 2), (6, 0x0c), (7, 0), (9, 2)] {
                let mut r = rejected_with_version_probe();
                observe(&VERSION_STATUS, h, &mut r).unwrap();
                let mut packet = VERSION;
                packet[index] = value;
                assert!(observe(&packet, h, &mut r).is_err());
                assert!(r.remote_version.is_none());
            }
            let mut r = rejected_with_version_probe();
            assert!(observe(&[4, 5, 4, 0, 1, 0, 0x16], h, &mut r).is_err());
        }
        #[test]
        fn missing_key_requires_rejection_before_explicit_disconnect() {
            let h = ConnHandle::new(1);
            let mut r = report(Failure::MissingKey);
            let rejected = [4, 8, 4, 6, 1, 0, 0];
            assert!(observe(&rejected, h, &mut r).is_err());
            observe(&STATUS, h, &mut r).unwrap();
            assert_eq!(observe(&rejected, h, &mut r).unwrap(), Action::Disconnect);
            assert!(observe(&[4, 5, 4, 0, 1, 0, 0x16], h, &mut r).is_err());
            r.disconnect_requested = true;
            observe(&[4, 15, 4, 0, 1, 6, 4], h, &mut r).unwrap();
            assert_eq!(
                observe(&[4, 5, 4, 0, 1, 0, 0x16], h, &mut r).unwrap(),
                Action::Complete
            );
        }
        #[test]
        fn wrong_key_never_accepts_success_data_foreign_handle_or_wrong_reason() {
            let h = ConnHandle::new(1);
            for packet in [
                vec![4, 8, 4, 0, 1, 0, 1],
                vec![2, 1, 0, 0, 0],
                vec![4, 5, 4, 0, 2, 0, 8],
                vec![4, 5, 4, 0, 1, 0, 0x13],
                vec![4, 5, 4, 0, 1, 0, 8, 0],
            ] {
                let mut r = report(Failure::WrongKey);
                observe(&STATUS, h, &mut r).unwrap();
                assert!(observe(&packet, h, &mut r).is_err());
            }
            let mut r = report(Failure::WrongKey);
            observe(&STATUS, h, &mut r).unwrap();
            assert_eq!(
                observe(&[4, 5, 4, 0, 1, 0, 8], h, &mut r).unwrap(),
                Action::Complete
            );
            assert!(observe(&STATUS, h, &mut r).is_err());
        }
    }
}
#[cfg(target_os = "linux")]
pub(super) use peer::run;
