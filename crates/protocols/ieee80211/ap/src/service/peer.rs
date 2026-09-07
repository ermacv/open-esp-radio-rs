//! Peer authentication, association, sequence spaces and closure.
//! Every method borrows the same service and peer storage; no second owner exists.

use super::*;

impl<'peers> AccessPointService<'peers> {
    pub fn next_management_sequence(&mut self) -> u16 {
        let sequence = self.next_management_sequence;
        self.next_management_sequence = (sequence + 1) & 0x0fff;
        sequence
    }

    /// Consume the non-QoS data sequence space used by the initial EAPOL and
    /// legacy data path. Per-TID sequence spaces are introduced with QoS.
    pub fn next_data_sequence(&mut self) -> u16 {
        let sequence = self.next_data_sequence;
        self.next_data_sequence = (sequence + 1) & 0x0fff;
        sequence
    }

    pub const fn current_data_sequence(&self) -> u16 {
        self.next_data_sequence
    }

    /// Consume one per-peer/per-TID sequence for protected data or the
    /// bounded Open QoS A-MSDU path. Security mode does not partition the
    /// receiver's IEEE sequence space.
    pub fn next_qos_sequence(&mut self, peer: [u8; 6], tid: u8) -> Option<u16> {
        let sequence = self
            .checked_peer_mut(peer)
            .ok()?
            .next_qos_sequences
            .get_mut(usize::from(tid))?;
        let current = *sequence;
        *sequence = (current + 1) & 0x0fff;
        Some(current)
    }

    /// Inspect a peer/TID sequence without consuming it during preflight.
    pub fn current_qos_sequence(&self, peer: [u8; 6], tid: u8) -> Option<u16> {
        self.checked_peer(peer)
            .ok()?
            .next_qos_sequences
            .get(usize::from(tid))
            .copied()
    }

    pub fn authenticate_open(&mut self, peer: [u8; 6], now_micros: u64) -> ApMlmeAction {
        let (status, changed) = if let Some(index) = self.peer_index(peer) {
            let association_id = self.storage().peers[index]
                .as_ref()
                .expect("peer index resolves an occupied entry")
                .association_id;
            self.advance_peer_generation();
            let association_epoch = self.storage().generation;
            self.storage_mut().peers[index] = Some(ApPeer::authenticated(
                peer,
                association_id,
                association_epoch,
                now_micros,
            ));
            (AP_STATUS_SUCCESS, true)
        } else if self.occupied_count() >= self.client_limit.get() {
            (AP_STATUS_TOO_MANY_STATIONS, false)
        } else if let Some(index) = self.storage().peers.iter().position(Option::is_none) {
            let association_id = u16::try_from(index + 1).expect("fifteen AIDs fit u16");
            self.advance_peer_generation();
            let association_epoch = self.storage().generation;
            self.storage_mut().peers[index] = Some(ApPeer::authenticated(
                peer,
                association_id,
                association_epoch,
                now_micros,
            ));
            (AP_STATUS_SUCCESS, true)
        } else {
            (AP_STATUS_TOO_MANY_STATIONS, false)
        };
        if changed {
            self.revise_status();
        }
        ApMlmeAction::AuthenticationResponse { peer, status }
    }

    pub fn associate_wpa2(
        &mut self,
        peer: [u8; 6],
        security: ApAssociationSecurityObservation<'_>,
        capabilities: ApAssociationCapabilities,
        authenticator_nonce: [u8; 32],
        initial_replay_counter: u64,
        now_micros: u64,
    ) -> Result<ApMlmeAction, ApServiceError> {
        if self.security_mode() != WifiSecurityMode::Wpa2Personal {
            return Err(ApServiceError::SecurityModeMismatch);
        }
        let association_security_ies = if security.malformed_elements || security.legacy_wpa_present
        {
            None
        } else {
            Self::validated_wpa2_association_security_ies(security)
        };
        let security_matches = association_security_ies.is_some();
        let association_security_binding = match association_security_ies.as_ref() {
            Some(ies) => Some(
                self.wpa2_material()?
                    .0
                    .bind_association_security_ies(ies.as_bytes()),
            ),
            None => None,
        };
        let access_point = self.address;
        let inactive_timeout_micros = self.inactive_timeout.micros();
        let existing = self.checked_peer_mut(peer)?;
        if existing.phase != ApPeerPhase::Authenticated {
            return Err(ApServiceError::WrongPeerPhase);
        }
        if !security_matches {
            return Ok(ApMlmeAction::AssociationResponse {
                peer,
                status: AP_STATUS_INVALID_RSN,
                association_id: None,
            });
        }
        if capabilities.maximum_legacy_rate_500kbps == 0 {
            return Ok(ApMlmeAction::AssociationResponse {
                peer,
                status: AP_STATUS_UNSUPPORTED_RATES,
                association_id: None,
            });
        }
        let wpa2 = Wpa2ApState::new(
            access_point,
            peer,
            authenticator_nonce,
            initial_replay_counter,
        )?;
        existing.phase = ApPeerPhase::Securing;
        existing.wpa2 = Some(wpa2);
        existing.association_security_binding = association_security_binding;
        existing.maximum_legacy_rate_500kbps = capabilities.maximum_legacy_rate_500kbps;
        existing.ht = capabilities.ht;
        existing.qos_supported = capabilities.qos_supported;
        existing.last_activity_micros = now_micros;
        existing.deadline_micros = now_micros.saturating_add(inactive_timeout_micros);
        let association_id = existing.association_id;
        self.revise_status();
        Ok(ApMlmeAction::AssociationResponse {
            peer,
            status: AP_STATUS_SUCCESS,
            association_id: Some(association_id),
        })
    }

    /// Admit an association into an explicitly Open AP epoch.
    ///
    /// An empty RSN body is the exact contract: a mixed or WPA-capable
    /// request is not silently downgraded. Authorization is immediate and no
    /// authenticator, PTK, GTK or hardware key owner is created.
    pub fn associate_open(
        &mut self,
        peer: [u8; 6],
        security: ApAssociationSecurityObservation<'_>,
        capabilities: ApAssociationCapabilities,
        now_micros: u64,
    ) -> Result<ApMlmeAction, ApServiceError> {
        if self.security_mode() != WifiSecurityMode::Open {
            return Err(ApServiceError::SecurityModeMismatch);
        }
        let security_matches = self.matches_association_security(security);
        let inactive_timeout_micros = self.inactive_timeout.micros();
        let existing = self.checked_peer_mut(peer)?;
        if existing.phase != ApPeerPhase::Authenticated {
            return Err(ApServiceError::WrongPeerPhase);
        }
        if !security_matches {
            return Ok(ApMlmeAction::AssociationResponse {
                peer,
                status: AP_STATUS_INVALID_RSN,
                association_id: None,
            });
        }
        if capabilities.maximum_legacy_rate_500kbps == 0 {
            return Ok(ApMlmeAction::AssociationResponse {
                peer,
                status: AP_STATUS_UNSUPPORTED_RATES,
                association_id: None,
            });
        }
        existing.phase = ApPeerPhase::Authorized;
        existing.wpa2 = None;
        existing.association_security_binding = None;
        existing.pending_ptk = None;
        existing.maximum_legacy_rate_500kbps = capabilities.maximum_legacy_rate_500kbps;
        existing.ht = capabilities.ht;
        // Ordinary Open MSDUs retain the non-QoS sequence space. The bounded
        // A-MSDU owner uses this peer's independent QoS/TID-0 counter only
        // after validating HT and QoS support for both coalesced leases.
        existing.qos_supported = capabilities.qos_supported;
        existing.tx_block_ack.stop();
        existing.last_activity_micros = now_micros;
        existing.deadline_micros = now_micros.saturating_add(inactive_timeout_micros);
        let association_id = existing.association_id;
        self.revise_status();
        Ok(ApMlmeAction::AssociationResponse {
            peer,
            status: AP_STATUS_SUCCESS,
            association_id: Some(association_id),
        })
    }

    pub fn observe_activity(
        &mut self,
        peer: [u8; 6],
        now_micros: u64,
    ) -> Result<(), ApServiceError> {
        let binding = self.bind_peer(peer).ok_or(ApServiceError::UnknownPeer)?;
        self.observe_bound_activity(binding, now_micros)
    }

    /// Refresh activity through a generation-bound O(1) peer identity.
    ///
    /// The RX data path resolves a transmitter once and reuses this capability
    /// across an in-order burst. Slot reuse invalidates the binding before any
    /// replacement peer can inherit the previous activity deadline.
    pub fn observe_bound_activity(
        &mut self,
        binding: ApPeerBinding,
        now_micros: u64,
    ) -> Result<(), ApServiceError> {
        let inactive_timeout_micros = self.inactive_timeout.micros();
        let existing = self
            .bound_peer_mut(binding)
            .ok_or(ApServiceError::UnknownPeer)?;
        if !matches!(
            existing.phase,
            ApPeerPhase::Securing | ApPeerPhase::Authorized
        ) {
            return Err(ApServiceError::WrongPeerPhase);
        }
        existing.last_activity_micros = now_micros;
        existing.deadline_micros = now_micros.saturating_add(inactive_timeout_micros);
        Ok(())
    }

    /// Coalesced equivalent of [`Self::observe_bound_activity`] for admitted
    /// data frames whose only role is keeping an already-associated peer live.
    pub fn observe_bound_data_activity(
        &mut self,
        binding: ApPeerBinding,
        now_micros: u64,
    ) -> Result<(), ApServiceError> {
        let inactive_timeout_micros = self.inactive_timeout.micros();
        let refresh_margin_micros = inactive_timeout_micros / 2;
        let existing = self
            .bound_peer_mut(binding)
            .ok_or(ApServiceError::UnknownPeer)?;
        if !matches!(
            existing.phase,
            ApPeerPhase::Securing | ApPeerPhase::Authorized
        ) {
            return Err(ApServiceError::WrongPeerPhase);
        }
        if existing.deadline_micros <= now_micros.saturating_add(refresh_margin_micros) {
            existing.last_activity_micros = now_micros;
            existing.deadline_micros = now_micros.saturating_add(inactive_timeout_micros);
        }
        Ok(())
    }

    pub fn next_peer_deadline(&self) -> Option<u64> {
        self.storage()
            .peers
            .iter()
            .flatten()
            .filter(|peer| peer.phase != ApPeerPhase::Closing)
            .map(|peer| peer.deadline_micros)
            .min()
    }

    pub fn begin_due_peer_close(&mut self, now_micros: u64) -> Option<ApPeerClose> {
        let index = self.storage().peers.iter().position(|peer| {
            peer.as_ref().is_some_and(|peer| {
                peer.phase != ApPeerPhase::Closing && peer.deadline_micros <= now_micros
            })
        })?;
        let peer = self.storage_mut().peers[index].as_mut()?;
        let was_associated = matches!(peer.phase, ApPeerPhase::Securing | ApPeerPhase::Authorized);
        let close = ApPeerClose {
            peer: peer.address,
            kind: if was_associated {
                ApPeerCloseKind::InactivityTimeout
            } else {
                ApPeerCloseKind::AuthenticationTimeout
            },
            was_associated,
            maximum_legacy_rate_500kbps: peer.maximum_legacy_rate_500kbps,
        };
        peer.phase = ApPeerPhase::Closing;
        self.revise_status();
        Some(close)
    }

    pub fn begin_wpa2_failure_close(
        &mut self,
        peer_address: [u8; 6],
    ) -> Result<ApPeerClose, ApServiceError> {
        let peer = self.checked_peer_mut(peer_address)?;
        if peer.phase != ApPeerPhase::Securing {
            return Err(ApServiceError::WrongPeerPhase);
        }
        peer.wpa2_retry.cancel();
        peer.wpa2_retry_alarm = None;
        let close = ApPeerClose {
            peer: peer.address,
            kind: ApPeerCloseKind::Wpa2HandshakeFailure,
            was_associated: true,
            maximum_legacy_rate_500kbps: peer.maximum_legacy_rate_500kbps,
        };
        peer.phase = ApPeerPhase::Closing;
        self.revise_status();
        Ok(close)
    }

    pub fn begin_stop_peer(&mut self) -> Option<ApPeerClose> {
        let index = self.storage().peers.iter().position(|peer| {
            peer.as_ref()
                .is_some_and(|peer| peer.phase != ApPeerPhase::Closing)
        })?;
        let peer = self.storage_mut().peers[index].as_mut()?;
        let was_associated = matches!(peer.phase, ApPeerPhase::Securing | ApPeerPhase::Authorized);
        let close = ApPeerClose {
            peer: peer.address,
            kind: ApPeerCloseKind::AccessPointStop,
            was_associated,
            maximum_legacy_rate_500kbps: peer.maximum_legacy_rate_500kbps,
        };
        peer.phase = ApPeerPhase::Closing;
        self.revise_status();
        Some(close)
    }

    pub fn remove_peer(&mut self, peer: [u8; 6]) -> Result<ApMlmeAction, ApServiceError> {
        let index = self.peer_index(peer).ok_or(ApServiceError::UnknownPeer)?;
        self.storage_mut().peers[index] = None;
        self.advance_peer_generation();
        self.revise_status();
        Ok(ApMlmeAction::PeerRemoved { peer })
    }
}
