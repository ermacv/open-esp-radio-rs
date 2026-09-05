//! WPA2 handshakes, key/replay transitions and retry publication.
//! Every method borrows the same service and peer storage; no second owner exists.

use super::*;

impl<'peers> AccessPointService<'peers> {
    /// Exact, non-mutating admission predicate used for both first and retry
    /// Association Requests.
    pub fn matches_association_security(
        &self,
        security: ApAssociationSecurityObservation<'_>,
    ) -> bool {
        if security.malformed_elements || security.legacy_wpa_present {
            return false;
        }
        match self.security_mode() {
            WifiSecurityMode::Open => {
                !security.privacy
                    && security.rsn_ie_count == 0
                    && security.rsn_ie.is_none()
                    && security.rsnxe_count == 0
                    && security.rsnxe.is_none()
            }
            WifiSecurityMode::Wpa2Personal => {
                Self::validated_wpa2_association_security_ies(security).is_some()
            }
        }
    }

    pub(super) fn validated_wpa2_association_security_ies(
        security: ApAssociationSecurityObservation<'_>,
    ) -> Option<OwnedAssociationSecurityIes> {
        if !security.privacy
            || security.rsn_ie_count != 1
            || security.rsnxe_count > 1
            || security.rsnxe_count == 0 && security.rsnxe.is_some()
            || security.rsnxe_count == 1 && security.rsnxe.is_none()
        {
            return None;
        }
        let rsn = validate_wpa2_ap_rsn(security.rsn_ie?).ok()?;
        OwnedAssociationSecurityIes::try_copy(rsn.owned(), security.rsnxe.unwrap_or(&[])).ok()
    }

    /// Signal that the successful Association Response reached TX complete.
    pub fn begin_wpa2(&self, peer: [u8; 6]) -> Result<ApMlmeAction, ApServiceError> {
        if self.security_mode() != WifiSecurityMode::Wpa2Personal {
            return Err(ApServiceError::SecurityModeMismatch);
        }
        let existing = self.checked_peer(peer)?;
        if existing.phase != ApPeerPhase::Securing {
            return Err(ApServiceError::WrongPeerPhase);
        }
        Ok(ApMlmeAction::BeginWpa2 { peer })
    }

    pub fn wpa2_mut(&mut self, peer: [u8; 6]) -> Result<&mut Wpa2ApState, ApServiceError> {
        if self.security_mode() != WifiSecurityMode::Wpa2Personal {
            return Err(ApServiceError::SecurityModeMismatch);
        }
        let existing = self.checked_peer_mut(peer)?;
        existing.wpa2.as_mut().ok_or(ApServiceError::WrongPeerPhase)
    }

    pub fn wpa2_authorized(&self, peer: [u8; 6]) -> Result<bool, ApServiceError> {
        let existing = self.checked_peer(peer)?;
        Ok(existing.wpa2.as_ref().map(Wpa2ApState::phase) == Some(Wpa2ApPhase::Authorized))
    }

    pub fn derive_ptk(&self, context: PtkContext) -> Result<Ptk, ApServiceError> {
        let (pmk, _) = self.wpa2_material()?;
        Ok(pmk.derive_ptk(context))
    }

    /// Build Message 1 only after the successful Association Response reached
    /// TX complete. The AP state retains the replay/nonce transaction.
    pub fn begin_wpa2_frame<const N: usize>(
        &self,
        peer: [u8; 6],
    ) -> Result<Wpa2TxFrame<N>, ApWpa2Error> {
        if self.security_mode() != WifiSecurityMode::Wpa2Personal {
            return Err(ApServiceError::SecurityModeMismatch.into());
        }
        let existing = self.checked_peer(peer)?;
        let state = existing
            .wpa2
            .as_ref()
            .ok_or(ApServiceError::WrongPeerPhase)?;
        let Wpa2ApAction::Transmit(transmit) = state.message1(false)? else {
            return Err(ApWpa2Error::UnexpectedAction);
        };
        Ok(build_ap_action_frame(state, transmit, [0; 8], &[])?)
    }

    /// Bind a terminal EAPOL-Key TX completion to the generic finite retry
    /// owner. A new handshake message replaces the previous response window;
    /// completion of a retransmission keeps the alarm already advanced by the
    /// timer edge that produced it.
    pub fn observe_wpa2_transmit(
        &mut self,
        peer: [u8; 6],
        retransmission: bool,
        acknowledged: bool,
        now_micros: u64,
    ) -> Result<bool, ApWpa2Error> {
        let inactive_timeout_micros = self.inactive_timeout.micros();
        let transmit = self
            .checked_peer(peer)?
            .wpa2
            .as_ref()
            .ok_or(ApServiceError::WrongPeerPhase)?
            .retry_transmit()?;
        let existing = self.checked_peer_mut(peer)?;
        let stage_changed = existing.wpa2_retry.pending_message() != Some(transmit.message);
        let armed = stage_changed || !retransmission;
        if armed {
            existing.wpa2_retry.cancel();
            let mut alarm = existing.wpa2_retry.arm(transmit, now_micros)?;
            // hostapd extends only the acknowledged initial M1 window. M3
            // retains the short first timeout, then uses the subsequent one.
            if acknowledged
                && transmit.message == open_esp_radio_wpa2::state::Wpa2TxMessage::PairwiseMessage1
            {
                alarm = existing
                    .wpa2_retry
                    .defer_first_after_ack(now_micros)?
                    .expect("freshly armed WPA2 retry has a first window");
            }
            existing.wpa2_retry_alarm = Some(alarm);
        }
        existing.last_activity_micros = now_micros;
        existing.deadline_micros = now_micros.saturating_add(inactive_timeout_micros);
        Ok(armed)
    }

    pub fn next_wpa2_retry_deadline(&self) -> Option<u64> {
        self.storage()
            .peers
            .iter()
            .flatten()
            .filter_map(|peer| peer.wpa2_retry_alarm.map(|alarm| alarm.deadline_us))
            .min()
    }

    /// Consume at most one due authenticator retry edge.
    pub fn take_due_wpa2_retry<const N: usize>(
        &mut self,
        now_micros: u64,
    ) -> Result<ApWpa2RetryProgress<N>, ApWpa2Error> {
        let Some(index) = self.storage().peers.iter().position(|peer| {
            peer.as_ref()
                .and_then(|peer| peer.wpa2_retry_alarm)
                .is_some_and(|alarm| alarm.deadline_us <= now_micros)
        }) else {
            return Ok(ApWpa2RetryProgress::None);
        };
        let (peer_address, action) = {
            let peer = self.storage_mut().peers[index]
                .as_mut()
                .expect("due WPA2 retry belongs to an occupied peer");
            let alarm = peer
                .wpa2_retry_alarm
                .take()
                .expect("due WPA2 retry retains its alarm");
            let action = peer.wpa2_retry.on_alarm(alarm, now_micros)?;
            (peer.address, action)
        };
        match action {
            Wpa2RetryAction::Stale => Ok(ApWpa2RetryProgress::None),
            Wpa2RetryAction::Transmit { frame, next_alarm } => {
                self.checked_peer_mut(peer_address)?.wpa2_retry_alarm = Some(next_alarm);
                let frame = match frame.message {
                    open_esp_radio_wpa2::state::Wpa2TxMessage::PairwiseMessage1 => {
                        let state = self
                            .checked_peer(peer_address)?
                            .wpa2
                            .as_ref()
                            .ok_or(ApServiceError::WrongPeerPhase)?;
                        build_ap_action_frame(state, frame, [0; 8], &[])?
                    }
                    open_esp_radio_wpa2::state::Wpa2TxMessage::PairwiseMessage3 => {
                        let ApWpa2Progress::Transmit(frame) =
                            self.build_pending_transmit(peer_address, frame)?
                        else {
                            return Err(ApWpa2Error::UnexpectedAction);
                        };
                        frame
                    }
                    _ => return Err(ApWpa2Error::UnexpectedAction),
                };
                Ok(ApWpa2RetryProgress::Transmit {
                    peer: peer_address,
                    frame,
                })
            }
            Wpa2RetryAction::Exhausted => {
                let peer = self.checked_peer_mut(peer_address)?;
                let close = ApPeerClose {
                    peer: peer.address,
                    kind: ApPeerCloseKind::Wpa2HandshakeTimeout,
                    was_associated: true,
                    maximum_legacy_rate_500kbps: peer.maximum_legacy_rate_500kbps,
                };
                peer.phase = ApPeerPhase::Closing;
                self.revise_status();
                Ok(ApWpa2RetryProgress::Close(close))
            }
        }
    }

    /// Advance the bounded authenticator state through Message 2 or Message 4.
    ///
    /// PTK derivation, MIC verification and GTK wrapping are pure bounded
    /// operations here. Hardware key installation remains an explicit later
    /// edge in the chip AP engine.
    pub fn on_eapol<const N: usize>(
        &mut self,
        peer: [u8; 6],
        frame: OwnedEapolFrame<N>,
    ) -> Result<ApWpa2Progress<N>, ApWpa2Error> {
        if self.security_mode() != WifiSecurityMode::Wpa2Personal {
            return Err(ApServiceError::SecurityModeMismatch.into());
        }
        let action = match self
            .checked_peer_mut(peer)?
            .wpa2
            .as_mut()
            .ok_or(ApServiceError::WrongPeerPhase)?
            .on_frame(frame)
        {
            Ok(action) => action,
            Err(error) if error.is_peer_input_rejection() => {
                // Unsupported, stale and otherwise unauthenticated EAPOL is
                // a peer-local receive reject, not a role-control failure.
                return Ok(ApWpa2Progress::None);
            }
            Err(error) => return Err(error.into()),
        };
        match action {
            Wpa2ApAction::None => Ok(ApWpa2Progress::None),
            Wpa2ApAction::DerivePtk {
                ticket,
                context,
                message2,
            } => self.complete_message2(peer, ticket, context, message2),
            Wpa2ApAction::VerifyMessage4Mic { ticket, message4 } => {
                let valid = {
                    let ptk = self
                        .checked_peer(peer)?
                        .pending_ptk
                        .as_ref()
                        .ok_or(ApWpa2Error::MissingPairwiseKey)?;
                    message4.key_frame().verify_mic(ptk)
                };
                let action = self
                    .checked_peer_mut(peer)?
                    .wpa2
                    .as_mut()
                    .ok_or(ApServiceError::WrongPeerPhase)?
                    .complete_message4_mic(ticket, message4, valid)?;
                match action {
                    Wpa2ApAction::AuthorizePeer => {
                        let existing = self.checked_peer_mut(peer)?;
                        existing.wpa2_retry.cancel();
                        existing.wpa2_retry_alarm = None;
                        Ok(ApWpa2Progress::AuthorizePeer)
                    }
                    Wpa2ApAction::None => Ok(ApWpa2Progress::None),
                    Wpa2ApAction::DeauthenticatePeer => Ok(ApWpa2Progress::DeauthenticatePeer),
                    _ => Err(ApWpa2Error::UnexpectedAction),
                }
            }
            Wpa2ApAction::Transmit(transmit) => self.build_pending_transmit(peer, transmit),
            Wpa2ApAction::DeauthenticatePeer => Ok(ApWpa2Progress::DeauthenticatePeer),
            _ => Err(ApWpa2Error::UnexpectedAction),
        }
    }

    fn complete_message2<const N: usize>(
        &mut self,
        peer: [u8; 6],
        ticket: open_esp_radio_wpa2::state::Wpa2Ticket,
        context: Wpa2StatePtkContext,
        message2: OwnedEapolFrame<N>,
    ) -> Result<ApWpa2Progress<N>, ApWpa2Error> {
        let ptk = self.derive_ptk(PtkContext {
            authenticator_address: context.authenticator_address,
            supplicant_address: context.supplicant_address,
            authenticator_nonce: context.authenticator_nonce,
            supplicant_nonce: context.supplicant_nonce,
        })?;
        let action = self
            .checked_peer_mut(peer)?
            .wpa2
            .as_mut()
            .ok_or(ApServiceError::WrongPeerPhase)?
            .complete_ptk(ticket, message2, true)?;
        let Wpa2ApAction::VerifyMessage2Mic { ticket, message2 } = action else {
            return Err(ApWpa2Error::UnexpectedAction);
        };
        let valid = message2.key_frame().verify_mic(&ptk);
        // The association commitment is an authenticated semantic binding.
        // Do not let attacker-controlled Key Data decide peer teardown until
        // this exact M2 has passed its PTK-derived MIC.
        let association_security_ies_match = valid
            && self
                .checked_peer(peer)?
                .association_security_binding
                .as_ref()
                .is_some_and(|binding| {
                    self.wpa2_material()
                        .is_ok_and(|(pmk, _)| binding.matches(pmk, message2.key_frame().key_data()))
                });
        let action = self
            .checked_peer_mut(peer)?
            .wpa2
            .as_mut()
            .ok_or(ApServiceError::WrongPeerPhase)?
            .complete_message2_mic(ticket, message2, valid)?;
        let ticket = match action {
            Wpa2ApAction::PrepareMessage3 { ticket } => ticket,
            Wpa2ApAction::None => return Ok(ApWpa2Progress::None),
            Wpa2ApAction::DeauthenticatePeer => {
                return Ok(ApWpa2Progress::DeauthenticatePeer);
            }
            _ => return Err(ApWpa2Error::UnexpectedAction),
        };

        if !association_security_ies_match {
            let action = self
                .checked_peer_mut(peer)?
                .wpa2
                .as_mut()
                .ok_or(ApServiceError::WrongPeerPhase)?
                .complete_message3_preparation::<N>(ticket, false)?;
            return match action {
                Wpa2ApAction::DeauthenticatePeer => Ok(ApWpa2Progress::DeauthenticatePeer),
                _ => Err(ApWpa2Error::UnexpectedAction),
            };
        }

        let (_, gtk) = self.wpa2_material()?;
        let authenticator_rsn = OwnedRsnIe::<64>::try_copy(&WPA2_PERSONAL_CCMP_PSK_RSN_IE)?;
        let plain =
            Wpa2PlainKeyData::<WPA2_PLAIN_KEY_DATA_CAPACITY>::build(&authenticator_rsn, gtk)?;
        let wrapped = software_aes128_key_wrap(ptk.kek(), plain.as_bytes())?;
        let action = self
            .checked_peer_mut(peer)?
            .wpa2
            .as_mut()
            .ok_or(ApServiceError::WrongPeerPhase)?
            .complete_message3_preparation::<N>(ticket, true)?;
        let Wpa2ApAction::Transmit(transmit) = action else {
            return Err(ApWpa2Error::UnexpectedAction);
        };
        let state = self
            .checked_peer(peer)?
            .wpa2
            .as_ref()
            .ok_or(ApServiceError::WrongPeerPhase)?;
        let response =
            build_ap_action_frame(state, transmit, [0; 8], wrapped.as_bytes())?.authenticate(&ptk);
        let existing = self.checked_peer_mut(peer)?;
        existing.pending_ptk = Some(ptk);
        // Valid M2 closes the Message-1 response window. Message 3 receives a
        // fresh schedule only after its own terminal TX completion.
        existing.wpa2_retry.cancel();
        existing.wpa2_retry_alarm = None;
        Ok(ApWpa2Progress::Transmit(response))
    }

    fn build_pending_transmit<const N: usize>(
        &self,
        peer: [u8; 6],
        transmit: open_esp_radio_wpa2::state::Wpa2Transmit,
    ) -> Result<ApWpa2Progress<N>, ApWpa2Error> {
        let existing = self.checked_peer(peer)?;
        let state = existing
            .wpa2
            .as_ref()
            .ok_or(ApServiceError::WrongPeerPhase)?;
        let ptk = existing
            .pending_ptk
            .as_ref()
            .ok_or(ApWpa2Error::MissingPairwiseKey)?;
        let (_, gtk) = self.wpa2_material()?;
        let authenticator_rsn = OwnedRsnIe::<64>::try_copy(&WPA2_PERSONAL_CCMP_PSK_RSN_IE)?;
        let plain =
            Wpa2PlainKeyData::<WPA2_PLAIN_KEY_DATA_CAPACITY>::build(&authenticator_rsn, gtk)?;
        let wrapped = software_aes128_key_wrap(ptk.kek(), plain.as_bytes())?;
        let response =
            build_ap_action_frame(state, transmit, [0; 8], wrapped.as_bytes())?.authenticate(ptk);
        Ok(ApWpa2Progress::Transmit(response))
    }

    pub fn pending_ptk(&self, peer: [u8; 6]) -> Result<&Ptk, ApServiceError> {
        self.checked_peer(peer)?
            .pending_ptk
            .as_ref()
            .ok_or(ApServiceError::WrongPeerPhase)
    }

    pub fn gtk(&self) -> Result<&Wpa2Gtk, ApServiceError> {
        self.wpa2_material().map(|(_, gtk)| gtk)
    }

    pub fn authorize(&mut self, peer: [u8; 6], now_micros: u64) -> Result<(), ApServiceError> {
        if self.security_mode() != WifiSecurityMode::Wpa2Personal {
            return Err(ApServiceError::SecurityModeMismatch);
        }
        let inactive_timeout_micros = self.inactive_timeout.micros();
        let existing = self.checked_peer_mut(peer)?;
        if existing.wpa2.as_ref().map(Wpa2ApState::phase) != Some(Wpa2ApPhase::Authorized) {
            return Err(ApServiceError::WrongPeerPhase);
        }
        existing.phase = ApPeerPhase::Authorized;
        existing.association_security_binding = None;
        existing.pending_ptk = None;
        existing.wpa2_retry.cancel();
        existing.wpa2_retry_alarm = None;
        existing.last_activity_micros = now_micros;
        existing.deadline_micros = now_micros.saturating_add(inactive_timeout_micros);
        self.revise_status();
        Ok(())
    }

    pub(super) fn wpa2_material(&self) -> Result<(&Pmk, &Wpa2Gtk), ApServiceError> {
        match &self.security {
            AccessPointSecurityMaterial::Open => Err(ApServiceError::SecurityModeMismatch),
            AccessPointSecurityMaterial::Wpa2Personal { pmk, gtk } => Ok((pmk, gtk)),
        }
    }
}
