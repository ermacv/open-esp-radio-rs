//! Per-peer TX Block Ack negotiation under the AP service owner.
//! Every method borrows the same service and peer storage; no second owner exists.

use super::*;

impl<'peers> AccessPointService<'peers> {
    /// Begin the AP-originated TID-0 TX BlockAck negotiation for one
    /// authorized HT peer. The agreement remains owned by that peer entry.
    pub fn begin_tx_block_ack(
        &mut self,
        peer: [u8; 6],
        now_micros: u64,
    ) -> Result<Option<AddbaRequest>, ApServiceError> {
        if self.security_mode() == WifiSecurityMode::Open {
            return Ok(None);
        }
        let starting_sequence = self
            .current_qos_sequence(peer, AP_TX_BLOCK_ACK_TID)
            .expect("AP data TID is representable");
        let peer = self.checked_peer_mut(peer)?;
        if peer.phase != ApPeerPhase::Authorized || peer.ht.is_none() || !peer.qos_supported {
            return Ok(None);
        }
        if peer.tx_block_ack.operational().is_some() || peer.tx_block_ack.is_awaiting() {
            return Ok(None);
        }
        Ok(Some(
            peer.tx_block_ack.begin(starting_sequence, now_micros)?,
        ))
    }

    pub fn on_tx_block_ack_action(
        &mut self,
        peer: [u8; 6],
        action: BlockAckAction,
    ) -> Result<Option<TxBlockAckResponse>, ApServiceError> {
        if self.security_mode() == WifiSecurityMode::Open {
            return Ok(None);
        }
        let peer = self.checked_peer_mut(peer)?;
        match action {
            BlockAckAction::AddbaResponse { .. } => {
                let response = peer.tx_block_ack.on_response_action(action)?;
                self.revise_status();
                Ok(Some(response))
            }
            // This owner represents only AP-originated TX aggregation. A
            // peer-originated DELBA (`initiator = true`) terminates the
            // independent peer -> AP agreement and must not revoke our
            // AP -> peer session. The recipient clears this TX agreement
            // with `initiator = false`.
            BlockAckAction::Delba {
                tid,
                initiator: false,
                ..
            } if tid == AP_TX_BLOCK_ACK_TID => {
                peer.tx_block_ack.stop();
                self.revise_status();
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    pub fn on_tx_block_ack_alarm(
        &mut self,
        peer: [u8; 6],
        alarm: TxBlockAckAlarm,
    ) -> Result<bool, ApServiceError> {
        if self.security_mode() == WifiSecurityMode::Open {
            return Ok(false);
        }
        let peer = self.checked_peer_mut(peer)?;
        Ok(peer.tx_block_ack.on_alarm(alarm))
    }
}
