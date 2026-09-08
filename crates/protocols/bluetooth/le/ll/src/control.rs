//! Bounded peripheral responder for feature and version exchange.
//!
//! Input must already have passed the controller's CRC and duplicate filters.
//! This module does not own SN/NESN, encryption, ACL delivery or LL teardown.
//! Optional features are deliberately absent until their procedures are closed.

/// Controller identity supplied by the product; independent of the radio vendor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeVersionInformation([u8; 5]);

impl LeVersionInformation {
    pub const fn new(version: u8, company_identifier: u16, subversion: u16) -> Self {
        let company = company_identifier.to_le_bytes();
        let subversion = subversion.to_le_bytes();
        Self([
            version,
            company[0],
            company[1],
            subversion[0],
            subversion[1],
        ])
    }
    pub const fn version(self) -> u8 {
        self.0[0]
    }
    pub const fn company_identifier(self) -> u16 {
        u16::from_le_bytes([self.0[1], self.0[2]])
    }
    pub const fn subversion(self) -> u16 {
        u16::from_le_bytes([self.0[3], self.0[4]])
    }
}

/// Payload of one response, excluding the hardware-owned data-channel header.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeControlResponse {
    bytes: [u8; 9],
    len: u8,
}

impl LeControlResponse {
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.len)]
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LePeripheralControlError {
    MalformedPdu,
    UnsupportedHeader,
    ResponseQueueFull,
    PeerTermination { reason: u8 },
    MandatoryProcedureUnavailable { opcode: u8 },
}

/// Two waiting responses plus the independently retained controller TX packet.
/// Removing a response means the memory owner accepted it, not that it was ACKed.
pub struct LePeripheralControl {
    responses: [Option<LeControlResponse>; 2],
    version_queued: bool,
    peer_version: Option<LeVersionInformation>,
}

impl LePeripheralControl {
    pub const fn new() -> Self {
        Self {
            responses: [None; 2],
            version_queued: false,
            peer_version: None,
        }
    }

    /// Dispatch one accepted unencrypted LE1M PDU in receive-list order.
    pub fn receive(
        &mut self,
        pdu: &[u8],
        local_version: Option<LeVersionInformation>,
    ) -> Result<(), LePeripheralControlError> {
        use LePeripheralControlError as Error;
        if pdu.len() < 2 || pdu.len() != usize::from(pdu[1]) + 2 {
            return Err(Error::MalformedPdu);
        }
        // CTE is outside this profile. RFU bits are ignored as required by LL.
        if pdu[0] & 0x20 != 0 {
            return Err(Error::UnsupportedHeader);
        }
        match pdu[0] & 3 {
            1 | 2 => return Ok(()), // ACL delivery remains a separate boundary.
            3 if pdu.len() >= 3 => {}
            _ => return Err(Error::MalformedPdu),
        }
        let payload = &pdu[2..];
        let opcode = payload[0];
        let mut response = LeControlResponse {
            bytes: [0; 9],
            len: 2,
        };
        match opcode {
            0x08 => {
                if payload.len() != 9 {
                    return Err(Error::MalformedPdu);
                }
                // FeatureSet_USED in octet zero and our own upper octets are
                // all zero, consistent with the bootstrap HCI feature profile.
                response.bytes[0] = 0x09;
                response.len = 9;
            }
            0x0c => {
                if payload.len() != 6 {
                    return Err(Error::MalformedPdu);
                }
                self.peer_version = Some(LeVersionInformation(
                    payload[1..].try_into().expect("validated version length"),
                ));
                if self.version_queued {
                    return Ok(());
                }
                let version =
                    local_version.ok_or(Error::MandatoryProcedureUnavailable { opcode })?;
                response.bytes[0] = 0x0c;
                response.bytes[1..6].copy_from_slice(&version.0);
                response.len = 6;
            }
            0x02 => {
                if payload.len() != 2 {
                    return Err(Error::MalformedPdu);
                }
                return Err(Error::PeerTermination { reason: payload[1] });
            }
            // Do not turn a response into another request/response loop.
            0x07 | 0x0d => {
                return if payload.len() == 2 {
                    Ok(())
                } else {
                    Err(Error::MalformedPdu)
                };
            }
            0x11 => {
                return if payload.len() == 3 {
                    Ok(())
                } else {
                    Err(Error::MalformedPdu)
                };
            }
            0x09 => {
                return if payload.len() == 9 {
                    Ok(())
                } else {
                    Err(Error::MalformedPdu)
                };
            }
            // Never claim an unsupported mandatory connection transition ran.
            0x00 | 0x01 => return Err(Error::MandatoryProcedureUnavailable { opcode }),
            _ => {
                response.bytes[0] = 0x07; // LL_UNKNOWN_RSP for unsupported LLCP.
                response.bytes[1] = opcode;
            }
        }
        let slot = self
            .responses
            .iter_mut()
            .find(|slot| slot.is_none())
            .ok_or(Error::ResponseQueueFull)?;
        *slot = Some(response);
        if opcode == 0x0c {
            self.version_queued = true;
        }
        Ok(())
    }

    pub const fn peer_version(&self) -> Option<LeVersionInformation> {
        self.peer_version
    }

    pub const fn pending_response(&self) -> Option<&LeControlResponse> {
        self.responses[0].as_ref()
    }

    /// Commit only after a CPU-owned TX graph accepted this exact response.
    pub fn response_enqueued(&mut self) {
        self.responses[0] = self.responses[1].take();
    }
}

impl Default for LePeripheralControl {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FEATURE_REQ: [u8; 11] = [3, 9, 8, 255, 255, 255, 255, 255, 255, 255, 255];

    #[test]
    fn feature_response_never_echoes_unimplemented_peer_features() {
        let mut ll = LePeripheralControl::new();
        ll.receive(&FEATURE_REQ, None).unwrap();
        assert_eq!(
            ll.pending_response().unwrap().as_bytes(),
            [9, 0, 0, 0, 0, 0, 0, 0, 0]
        );
        // Pending survives arbitrary reads while the controller queue is busy.
        assert_eq!(ll.pending_response().unwrap().as_bytes()[0], 9);
        ll.response_enqueued();
        assert!(ll.pending_response().is_none());
    }

    #[test]
    fn queued_responses_preserve_order_and_overflow_preserves_both() {
        let mut ll = LePeripheralControl::new();
        ll.receive(&FEATURE_REQ, None).unwrap();
        ll.receive(&[3, 1, 0xf1], None).unwrap();
        assert_eq!(
            ll.receive(&FEATURE_REQ, None),
            Err(LePeripheralControlError::ResponseQueueFull)
        );
        assert_eq!(ll.pending_response().unwrap().as_bytes()[0], 9);
        ll.response_enqueued();
        assert_eq!(ll.pending_response().unwrap().as_bytes(), [7, 0xf1]);
        ll.response_enqueued();
        assert!(ll.pending_response().is_none());
    }

    #[test]
    fn malformed_feature_request_does_not_replace_a_waiting_response() {
        let mut ll = LePeripheralControl::new();
        ll.receive(&FEATURE_REQ, None).unwrap();
        for pdu in [&[3, 0][..], &[3, 2, 8, 0], &[3, 9, 8]] {
            assert_eq!(
                ll.receive(pdu, None),
                Err(LePeripheralControlError::MalformedPdu)
            );
        }
        assert_eq!(ll.pending_response().unwrap().as_bytes()[0], 9);
    }

    #[test]
    fn empty_and_response_pdus_do_not_create_control_loops() {
        let mut ll = LePeripheralControl::new();
        for pdu in [
            &[1, 0][..],
            &[3, 2, 7, 8],
            &[3, 2, 13, 0x1a],
            &[3, 3, 17, 8, 0x1a],
        ] {
            ll.receive(pdu, None).unwrap();
        }
        assert!(ll.pending_response().is_none());
    }

    #[test]
    fn unsupported_mandatory_transitions_and_termination_are_explicit() {
        let mut ll = LePeripheralControl::new();
        assert_eq!(
            ll.receive(&[3, 2, 2, 0x13], None),
            Err(LePeripheralControlError::PeerTermination { reason: 0x13 })
        );
        assert_eq!(
            ll.receive(&[3, 6, 0x0c, 13, 2, 0, 1, 0], None),
            Err(LePeripheralControlError::MandatoryProcedureUnavailable { opcode: 0x0c })
        );
        assert!(ll.pending_response().is_none());
    }
    #[test]
    fn version_is_queued_once_and_preserves_the_product_identity() {
        let version = LeVersionInformation::new(13, 0xffff, 0x1234);
        let request = [3, 6, 12, 12, 2, 0, 0x78, 0x56];
        let mut ll = LePeripheralControl::new();
        ll.receive(&request, Some(version)).unwrap();
        ll.receive(&request, Some(version)).unwrap();
        assert_eq!(
            ll.pending_response().unwrap().as_bytes(),
            [12, 13, 255, 255, 0x34, 0x12]
        );
        assert_eq!(
            ll.peer_version(),
            Some(LeVersionInformation::new(12, 2, 0x5678))
        );
        ll.response_enqueued();
        ll.receive(&request, Some(version)).unwrap();
        assert!(ll.pending_response().is_none());
    }

    #[test]
    fn a_full_queue_or_malformed_version_does_not_consume_the_once_flag() {
        let version = Some(LeVersionInformation::new(13, 0xffff, 1));
        let request = [3, 6, 12, 12, 2, 0, 1, 0];
        let mut ll = LePeripheralControl::new();
        ll.receive(&FEATURE_REQ, None).unwrap();
        ll.receive(&FEATURE_REQ, None).unwrap();
        assert_eq!(
            ll.receive(&request, version),
            Err(LePeripheralControlError::ResponseQueueFull)
        );
        ll.response_enqueued();
        assert_eq!(
            ll.receive(&[3, 1, 12], version),
            Err(LePeripheralControlError::MalformedPdu)
        );
        ll.receive(&request, version).unwrap();
        ll.response_enqueued();
        assert_eq!(ll.pending_response().unwrap().as_bytes()[0], 12);
    }
}
