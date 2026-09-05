//! AP management dispatch, association responses and handshake initiation.
//! The engine retains the single service, key and beacon owners.

use super::*;

impl<'storage> Esp32s31ApEngine<'storage> {
    /// Build handshake message one after the successful Association Response
    /// has reached TX completion.
    pub fn begin_wpa2<const N: usize>(
        &self,
        peer: [u8; 6],
    ) -> Result<Wpa2TxFrame<N>, Esp32s31ApEngineError> {
        Ok(self.service.begin_wpa2_frame(peer)?)
    }

    /// Encode one AP recipient response to a peer-originated BlockAck action.
    /// Agreement state, hardware publication and TX completion remain owned
    /// by the caller; the engine owns only AP addressing and sequence space.
    pub fn encode_rx_block_ack_response(
        &mut self,
        peer: [u8; 6],
        body: &[u8],
        output: &mut [u8],
    ) -> Result<usize, Esp32s31ApEngineError> {
        let sequence = self.service.next_management_sequence();
        Ok(ApActionFrame {
            access_point: self.service.address(),
            peer,
            sequence_number: sequence,
            body,
        }
        .encode(output)?)
    }

    pub fn handle_management<H: Esp32s31ApRuntimeHardware>(
        &mut self,
        hardware: &mut H,
        frame: &[u8],
        authenticator_nonce: [u8; 32],
        initial_replay_counter: u64,
        now_micros: u64,
        output: &mut [u8],
    ) -> Result<Esp32s31ApManagementOutcome, Esp32s31ApEngineError> {
        let Some(request) = parse_ap_management_request(
            &crate::profile::ADVERTISEMENT,
            frame,
            self.service.address(),
        ) else {
            return Ok(Esp32s31ApManagementOutcome::Ignored);
        };
        let retry = frame.get(1).is_some_and(|byte| byte & 0x08 != 0);
        match request {
            ApManagementRequest::OpenAuthentication { peer } => {
                if retry
                    && self
                        .service
                        .peer_status(peer)
                        .is_some_and(|status| status.phase != ApPeerPhase::Authenticated)
                {
                    // libnet80211 routes retry/duplicate management frames as
                    // ordinary receive outcomes; it does not tear down its
                    // station node merely because an earlier response ACK was
                    // lost. Re-emit success while preserving the current WPA2
                    // or authorized key epoch. A non-retry authentication
                    // below still owns the explicit reauthentication reset.
                    let sequence = self.service.next_management_sequence();
                    let len = write_open_authentication_response(
                        output,
                        self.service.address(),
                        peer,
                        0,
                        sequence,
                    )?;
                    #[cfg(any(feature = "diagnostics", test))]
                    self.observe(Esp32s31ApEngineObservationEvent::AuthenticationResponsePrepared);
                    return Ok(Esp32s31ApManagementOutcome::Response {
                        len,
                        begin_wpa2: false,
                    });
                }
                // A peer may restart authentication without first sending a
                // deauthentication frame. End its old pairwise-key epoch
                // before the portable service resets the handshake state;
                // otherwise the stable AID would still own a stale hardware
                // entry and the next authorization could not install its PTK.
                if self.service.peer_status(peer).is_some() {
                    self.security.clear_peer(hardware, peer)?;
                }
                let ApMlmeAction::AuthenticationResponse { status, .. } =
                    self.service.authenticate_open(peer, now_micros)
                else {
                    unreachable!("authenticate_open has one response action")
                };
                let sequence = self.service.next_management_sequence();
                let len = write_open_authentication_response(
                    output,
                    self.service.address(),
                    peer,
                    status,
                    sequence,
                )?;
                #[cfg(any(feature = "diagnostics", test))]
                self.observe(Esp32s31ApEngineObservationEvent::AuthenticationResponsePrepared);
                Ok(Esp32s31ApManagementOutcome::Response {
                    len,
                    begin_wpa2: false,
                })
            }
            ApManagementRequest::Association {
                peer,
                security,
                maximum_legacy_rate_500kbps,
                ht_capabilities,
                qos_supported,
            } => {
                let Some(peer_status) = self.service.peer_status(peer) else {
                    // Vendor `hostap_recv_mgmt` treats a peer lookup miss as
                    // an on-air management outcome (including explicit
                    // deauthentication paths), never as a task failure. This
                    // bounded port has no deauthentication response for the
                    // class-2 case yet, so retain the owner and ignore it.
                    return Ok(Esp32s31ApManagementOutcome::Ignored);
                };
                if self.service.matches_association_security(security)
                    && (peer_status.phase == ApPeerPhase::Securing
                        || (self.service.security_mode() == WifiSecurityMode::Open
                            && peer_status.phase == ApPeerPhase::Authorized))
                {
                    // A station can repeat Association Request when the first
                    // response ACK was lost. Preserve the in-flight WPA2
                    // state, retransmit the same successful association and
                    // do not start a second Message-1 transaction.
                    let sequence = self.service.next_management_sequence();
                    let len = write_ht_association_response_frame_for_security(
                        &crate::profile::ADVERTISEMENT,
                        output,
                        self.service.address(),
                        peer,
                        0,
                        peer_status.association_id,
                        sequence,
                        self.channel,
                        peer_status.ht,
                        self.service.security_mode(),
                    )?;
                    #[cfg(any(feature = "diagnostics", test))]
                    self.observe(
                        Esp32s31ApEngineObservationEvent::AssociationResponsePrepared {
                            associated_peers: self.service.associated_count(),
                        },
                    );
                    return Ok(Esp32s31ApManagementOutcome::Response {
                        len,
                        begin_wpa2: false,
                    });
                }
                if peer_status.phase != ApPeerPhase::Authenticated {
                    return Ok(Esp32s31ApManagementOutcome::Ignored);
                }
                let capabilities = ApAssociationCapabilities {
                    maximum_legacy_rate_500kbps,
                    ht: ht_capabilities,
                    qos_supported,
                };
                let action = match self.service.security_mode() {
                    WifiSecurityMode::Open => {
                        self.service
                            .associate_open(peer, security, capabilities, now_micros)?
                    }
                    WifiSecurityMode::Wpa2Personal => self.service.associate_wpa2(
                        peer,
                        security,
                        capabilities,
                        authenticator_nonce,
                        initial_replay_counter,
                        now_micros,
                    )?,
                };
                let ApMlmeAction::AssociationResponse {
                    status,
                    association_id,
                    ..
                } = action
                else {
                    unreachable!("AP association has one response action")
                };
                let sequence = self.service.next_management_sequence();
                let len = write_ht_association_response_frame_for_security(
                    &crate::profile::ADVERTISEMENT,
                    output,
                    self.service.address(),
                    peer,
                    status,
                    association_id.unwrap_or(0),
                    sequence,
                    self.channel,
                    ht_capabilities,
                    self.service.security_mode(),
                )?;
                if association_id.is_some() {
                    // Recovered `ic_set_sta` evidence gives the legacy B/G
                    // path a software transmit-rate context, represented here
                    // by the peer's negotiated rate and stable AID. Its extra
                    // station-programming calls are HE-only. Pairwise hardware
                    // state is installed later through the AID-owned key slot,
                    // so the current B/G AP has no unevidenced station-table
                    // MMIO operation to imitate.
                }
                #[cfg(any(feature = "diagnostics", test))]
                self.observe(
                    Esp32s31ApEngineObservationEvent::AssociationResponsePrepared {
                        associated_peers: self.service.associated_count(),
                    },
                );
                #[cfg(any(feature = "diagnostics", test))]
                if association_id.is_some()
                    && self.service.security_mode() == WifiSecurityMode::Open
                {
                    self.observe(Esp32s31ApEngineObservationEvent::PeerAuthorized {
                        authorized_peers: self.service.authorized_count(),
                    });
                }
                Ok(Esp32s31ApManagementOutcome::Response {
                    len,
                    begin_wpa2: association_id.is_some()
                        && self.service.security_mode() == WifiSecurityMode::Wpa2Personal,
                })
            }
            ApManagementRequest::Disassociation { peer, .. }
            | ApManagementRequest::Deauthentication { peer, .. } => {
                let Some(peer_status) = self.service.peer_status(peer) else {
                    return Ok(Esp32s31ApManagementOutcome::Ignored);
                };
                // Once local timeout/stop teardown owns the peer, its ordered
                // disassociation -> deauthentication -> key clear transaction
                // is authoritative. A peer response racing that transaction
                // must not remove the state below the in-flight TX owner.
                if peer_status.phase == ApPeerPhase::Closing {
                    return Ok(Esp32s31ApManagementOutcome::Ignored);
                }
                self.security.clear_peer(hardware, peer)?;
                self.service.remove_peer(peer)?;
                #[cfg(any(feature = "diagnostics", test))]
                self.observe(Esp32s31ApEngineObservationEvent::PeerRemoved);
                Ok(Esp32s31ApManagementOutcome::PeerRemoved { peer })
            }
            ApManagementRequest::BlockAck { peer, action } => {
                if self.service.peer_status(peer).is_none() {
                    return Ok(Esp32s31ApManagementOutcome::Ignored);
                }
                if let Some(response) = self.service.on_tx_block_ack_action(peer, action)? {
                    #[cfg(any(feature = "diagnostics", test))]
                    self.observe(Esp32s31ApEngineObservationEvent::TxBlockAckResponseObserved);
                    match response {
                        TxBlockAckResponse::Operational(_) => {
                            #[cfg(any(feature = "diagnostics", test))]
                            self.observe(Esp32s31ApEngineObservationEvent::TxBlockAckOperational);
                        }
                        TxBlockAckResponse::Rejected(_) => {
                            #[cfg(any(feature = "diagnostics", test))]
                            self.observe(Esp32s31ApEngineObservationEvent::TxBlockAckRejected);
                        }
                    }
                }
                Ok(Esp32s31ApManagementOutcome::Ignored)
            }
        }
    }
}
