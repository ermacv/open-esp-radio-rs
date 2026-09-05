//! Controlled-port admission, pairwise replay and RX peer binding.
//! The engine retains the single service, key and beacon owners.

use super::*;

impl<'storage> Esp32s31ApEngine<'storage> {
    /// Admit one AP data MPDU against the live controlled port and exact PTK
    /// generation, committing its CCMP PN before Ethernet publication.
    ///
    /// The RX dispatcher calls this only after software BlockAck release and
    /// hardware MIC verification. Keeping replay state beside the installed
    /// key makes PTK install, clear and reinstall the only reset edges.
    pub fn admit_rx_data(
        &mut self,
        request: Esp32s31ApRxAdmissionRequest,
    ) -> Esp32s31ApRxAdmission {
        let peer = request.peer();
        let rx_peer = match self.resolve_rx_peer(peer) {
            Ok(bound) => bound,
            Err(admission) => return admission,
        };
        if matches!(request.lane(), CcmpReplayLane::Tid(_)) && !rx_peer.qos_supported {
            return Esp32s31ApRxAdmission::rejected(Esp32s31ApRxError::PeerQosMismatch);
        }
        // The exact duplicate slot and association/key generations were
        // captured with this control-plane revision. AID reuse invalidates the
        // RX binding before another frame can inherit the old history.
        let duplicate_owner = rx_peer.duplicate_owner;
        match request.operation() {
            Esp32s31ApRxAdmissionOperation::Ordinary => {
                match (self.service.security_mode(), request.ccmp_header()) {
                    (WifiSecurityMode::Open, None) => {
                        Esp32s31ApRxAdmission::authorized(duplicate_owner)
                    }
                    (WifiSecurityMode::Wpa2Personal, Some(header)) => {
                        if header.key_id() != CcmpKeyId::PAIRWISE {
                            return Esp32s31ApRxAdmission::rejected(
                                Esp32s31ApRxError::PairwiseKeyId(header.key_id().value()),
                            );
                        }
                        let Some(binding) = rx_peer.pairwise else {
                            return Esp32s31ApRxAdmission::rejected(
                                Esp32s31ApRxError::KeyGenerationMismatch,
                            );
                        };
                        match self.security.commit_bound_pairwise_rx_immediate(
                            binding,
                            request.lane(),
                            header.packet_number(),
                        ) {
                            Ok(()) => Esp32s31ApRxAdmission::authorized(duplicate_owner),
                            Err(error) => rejected_rx_security(error),
                        }
                    }
                    _ => Esp32s31ApRxAdmission::rejected(Esp32s31ApRxError::SecurityModeMismatch),
                }
            }
            Esp32s31ApRxAdmissionOperation::AuthorizeFragment
            | Esp32s31ApRxAdmissionOperation::PrepareFragment => {
                let Some(header) = request.ccmp_header() else {
                    return Esp32s31ApRxAdmission::rejected(
                        Esp32s31ApRxError::SecurityModeMismatch,
                    );
                };
                if self.service.security_mode() != WifiSecurityMode::Wpa2Personal {
                    return Esp32s31ApRxAdmission::rejected(
                        Esp32s31ApRxError::SecurityModeMismatch,
                    );
                }
                if header.key_id() != CcmpKeyId::PAIRWISE {
                    return Esp32s31ApRxAdmission::rejected(Esp32s31ApRxError::PairwiseKeyId(
                        header.key_id().value(),
                    ));
                }
                let Some(binding) = rx_peer.pairwise else {
                    return Esp32s31ApRxAdmission::rejected(
                        Esp32s31ApRxError::KeyGenerationMismatch,
                    );
                };
                if matches!(
                    request.operation(),
                    Esp32s31ApRxAdmissionOperation::AuthorizeFragment
                ) {
                    return Esp32s31ApRxAdmission::authorized(duplicate_owner);
                }
                let candidate = match self.security.prepare_bound_pairwise_rx(
                    binding,
                    request.lane(),
                    header.packet_number(),
                ) {
                    Ok(candidate) => candidate,
                    Err(error) => return rejected_rx_security(error),
                };
                Esp32s31ApRxAdmission::prepared(Esp32s31ApRxPreparedReplay {
                    peer,
                    lane: request.lane(),
                    ccmp_header: header,
                    owner: duplicate_owner,
                    candidate: Esp32s31ApRxPreparedCandidate::Hardware(candidate),
                })
            }
            Esp32s31ApRxAdmissionOperation::CommitFragment(prepared) => {
                if self.service.security_mode() != WifiSecurityMode::Wpa2Personal
                    || prepared.peer != peer
                    || prepared.lane != request.lane()
                    || Some(prepared.ccmp_header) != request.ccmp_header()
                {
                    return Esp32s31ApRxAdmission::rejected(
                        Esp32s31ApRxError::SecurityModeMismatch,
                    );
                }
                let Some(_binding) = rx_peer.pairwise else {
                    return Esp32s31ApRxAdmission::rejected(
                        Esp32s31ApRxError::KeyGenerationMismatch,
                    );
                };
                if duplicate_owner != prepared.owner {
                    return Esp32s31ApRxAdmission::rejected(
                        Esp32s31ApRxError::KeyGenerationMismatch,
                    );
                }
                #[cfg(not(test))]
                let Esp32s31ApRxPreparedCandidate::Hardware(candidate) = prepared.candidate;
                #[cfg(test)]
                let candidate = match prepared.candidate {
                    Esp32s31ApRxPreparedCandidate::Hardware(candidate) => candidate,
                    Esp32s31ApRxPreparedCandidate::Model => {
                        return Esp32s31ApRxAdmission::rejected(
                            Esp32s31ApRxError::KeyGenerationMismatch,
                        );
                    }
                };
                match self.security.commit_bound_pairwise_rx(candidate) {
                    Ok(()) => Esp32s31ApRxAdmission::authorized(duplicate_owner),
                    Err(error) => rejected_rx_security(error),
                }
            }
        }
    }

    /// Admit the compact complete WPA2 pairwise request used by the saturated
    /// AP ordinary-data leaf.
    pub fn admit_ordinary_pairwise_rx(
        &mut self,
        request: Esp32s31ApOrdinaryPairwiseRxRequest,
    ) -> Esp32s31ApRxAdmission {
        let rx_peer = match self.resolve_rx_peer(request.peer()) {
            Ok(bound) => bound,
            Err(admission) => return admission,
        };
        if matches!(request.lane(), CcmpReplayLane::Tid(_)) && !rx_peer.qos_supported {
            return Esp32s31ApRxAdmission::rejected(Esp32s31ApRxError::PeerQosMismatch);
        }
        let Some(binding) = rx_peer.pairwise else {
            return Esp32s31ApRxAdmission::rejected(Esp32s31ApRxError::KeyGenerationMismatch);
        };
        match self.security.commit_bound_pairwise_rx_immediate(
            binding,
            request.lane(),
            request.ccmp_header().packet_number(),
        ) {
            Ok(()) => Esp32s31ApRxAdmission::authorized(rx_peer.duplicate_owner),
            Err(error) => rejected_rx_security(error),
        }
    }

    /// Admit one complete ordinary data MPDU and apply its peer activity
    /// through the exact binding resolved by the same transaction.
    ///
    /// The result keeps protocol admission independent from portable service
    /// errors: a caller may finish the already-committed replay/publication
    /// transaction and then surface the activity error at its role boundary.
    pub fn admit_ordinary_pairwise_rx_with_activity(
        &mut self,
        request: Esp32s31ApOrdinaryPairwiseRxRequest,
        state: ApPeerPowerState,
        now_micros: u64,
    ) -> (
        Esp32s31ApRxAdmission,
        Result<Option<ApPowerSaveAction>, Esp32s31ApEngineError>,
    ) {
        let admission = self.admit_ordinary_pairwise_rx(request);
        if admission.authorized_owner().is_none() {
            return (admission, Ok(None));
        }
        let Some(binding) = self.rx_peer else {
            return (admission, Err(ApServiceError::UnknownPeer.into()));
        };
        let activity = self
            .service
            .observe_bound_data_power_state(binding.peer, state, now_micros)
            .map(Some)
            .map_err(Esp32s31ApEngineError::from);
        (admission, activity)
    }

    /// Resolve one AP-specific peer/security context around the role-neutral
    /// data dispatcher. The common burst revalidates two affine bindings in
    /// O(1); only a peer switch or control-plane revision scans the bounded
    /// portable table.
    fn resolve_rx_peer(
        &mut self,
        peer: [u8; 6],
    ) -> Result<Esp32s31ApRxPeerBinding, Esp32s31ApRxAdmission> {
        let status_revision = self.service.status_revision();
        if let Some(binding) = self.rx_peer
            && binding.peer.address() == peer
            && binding.status_revision == status_revision
        {
            return Ok(binding);
        }

        let Some(service_binding) = self.service.bind_peer(peer) else {
            self.rx_peer = None;
            return Err(Esp32s31ApRxAdmission::unauthorized());
        };
        let Some(status) = self
            .service
            .bound_peer_status(service_binding)
            .filter(|status| status.phase == ApPeerPhase::Authorized)
        else {
            self.rx_peer = None;
            return Err(Esp32s31ApRxAdmission::unauthorized());
        };
        let pairwise = match self.service.security_mode() {
            WifiSecurityMode::Open => None,
            WifiSecurityMode::Wpa2Personal => Some(
                self.security
                    .bind_pairwise(peer, status.association_id)
                    .map_err(rejected_rx_security)?,
            ),
        };
        let Some(mut duplicate_owner) =
            Esp32s31ApRxDuplicateOwner::new(status.association_id, status.association_epoch)
        else {
            self.rx_peer = None;
            return Err(Esp32s31ApRxAdmission::rejected(
                Esp32s31ApRxError::KeyGenerationMismatch,
            ));
        };
        if let Some(pairwise) = pairwise {
            duplicate_owner = duplicate_owner.with_key_generation(pairwise.generation());
        }
        let binding = Esp32s31ApRxPeerBinding {
            peer: service_binding,
            pairwise,
            duplicate_owner,
            qos_supported: status.qos_supported,
            status_revision,
        };
        self.rx_peer = Some(binding);
        Ok(binding)
    }
}
