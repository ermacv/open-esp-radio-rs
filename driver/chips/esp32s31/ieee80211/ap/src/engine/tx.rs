//! Peer-bound data encoding and transmit sequence/key commit boundaries.
//! The engine retains the single service, key and beacon owners.

use super::*;

impl<'storage> Esp32s31ApEngine<'storage> {
    /// Prepare the AP-originated TID-0 ADDBA request for an authorized HT
    /// peer. The peer table owns both the negotiation and its timer token.
    pub fn prepare_tx_block_ack_request(
        &mut self,
        peer: [u8; 6],
        now_micros: u64,
        output: &mut [u8],
    ) -> Result<Option<(usize, TxBlockAckAlarm)>, Esp32s31ApEngineError> {
        let Some(request) = self.service.begin_tx_block_ack(peer, now_micros)? else {
            return Ok(None);
        };
        let sequence = self.service.next_management_sequence();
        let length = ApActionFrame {
            access_point: self.service.address(),
            peer,
            sequence_number: sequence,
            body: &request.body,
        }
        .encode(output)?;
        #[cfg(any(feature = "diagnostics", test))]
        self.observe(Esp32s31ApEngineObservationEvent::TxBlockAckRequestPrepared);
        Ok(Some((length, request.alarm)))
    }

    pub fn tx_block_ack_agreement(&self, peer: [u8; 6]) -> Option<OperationalTxBlockAck> {
        self.service.peer_status(peer)?.tx_block_ack
    }

    pub fn bind_aggregate_peer(
        &self,
        peer: [u8; 6],
    ) -> Result<(Esp32s31ApAggregateBinding, ApPeerStatus), Esp32s31ApEngineError> {
        let service_binding = self
            .service
            .bind_peer(peer)
            .ok_or(ApServiceError::UnknownPeer)?;
        let status = self
            .service
            .bound_peer_status(service_binding)
            .ok_or(ApServiceError::UnknownPeer)?;
        let security = self.security.bind_pairwise(peer, status.association_id)?;
        Ok((
            Esp32s31ApAggregateBinding {
                peer: service_binding,
                security,
            },
            status,
        ))
    }

    pub fn has_operational_tx_block_ack(&self) -> bool {
        self.service.has_operational_tx_block_ack()
    }

    pub fn smallest_operational_tx_block_ack_window(&self) -> Option<u16> {
        self.service.smallest_operational_tx_block_ack_window()
    }

    pub fn observe_tx_block_ack_alarm(
        &mut self,
        peer: [u8; 6],
        alarm: TxBlockAckAlarm,
    ) -> Result<bool, Esp32s31ApEngineError> {
        let expired = self.service.on_tx_block_ack_alarm(peer, alarm)?;
        if expired {
            #[cfg(any(feature = "diagnostics", test))]
            self.observe(Esp32s31ApEngineObservationEvent::TxBlockAckNegotiationTimeout);
        }
        Ok(expired)
    }

    /// Encode one authenticator EAPOL action as an unprotected AP data MPDU.
    /// The sequence number is consumed only when the complete frame fits.
    pub fn encode_eapol<const N: usize>(
        &mut self,
        peer: [u8; 6],
        frame: &Wpa2TxFrame<N>,
        output: &mut [u8],
    ) -> Result<usize, Esp32s31ApEngineError> {
        let sequence_number = self.service.current_data_sequence();
        let len = ApDataFrame {
            access_point: self.service.address(),
            destination: peer,
            sequence_number,
            ether_type: 0x888e,
            payload: frame.as_bytes(),
        }
        .encode(output)?;
        // Consume protocol state in ordinary code. Keeping this call inside
        // `debug_assert_eq!` made release builds transmit every AP data MPDU
        // with sequence number zero because the assertion expression is
        // compiled out.
        let consumed_sequence_number = self.service.next_data_sequence();
        debug_assert_eq!(consumed_sequence_number, sequence_number);
        Ok(len)
    }

    /// Capacity-admit one downlink and explicitly retain the More Data bit.
    ///
    /// Open yields a plaintext non-QoS MPDU and no key selector; WPA2 encodes
    /// a zero CCMP placeholder. Neither path advances sequence or PN until
    /// the AP MAC admits every ordinary retry rate and commits the token.
    pub(crate) fn encode_protected_ethernet_with_more_data(
        &self,
        destination: [u8; 6],
        ethernet: &[u8],
        output: &mut [u8],
        more_data: bool,
    ) -> Result<Esp32s31ApPreparedDataFrame, Esp32s31ApEngineError> {
        if self.service.security_mode() == WifiSecurityMode::Open {
            let group = destination[0] & 1 != 0;
            if group {
                if self.service.authorized_count() == 0 {
                    return Err(ApServiceError::WrongPeerPhase.into());
                }
            } else if self.service.peer_status(destination).is_none() {
                return Err(ApServiceError::UnknownPeer.into());
            } else if !self.service.is_authorized(destination) {
                return Err(ApServiceError::WrongPeerPhase.into());
            }
            let sequence_number = self.service.current_data_sequence();
            let length = ApUnprotectedDataFrame {
                access_point: self.service.address(),
                peer: destination,
                sequence_number,
                more_data,
                ethernet,
            }
            .encode(output)?;
            return Ok(Esp32s31ApPreparedDataFrame {
                length,
                peer: destination,
                sequence_number,
                sequence_space: Esp32s31ApDataSequenceSpace::NonQos,
                hardware_key_selector: None,
            });
        }
        let group = destination[0] & 1 != 0;
        let (hardware_key_selector, peer_qos) = if group {
            if self.service.authorized_count() == 0 {
                return Err(ApServiceError::WrongPeerPhase.into());
            }
            (self.security.group_hardware_index()?, false)
        } else {
            if self.service.peer_status(destination).is_none() {
                return Err(ApServiceError::UnknownPeer.into());
            }
            if !self.service.is_authorized(destination) {
                return Err(ApServiceError::WrongPeerPhase.into());
            }
            let status = self
                .service
                .peer_status(destination)
                .ok_or(ApServiceError::UnknownPeer)?;
            (
                self.security.pairwise_hardware_index(destination)?,
                status.qos_supported,
            )
        };
        let sequence_number = if peer_qos {
            self.service
                .current_qos_sequence(destination, open_esp_radio_wifi_ap::AP_TX_BLOCK_ACK_TID)
                .expect("AP TX data TID is representable")
        } else {
            self.service.current_data_sequence()
        };
        let length = ApProtectedDataFrame {
            access_point: self.service.address(),
            peer: destination,
            sequence_number,
            user_priority: 0,
            peer_qos,
            more_data,
            // Complete geometry, peer, QoS and output admission before the
            // monotonic pairwise/group PN owner advances.
            ccmp_header: [0; 8],
            ethernet,
        }
        .encode(output)?;
        Ok(Esp32s31ApPreparedDataFrame {
            length,
            peer: destination,
            sequence_number,
            sequence_space: if peer_qos {
                Esp32s31ApDataSequenceSpace::Qos
            } else {
                Esp32s31ApDataSequenceSpace::NonQos
            },
            hardware_key_selector: Some(hardware_key_selector),
        })
    }

    /// Commit one ordinary AP frame only after protection admission.
    pub(crate) fn commit_prepared_data(
        &mut self,
        prepared: Esp32s31ApPreparedDataFrame,
        output: &mut [u8],
    ) -> Result<Esp32s31ApProtectedFrame, Esp32s31ApEngineError> {
        if let Some(selector) = prepared.hardware_key_selector {
            let group = prepared.peer[0] & 1 != 0;
            let ccmp_header = if group {
                if self.security.group_hardware_index()? != selector {
                    return Err(Esp32s31ApSecurityError::WrongPeer.into());
                }
                self.security.next_group_tx_ccmp_header()?
            } else {
                if self.security.pairwise_hardware_index(prepared.peer)? != selector {
                    return Err(Esp32s31ApSecurityError::WrongPeer.into());
                }
                self.security.next_pairwise_tx_ccmp_header(prepared.peer)?
            };
            let ccmp_offset = match prepared.sequence_space {
                Esp32s31ApDataSequenceSpace::Qos => IEEE80211_QOS_DATA_HEADER_LEN,
                Esp32s31ApDataSequenceSpace::NonQos => IEEE80211_LEGACY_DATA_HEADER_LEN,
            };
            output[ccmp_offset..ccmp_offset + ccmp_header.len()].copy_from_slice(&ccmp_header);
        }
        let consumed_sequence_number = match prepared.sequence_space {
            Esp32s31ApDataSequenceSpace::Qos => self
                .service
                .next_qos_sequence(prepared.peer, open_esp_radio_wifi_ap::AP_TX_BLOCK_ACK_TID)
                .expect("AP TX data TID is representable"),
            Esp32s31ApDataSequenceSpace::NonQos => self.service.next_data_sequence(),
        };
        debug_assert_eq!(consumed_sequence_number, prepared.sequence_number);
        Ok(Esp32s31ApProtectedFrame {
            length: prepared.length,
            hardware_key_selector: prepared.hardware_key_selector,
        })
    }

    /// Encode exactly two ordered AP network leases as one prepared QoS
    /// A-MSDU without consuming sequence or CCMP ownership.
    ///
    /// `Ok(None)` is a non-mutating admission miss: the caller may transmit
    /// the first lease normally and retain the second without reordering. A
    /// WPA2 epoch additionally requires the exact TID-0 BlockAck agreement to
    /// have negotiated A-MSDU support. Open HT peers use ordinary ACK policy.
    /// Only the AP MAC may turn the returned value into a committed frame,
    /// after its ordinary initial/retry rate series passes protection policy.
    pub(crate) fn encode_amsdu_ethernet_pair(
        &self,
        first: &[u8],
        second: &[u8],
        output: &mut [u8],
    ) -> Result<Option<Esp32s31ApPreparedAmsduFrame>, Esp32s31ApEngineError> {
        let Some(destination) = first
            .get(..6)
            .and_then(|bytes| <[u8; 6]>::try_from(bytes).ok())
        else {
            return Ok(None);
        };
        if destination[0] & 1 != 0
            || second.get(..6) != Some(destination.as_slice())
            || first.len() < 14
            || second.len() < 14
        {
            return Ok(None);
        }
        let Some(status) = self.service.peer_status(destination) else {
            return Ok(None);
        };
        if status.phase != ApPeerPhase::Authorized || !status.qos_supported || status.ht.is_none() {
            return Ok(None);
        }
        let protected = self.service.security_mode() != WifiSecurityMode::Open;
        if protected
            && !status.tx_block_ack.is_some_and(|agreement| {
                agreement.tid == open_esp_radio_wifi_ap::AP_TX_BLOCK_ACK_TID && agreement.amsdu
            })
        {
            return Ok(None);
        }

        let sequence_number = self
            .service
            .current_qos_sequence(destination, open_esp_radio_wifi_ap::AP_TX_BLOCK_ACK_TID)
            .expect("AP A-MSDU TID is representable");
        let ethernet_frames = [first, second];
        let placeholder_ccmp = protected.then_some([0; 8]);
        let length = match (ApAmsduFrame {
            access_point: self.service.address(),
            peer: destination,
            sequence_number,
            user_priority: open_esp_radio_wifi_ap::AP_TX_BLOCK_ACK_TID,
            more_data: false,
            ccmp_header: placeholder_ccmp,
            ethernet_frames: &ethernet_frames,
        })
        .encode(output)
        {
            Ok(length) => length,
            Err(
                ApDataFrameError::OutputTooSmall { .. }
                | ApDataFrameError::AmsduTooLong { .. }
                | ApDataFrameError::EthernetFrameTooShort,
            ) => return Ok(None),
            Err(error) => return Err(error.into()),
        };

        let hardware_key_selector = if protected {
            Some(self.security.pairwise_hardware_index(destination)?)
        } else {
            None
        };
        Ok(Some(Esp32s31ApPreparedAmsduFrame {
            length,
            peer: destination,
            sequence_number,
            hardware_key_selector,
        }))
    }

    /// Commit the monotonic security/sequence state of one prepared A-MSDU.
    /// The caller must complete TX-protection admission before this edge.
    pub(crate) fn commit_prepared_amsdu(
        &mut self,
        prepared: Esp32s31ApPreparedAmsduFrame,
        output: &mut [u8],
    ) -> Result<Esp32s31ApProtectedFrame, Esp32s31ApEngineError> {
        if let Some(selector) = prepared.hardware_key_selector {
            if self.security.pairwise_hardware_index(prepared.peer)? != selector {
                return Err(Esp32s31ApSecurityError::WrongPeer.into());
            }
            let ccmp_header = self.security.next_pairwise_tx_ccmp_header(prepared.peer)?;
            output
                [IEEE80211_QOS_DATA_HEADER_LEN..IEEE80211_QOS_DATA_HEADER_LEN + ccmp_header.len()]
                .copy_from_slice(&ccmp_header);
        }
        let consumed = self
            .service
            .next_qos_sequence(prepared.peer, open_esp_radio_wifi_ap::AP_TX_BLOCK_ACK_TID)
            .expect("AP A-MSDU TID is representable");
        debug_assert_eq!(consumed, prepared.sequence_number);
        Ok(Esp32s31ApProtectedFrame {
            length: prepared.length,
            hardware_key_selector: prepared.hardware_key_selector,
        })
    }

    /// AP-specific adapter from a network allocation to the role-neutral
    /// retained A-MPDU backing contract.
    ///
    /// Saturated AP TX executes this leaf once per MPDU (roughly 150,000
    /// calls per 16-second BA16 HIL interval). The PSRAM-code profile keeps
    /// only this measured synchronous encoder leaf in the semantic hot-text
    /// class; the linker, not the portable AP model, selects physical SRAM.
    #[cfg_attr(
        target_arch = "riscv32",
        unsafe(link_section = ".hot.text.open_radio_ap_tx_encode")
    )]
    #[inline(never)]
    pub fn encode_aggregate_ethernet_in_place(
        &mut self,
        binding: Esp32s31ApAggregateBinding,
        storage: &mut [u8],
        ethernet_offset: usize,
        ethernet_length: usize,
    ) -> Result<Esp32s31ApAggregateFrame, Esp32s31ApEngineError> {
        let status = self
            .service
            .bound_peer_status(binding.peer)
            .ok_or(ApServiceError::UnknownPeer)?;
        if status.phase != ApPeerPhase::Authorized
            || !status.qos_supported
            || status.tx_block_ack.is_none()
        {
            return Err(ApServiceError::WrongPeerPhase.into());
        }
        let peer = binding.peer();
        let hardware_key_selector = binding.security.hardware_index();
        let sequence_number = self
            .service
            .current_qos_sequence(peer, open_esp_radio_wifi_ap::AP_TX_BLOCK_ACK_TID)
            .expect("AP TX data TID is representable");
        let encoded = ApProtectedDataFrame {
            access_point: self.service.address(),
            peer,
            sequence_number,
            user_priority: open_esp_radio_wifi_ap::AP_TX_BLOCK_ACK_TID,
            peer_qos: true,
            more_data: false,
            ccmp_header: [0; 8],
            ethernet: &[],
        }
        .encode_in_place(storage, ethernet_offset, ethernet_length)?;
        let ccmp_header = self
            .security
            .next_bound_pairwise_tx_ccmp_header(binding.security)?;
        let ccmp_offset = encoded.offset + IEEE80211_QOS_DATA_HEADER_LEN;
        storage[ccmp_offset..ccmp_offset + ccmp_header.len()].copy_from_slice(&ccmp_header);
        let consumed = self
            .service
            .next_qos_sequence(peer, open_esp_radio_wifi_ap::AP_TX_BLOCK_ACK_TID)
            .expect("AP TX data TID is representable");
        debug_assert_eq!(consumed, sequence_number);
        Ok(Esp32s31ApAggregateFrame {
            encoded,
            hardware_key_selector,
            sequence_number,
        })
    }
}
