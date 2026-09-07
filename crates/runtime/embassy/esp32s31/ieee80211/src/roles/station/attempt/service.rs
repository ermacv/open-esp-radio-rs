use super::*;

impl<
    'hardware,
    'transmit,
    'storage,
    'scratch,
    'security,
    H,
    C,
    D,
    T,
    J,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> StaAttemptPort
    for StaAttemptTargetPort<
        StaAttemptTargetOwner<
            'hardware,
            'transmit,
            'storage,
            'scratch,
            'security,
            H,
            C,
            D,
            T,
            J,
            COUNT,
            DMA_BUFFER_SIZE,
            DMA_STORAGE_SIZE,
        >,
    >
where
    H: RxDma
        + TxHardware
        + StaLinkRxPolicyHardware
        + StaNoiseFloorHardware
        + He20PeerHardware
        + BeamformingReportHardware
        + CcmpKeyHardware
        + MacRuntimeStopHardware
        + 'hardware,
    C: StaAttemptChannel<H>,
    D: RxFrontierDelay,
    T: StaJoinTransmit<H> + HandshakeTransmit<H> + StaPeerTransmit + 'transmit,
    J: StaJoinObserver + Default,
{
    type Owner = StaAttemptTargetOwner<
        'hardware,
        'transmit,
        'storage,
        'scratch,
        'security,
        H,
        C,
        D,
        T,
        J,
        COUNT,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
    >;
    type Connected = StaAttemptConnected<Self::Owner>;
    type Error =
        StaAttemptTargetError<<T as StaJoinTransmit<H>>::Error, <T as HandshakeTransmit<H>>::Error>;

    async fn prepare_candidate<'a>(
        &'a mut self,
        owner: &'a mut Self::Owner,
    ) -> Result<(), StaAttemptStepError<Self::Error>> {
        // This is the first attempt edge and precedes candidate TX policy
        // or channel/MMIO mutation. A public caller may only pair a
        // station request with security material of the same exact mode;
        // never reinterpret missing WPA material as an Open attempt.
        if owner.station.security != owner.security.mode() {
            return Err(StaAttemptStepError::terminal(
                StaAttemptTargetError::Security(StaSecurityError::SecurityModeMismatch),
            ));
        }
        owner.prepared_peer = None;
        owner.association = None;
        owner.connected_peer = None;
        owner.pending_keys = None;
        owner.installed_security = None;
        owner.report = StaAttemptReport::default();
        owner.report.security = Some(match owner.security.mode() {
            WifiSecurityMode::Open => {
                StaAttemptSecurityExecution::OpenHandshakeAndKeyInstallSkipped
            }
            WifiSecurityMode::Wpa2Personal => StaAttemptSecurityExecution::Wpa2Personal,
        });
        owner.prepared_peer = Some(
            StaPeerPort::prepare(owner.transmit, &owner.station.access_point).map_err(|error| {
                StaAttemptStepError::terminal(StaAttemptTargetError::Candidate(error))
            })?,
        );
        Ok(())
    }

    async fn select_channel<'a>(
        &'a mut self,
        owner: &'a mut Self::Owner,
    ) -> Result<(), StaAttemptStepError<Self::Error>> {
        let selection = select_association(
            &owner.station.access_point,
            owner.station.association_preference,
        );
        owner
            .channel
            .switch_channel(
                owner.hardware,
                selection.channel_or_frequency,
                selection.cbw,
            )
            .await
            .map_err(|error| {
                StaAttemptStepError::retry_current(StaAttemptTargetError::Channel(error))
            })
    }

    async fn authenticate<'a>(
        &'a mut self,
        owner: &'a mut Self::Owner,
    ) -> Result<(), StaAttemptStepError<Self::Error>> {
        let receive = owner.receive.take().ok_or_else(|| {
            StaAttemptStepError::terminal(StaAttemptTargetError::State(
                StaAttemptStateError::MissingReceive,
            ))
        })?;
        let mut port = StaJoinPort::new(
            StaJoinRadio::new(
                &mut *owner.hardware,
                StaJoinRx::new(receive, owner.rx_storage),
                &mut *owner.transmit,
            ),
            StaJoinStorage::new(owner.frame, J::default()),
            StaJoinStation::new(
                owner.station.station_address,
                owner.station.access_point,
                owner.station.association_preference,
            )
            .with_listen_interval(owner.listen_interval)
            .with_security(owner.station.security),
        );
        port.prepare_authentication();
        let mut runner = StaJoinRunner::new(port, EmbassyStaJoinTimer);
        let result = runner
            .authenticate(
                owner.station.station_address,
                owner.station.access_point.bssid,
                owner.security.sequences.non_qos_mut(),
            )
            .await;
        let (port, _) = runner.into_parts();
        owner.receive = Some(port.into_receive().into_owner());
        match result {
            Ok(success) => {
                owner.report.authentication = Some(success);
                Ok(())
            }
            Err(error) => Err(StaAttemptStepError::retry_current(
                StaAttemptTargetError::Authentication(error),
            )),
        }
    }

    async fn associate<'a>(
        &'a mut self,
        owner: &'a mut Self::Owner,
    ) -> Result<(), StaAttemptStepError<Self::Error>> {
        let receive = owner.receive.take().ok_or_else(|| {
            StaAttemptStepError::terminal(StaAttemptTargetError::State(
                StaAttemptStateError::MissingReceive,
            ))
        })?;
        let port = StaJoinPort::new(
            StaJoinRadio::new(
                &mut *owner.hardware,
                StaJoinRx::new(receive, owner.rx_storage),
                &mut *owner.transmit,
            ),
            StaJoinStorage::new(owner.frame, J::default()),
            StaJoinStation::new(
                owner.station.station_address,
                owner.station.access_point,
                owner.station.association_preference,
            )
            .with_listen_interval(owner.listen_interval)
            .with_security(owner.station.security),
        );
        let mut runner = StaJoinRunner::new(port, EmbassyStaJoinTimer);
        let result = runner
            .associate(
                owner.station.station_address,
                owner.station.access_point.bssid,
                owner.station.security,
                owner.security.sequences.non_qos_mut(),
            )
            .await;
        let (port, _) = runner.into_parts();
        owner.receive = Some(port.into_receive().into_owner());
        match result {
            Ok(success) => {
                owner.association = Some(success.response);
                owner.report.association = Some(success);
                Ok(())
            }
            Err(error) => Err(StaAttemptStepError::retry_current(
                StaAttemptTargetError::Association(error),
            )),
        }
    }

    async fn program_peer<'a>(
        &'a mut self,
        owner: &'a mut Self::Owner,
    ) -> Result<(), StaAttemptStepError<Self::Error>> {
        let prepared = owner.prepared_peer.take().ok_or_else(|| {
            StaAttemptStepError::terminal(StaAttemptTargetError::State(
                StaAttemptStateError::MissingPreparedPeer,
            ))
        })?;
        let response = owner.association.ok_or_else(|| {
            StaAttemptStepError::terminal(StaAttemptTargetError::State(
                StaAttemptStateError::MissingAssociation,
            ))
        })?;
        let association_phy = select_association(
            &owner.station.access_point,
            owner.station.association_preference,
        )
        .phy;
        let ProgrammedStaPeer { peer, report } = StaPeerPort::program(
            StaPeerRadio::new(&mut *owner.hardware, &mut *owner.transmit),
            StaPeerStation::new(owner.station.station_address, association_phy),
            &response,
            prepared,
        )
        .map_err(|error| StaAttemptStepError::terminal(StaAttemptTargetError::Peer(error)))?;
        owner.connected_peer = Some(peer);
        owner.report.peer = Some(report);
        Ok(())
    }

    async fn run_wpa2_handshake<'a>(
        &'a mut self,
        owner: &'a mut Self::Owner,
    ) -> Result<(), StaAttemptStepError<Self::Error>> {
        if owner.security.mode() == WifiSecurityMode::Open {
            owner.installed_security = Some(StaInstalledSecurity::Open);
            return Ok(());
        }
        if owner.connected_peer.is_none() {
            return Err(StaAttemptStepError::terminal(StaAttemptTargetError::State(
                StaAttemptStateError::MissingConnectedPeer,
            )));
        }
        let selected_rsn = select_wpa2_psk_rsn(&owner.station.access_point).map_err(|error| {
            StaAttemptStepError::terminal(StaAttemptTargetError::Security(error))
        })?;
        let receive = owner.receive.take().ok_or_else(|| {
            StaAttemptStepError::terminal(StaAttemptTargetError::State(
                StaAttemptStateError::MissingReceive,
            ))
        })?;
        let station = Wpa2Station::new(
            owner.station.station_address,
            owner.station.access_point.bssid,
        );
        let port = Wpa2HandshakePort::new(
            Wpa2HandshakeRadio::new(
                &mut *owner.hardware,
                Wpa2Rx::new(receive, owner.rx_storage, station),
                &mut *owner.transmit,
            ),
            Wpa2HandshakeStorage::new(owner.frame),
            station,
        );
        let mut runner =
            Wpa2HandshakeRunner::new(port, EmbassyWpa2HandshakeTimer, Wpa2SoftwareAes::new());
        let (pmk, supplicant_nonce, sequences) =
            owner.security.wpa2_handshake_parts().ok_or_else(|| {
                StaAttemptStepError::terminal(StaAttemptTargetError::State(
                    StaAttemptStateError::MissingConnectedSecurity,
                ))
            })?;
        let mut next_sequence = || sequences.take_non_qos();
        let result = runner
            .run(
                Wpa2HandshakeConfig {
                    local: owner.station.station_address,
                    authenticator: owner.station.access_point.bssid,
                    supplicant_nonce,
                    association_security_ies: selected_rsn.as_bytes(),
                    authenticator_rsn_ie: owner.station.access_point.rsn_ie_bytes(),
                    authenticator_rsnxe: owner.station.access_point.rsnxe_bytes(),
                    pmk,
                },
                &mut next_sequence,
            )
            .await;
        let telemetry = runner.backend().telemetry();
        let (port, _, _) = runner.into_parts();
        owner.receive = Some(port.into_receive().into_owner());
        owner.report.wpa2_handshake = Some(telemetry);
        match result {
            Ok(pending) => {
                owner.pending_keys = Some(pending);
                Ok(())
            }
            Err(error) => Err(StaAttemptStepError::retry_current(
                StaAttemptTargetError::Wpa2Handshake(error),
            )),
        }
    }

    async fn install_wpa2_keys<'a>(
        &'a mut self,
        owner: &'a mut Self::Owner,
    ) -> Result<(), StaAttemptStepError<Self::Error>> {
        if owner.security.mode() == WifiSecurityMode::Open {
            return Ok(());
        }
        let pending = owner.pending_keys.take().ok_or_else(|| {
            StaAttemptStepError::terminal(StaAttemptTargetError::State(
                StaAttemptStateError::MissingHandshake,
            ))
        })?;
        let link = owner
            .connected_peer
            .as_ref()
            .ok_or_else(|| {
                StaAttemptStepError::terminal(StaAttemptTargetError::State(
                    StaAttemptStateError::MissingConnectedPeer,
                ))
            })?
            .link;
        let (_, _, message4_protection) = owner.security.wpa2_material().ok_or_else(|| {
            StaAttemptStepError::terminal(StaAttemptTargetError::State(
                StaAttemptStateError::MissingConnectedSecurity,
            ))
        })?;
        let port = Wpa2KeyPort::new(
            Wpa2KeyRadio::new(&mut *owner.hardware, &mut *owner.transmit),
            Wpa2KeySession::new(
                Wpa2Station::new(link.station_address, link.bssid),
                link.peer_qos,
                &mut owner.security.sequences,
                message4_protection,
            ),
        );
        let mut runner = Wpa2KeyInstallRunner::new(port);
        let result: Result<Wpa2Established<InstalledWpa2Keys>, _> = runner.run(pending).await;
        let port = runner.into_backend();
        let completion = port.completion();
        let _parts = port.into_parts();
        owner.report.message4 = completion;
        match result {
            Ok(established) => {
                owner.report.wpa2 = Some(established.metadata());
                let (keys, connected) = established.into_parts();
                let (pairwise, group, group_material, replay) = keys.into_parts();
                owner.installed_security = Some(StaInstalledSecurity::Wpa2Personal {
                    pairwise,
                    group,
                    group_material,
                    replay,
                });
                if !owner.security.set_connected(connected) {
                    return Err(StaAttemptStepError::terminal(StaAttemptTargetError::State(
                        StaAttemptStateError::MissingConnectedSecurity,
                    )));
                }
                Ok(())
            }
            Err(error) => Err(StaAttemptStepError::retry_current(
                StaAttemptTargetError::Wpa2KeyInstall(error),
            )),
        }
    }

    // Only the transferred owner belongs in this future. Capturing the unused
    // adapter as well expands code generation for the enclosing attempt future.
    #[expect(
        clippy::manual_async_fn,
        reason = "the explicit owner-only future avoids code expansion in the enclosing station attempt"
    )]
    fn enter_connected(
        &mut self,
        owner: Self::Owner,
    ) -> impl Future<
        Output = Result<Self::Connected, StaConnectedEntryFailure<Self::Owner, Self::Error>>,
    > + '_ {
        async move {
            let missing = if owner.connected_peer.is_none() {
                Some(StaAttemptStateError::MissingConnectedPeer)
            } else if owner.installed_security.is_none() {
                Some(StaAttemptStateError::MissingKeys)
            } else if owner.security.mode() == WifiSecurityMode::Wpa2Personal
                && !owner.security.has_connected_wpa2()
            {
                Some(StaAttemptStateError::MissingConnectedSecurity)
            } else {
                None
            };
            match missing {
                Some(error) => Err(StaConnectedEntryFailure::new(
                    owner,
                    StaFailureDisposition::Terminal,
                    StaAttemptTargetError::State(error),
                )),
                None => Ok(StaAttemptConnected::new(owner)),
            }
        }
    }
}
